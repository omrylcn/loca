import os
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path


SKILL_DIR = Path(__file__).resolve().parents[1]


class RuntimeScriptTests(unittest.TestCase):
    def _start_with_fake_systemd(self, requested_runtime, *, thread_id=None):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        home = Path(temp.name)
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
        subprocess.run(
            [
                "bash",
                str(SKILL_DIR / "runtime.sh"),
                "start",
                "worker",
                "--runtime",
                requested_runtime,
                "--env",
                str(identity),
            ],
            text=True,
            capture_output=True,
            env=env,
            timeout=5,
            check=False,
        )
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

    def test_legacy_codex_v1_requires_an_explicit_thread(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = dict(os.environ)
            env["HOME"] = tmp
            env.pop("CODEX_THREAD_ID", None)
            result = subprocess.run(
                [
                    "bash",
                    str(SKILL_DIR / "runtime.sh"),
                    "start",
                    "worker",
                    "--runtime",
                    "codex-v1",
                ],
                text=True,
                capture_output=True,
                env=env,
                timeout=5,
                check=False,
            )

            self.assertEqual(result.returncode, 2)
            self.assertIn("legacy Codex v1 thread id missing", result.stderr)

    def test_codex_v2_shadow_requires_the_existing_v1_thread(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = dict(os.environ)
            env["HOME"] = tmp
            env.pop("CODEX_THREAD_ID", None)
            result = subprocess.run(
                [
                    "bash",
                    str(SKILL_DIR / "runtime.sh"),
                    "start",
                    "worker",
                    "--runtime",
                    "codex-v2",
                    "--relay-mode",
                    "shadow",
                ],
                text=True,
                capture_output=True,
                env=env,
                timeout=5,
                check=False,
            )

            self.assertEqual(result.returncode, 2)
            self.assertIn("requires --thread-id", result.stderr)

    def test_codex_v2_shadow_launcher_keeps_v1_and_v2_queues_separate(self):
        script = (SKILL_DIR / "runtime.sh").read_text(encoding="utf-8")

        self.assertIn('inbox_file="$inbox_dir/$safe.v2-shadow.jsonl"', script)
        self.assertIn(
            'worker_cursor_file="$worker_cursor_dir/$safe.v2-shadow.json"',
            script,
        )
        self.assertIn('"--exec" "$hook"', script)
        self.assertIn('"--shadow-persistent-exec" "$shadow_hook"', script)
        self.assertIn('"--shadow-ready-file" "$shadow_health_file"', script)
        self.assertIn('hook="$(printf \'%q \' python3 "$SKILL_DIR/nudge.py" codex', script)
        self.assertIn(
            'v2_hook="exec $(printf \'%q \' python3 "$SKILL_DIR/codex_adapter_v2.py"',
            script,
        )

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
