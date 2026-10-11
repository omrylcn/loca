import os
import json
import shutil
import stat
import subprocess
import tempfile
import time
import unittest
from pathlib import Path


SKILL_DIR = Path(__file__).resolve().parents[1]


class RuntimeScriptTests(unittest.TestCase):
    def _start_with_fake_systemd(self, requested_runtime, *, thread_id=None, options=()):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        home = Path(temp.name)
        # Exercise launcher selection without probing a real Building or a
        # caller's credentials. status is covered by credential/API tests.
        isolated_skill = home / "skill"
        isolated_skill.mkdir()
        shutil.copyfile(SKILL_DIR / "runtime.sh", isolated_skill / "runtime.sh")
        connect = isolated_skill / "connect.sh"
        connect.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        connect.chmod(0o700)
        identity = home / ".loca/worker.env"
        identity.parent.mkdir(parents=True)
        identity.write_text(
            "ROOM_SERVER_URL=http://127.0.0.1:9\nLOCA_NAME=worker\n",
            encoding="utf-8",
        )
        fake_bin = home / "bin"
        fake_bin.mkdir()
        systemctl = fake_bin / "systemctl"
        systemctl.write_text(
            """#!/bin/sh
case "$*" in
  *show-environment*) exit 0 ;;
  *is-active*) exit 1 ;;
  *MainPID*) echo "$FAKE_MAIN_PID"; exit 0 ;;
  *) exit 0 ;;
esac
""",
            encoding="utf-8",
        )
        systemctl.chmod(systemctl.stat().st_mode | stat.S_IXUSR)
        env = dict(os.environ)
        env["HOME"] = str(home)
        env["PATH"] = f"{fake_bin}:/usr/bin:/bin"
        env["CODEX_BIN"] = "/bin/true"
        env["FAKE_MAIN_PID"] = str(os.getpid())
        if thread_id is None:
            env.pop("CODEX_THREAD_ID", None)
        else:
            env["CODEX_THREAD_ID"] = thread_id
        result = subprocess.run(
            [
                "bash",
                str(isolated_skill / "runtime.sh"),
                "start",
                "worker",
                "--runtime",
                requested_runtime,
                "--env",
                str(identity),
                *options,
            ],
            text=True,
            capture_output=True,
            env=env,
            timeout=5,
            check=False,
        )
        self.last_start_result = result
        self.last_start_env = env
        return home

    def test_auto_codex_uses_v2_live_reply_relay(self):
        home = self._start_with_fake_systemd("auto", thread_id="ide-thread")

        run = home / ".loca/run"
        self.assertEqual((run / "worker.runtime").read_text().strip(), "codex-v2")
        self.assertEqual((run / "worker.relay-mode").read_text().strip(), "live")
        launcher = (run / "worker-service.sh").read_text(encoding="utf-8")
        self.assertIn("--persistent-exec", launcher)
        self.assertIn("codex_adapter_v2.py", launcher)
        self.assertNotIn("nudge.py", launcher)

    def test_codex_public_alias_uses_v2_live_without_interactive_thread(self):
        home = self._start_with_fake_systemd("codex")

        run = home / ".loca/run"
        self.assertEqual((run / "worker.runtime").read_text().strip(), "codex-v2")
        self.assertEqual((run / "worker.relay-mode").read_text().strip(), "live")

    def test_removed_v1_rejected_even_with_interactive_thread(self):
        home = self._start_with_fake_systemd("codex-v1", thread_id="ide-thread")
        self.assertEqual(self.last_start_result.returncode, 2)
        self.assertIn("Codex v1 was removed", self.last_start_result.stderr)
        self.assertFalse((home / ".loca/run/worker-service.sh").exists())

    def test_interactive_thread_rebind_is_rejected(self):
        home = self._start_with_fake_systemd("codex", options=("--thread-id", "ide-thread"))
        self.assertEqual(self.last_start_result.returncode, 2)
        self.assertIn("thread binding was removed", self.last_start_result.stderr)
        self.assertFalse((home / ".loca/run/worker-service.sh").exists())

    def test_shadow_observer_starts_only_v2_without_a_v1_responder(self):
        home = self._start_with_fake_systemd("codex-v2", options=("--relay-mode", "shadow"))
        self.assertEqual(self.last_start_result.returncode, 0, self.last_start_result.stderr)
        run = home / ".loca/run"
        self.assertEqual((run / "worker.runtime").read_text().strip(), "codex-v2")
        self.assertEqual((run / "worker.relay-mode").read_text().strip(), "shadow")
        launcher = (run / "worker-service.sh").read_text()
        import shlex
        argv = shlex.split(next(line for line in launcher.splitlines() if line.startswith("exec ")))
        self.assertIn("--persistent-exec", argv)
        self.assertIn("--persistent-ready-file", argv)
        self.assertNotIn("--exec", argv)
        self.assertNotIn("--shadow-persistent-exec", argv)
        self.assertNotIn("nudge.py", launcher)
        self.assertNotIn("--wiki-runtime", argv)
        command = argv[argv.index("--persistent-exec") + 1]
        self.assertIn("codex_adapter_v2.py", command)
        self.assertIn("--relay-mode shadow", command)
        self.assertFalse((run / "worker.codex-thread").exists())

    def test_shadow_status_cannot_claim_healthy_live_monitoring(self):
        home = self._start_with_fake_systemd("codex-v2", options=("--relay-mode", "shadow"))
        (home / ".loca/run/worker.health.json").write_text(json.dumps({
            "updated_at_ms": int(time.time() * 1000),
            "wake": "OK", "reply": "OK", "ack": "OK",
            "reply_required_pending": 0,
        }))
        result = subprocess.run(
            ["bash", str(home / "skill/runtime.sh"), "status", "worker"],
            env=self.last_start_env, text=True, capture_output=True, timeout=10,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("agent health: DEGRADED (no supported live Codex responder)", result.stdout)
        self.assertNotIn("agent health: OK", result.stdout)
        self.assertIn("shadow observer; no room responder", result.stdout)

    def test_manual_start_does_not_replace_running_codex_runtime(self):
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp)
            run = home / ".loca/run"
            run.mkdir(parents=True)
            identity = home / ".loca/worker.env"
            identity.write_text(
                "ROOM_SERVER_URL=https://loca.example\nLOCA_NAME=worker\n",
                encoding="utf-8",
            )
            (run / "worker.runtime").write_text("codex\n", encoding="utf-8")
            (run / "worker.delivery").write_text("direct\n", encoding="utf-8")
            (run / "worker.codex-sandbox").write_text(
                "danger-full-access\n", encoding="utf-8"
            )

            fake_bin = home / "bin"
            fake_bin.mkdir()
            systemctl = fake_bin / "systemctl"
            systemctl.write_text(
                """#!/bin/sh
case "$*" in
  *show-environment*) exit 0 ;;
  *is-active*) exit 0 ;;
  *is-enabled*) exit 0 ;;
  *MainPID*) echo 4242; exit 0 ;;
esac
exit 1
""",
                encoding="utf-8",
            )
            systemctl.chmod(systemctl.stat().st_mode | stat.S_IXUSR)
            env = dict(os.environ)
            env["HOME"] = str(home)
            env["PATH"] = f"{fake_bin}:/usr/bin:/bin"
            result = subprocess.run(
                [
                    "bash",
                    str(SKILL_DIR / "runtime.sh"),
                    "start",
                    "worker",
                    "--runtime",
                    "manual",
                    "--env",
                    str(identity),
                ],
                text=True,
                capture_output=True,
                env=env,
                timeout=5,
                check=False,
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("preserved existing runtime=codex", result.stdout)
            self.assertEqual(
                (run / "worker.runtime").read_text(encoding="utf-8").strip(),
                "codex",
            )


if __name__ == "__main__":
    unittest.main()
