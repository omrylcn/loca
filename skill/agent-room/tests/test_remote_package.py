import base64
import hashlib
import json
import os
import shutil
import subprocess
import tempfile
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlparse


REPO_DIR = Path(__file__).resolve().parents[3]
SKILL_DIR = REPO_DIR / "skill" / "agent-room"
CARE_SKILL_DIR = REPO_DIR / "skill" / "loca-care"
PACKAGE_DIR = REPO_DIR / "packaging" / "remote-agent"


def copy_skills(kit):
    shutil.copytree(SKILL_DIR, kit / "loca")
    shutil.copytree(CARE_SKILL_DIR, kit / "loca-care")


class DaemonHTTPServer(ThreadingHTTPServer):
    daemon_threads = True


class PackageServerHandler(BaseHTTPRequestHandler):
    paths = []
    paths_lock = threading.Lock()

    def log_message(self, *_):
        pass

    def _record(self):
        with self.paths_lock:
            self.paths.append(self.path)

    def _json(self, body, status=200):
        payload = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def _websocket(self):
        key = self.headers.get("Sec-WebSocket-Key", "")
        accept = base64.b64encode(
            hashlib.sha1(
                (key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()
            ).digest()
        ).decode()
        self.send_response(101, "Switching Protocols")
        self.send_header("Upgrade", "websocket")
        self.send_header("Connection", "Upgrade")
        self.send_header("Sec-WebSocket-Accept", accept)
        self.end_headers()
        self.wfile.flush()
        time.sleep(4)

    def do_GET(self):
        self._record()
        path = urlparse(self.path).path
        token = self.headers.get("x-room-token")
        if path == "/health":
            self._json({"ok": True})
        elif path == "/whoami" and token == "mb_worker":
            self._json(
                {
                    "kind": "member",
                    "name": "worker",
                    "member": "mb_worker",
                    "locas": [],
                }
            )
        elif path == "/rooms" and token == "mb_worker":
            self._json([])
        elif path == "/lobby/ws":
            self._websocket()
        else:
            self._json({"error": "unauthorized"}, 401)

    def do_POST(self):
        self._record()
        if self.path == "/sessions":
            length = int(self.headers.get("content-length", "0"))
            self.rfile.read(length)
            self._json({})
        else:
            self._json({"error": "not found"}, 404)


class RemotePackageTests(unittest.TestCase):
    def test_release_kit_is_reproducible_across_runs_and_time_zones(self):
        with tempfile.TemporaryDirectory() as tmp:
            hashes = []
            for timezone in ("UTC", "Europe/Istanbul"):
                env = dict(os.environ)
                env["TMPDIR"] = tmp
                env["TZ"] = timezone
                result = subprocess.run(
                    [str(REPO_DIR / "scripts/build-remote-agent-kit.sh")],
                    cwd=REPO_DIR,
                    env=env,
                    text=True,
                    capture_output=True,
                    timeout=15,
                    check=False,
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                archive = Path(result.stdout.strip())
                hashes.append(hashlib.sha256(archive.read_bytes()).hexdigest())
            self.assertEqual(hashes[0], hashes[1])

    def test_release_kit_contains_v1_rollback_and_v2_runtime_spine(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = dict(os.environ)
            env["TMPDIR"] = tmp
            result = subprocess.run(
                [str(REPO_DIR / "scripts/build-remote-agent-kit.sh")],
                cwd=REPO_DIR,
                env=env,
                text=True,
                capture_output=True,
                timeout=15,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            archive = Path(result.stdout.strip())
            listing = subprocess.run(
                ["unzip", "-Z1", str(archive)],
                text=True,
                capture_output=True,
                check=True,
            ).stdout.splitlines()
            self.assertIn(
                "loca-remote-agent/loca/runtime_agent.py",
                listing,
            )
            self.assertIn(
                "loca-remote-agent/loca/runtime_consumer.py",
                listing,
            )
            self.assertIn(
                "loca-remote-agent/loca/monitor_listener.py",
                listing,
            )
            self.assertIn(
                "loca-remote-agent/loca/references/adapter-protocol-v1.md",
                listing,
            )
            self.assertIn(
                "loca-remote-agent/loca/attention_store.py",
                listing,
            )
            self.assertIn(
                "loca-remote-agent/loca/codex_adapter_v2.py",
                listing,
            )
            self.assertIn(
                "loca-remote-agent/loca/references/adapter-protocol-v2.md",
                listing,
            )
            self.assertIn("loca-remote-agent/loca-care/SKILL.md", listing)
            self.assertIn(
                "loca-remote-agent/loca-care/scripts/audit.py",
                listing,
            )

    def test_installer_requires_an_explicit_server_choice(self):
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp) / "home"
            kit = Path(tmp) / "loca-remote-agent"
            home.mkdir()
            shutil.copytree(PACKAGE_DIR, kit)
            copy_skills(kit)
            env = dict(os.environ)
            env["HOME"] = str(home)
            result = subprocess.run(
                [
                    str(kit / "install.sh"),
                    "--name",
                    "worker",
                    "--target",
                    "claude",
                    "--no-start",
                ],
                env=env,
                text=True,
                capture_output=True,
                timeout=5,
                check=False,
            )
            self.assertEqual(result.returncode, 2)
            self.assertIn("choose a server explicitly", result.stderr)
            self.assertFalse((home / ".loca").exists())

    def test_skill_upgrade_and_rollback_do_not_need_credentials(self):
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp) / "home"
            kit = Path(tmp) / "loca-remote-agent"
            home.mkdir()
            shutil.copytree(PACKAGE_DIR, kit)
            copy_skills(kit)
            for runtime in (".claude", ".codex"):
                installed = home / runtime / "skills" / "loca"
                installed.mkdir(parents=True)
                (installed / "VERSION").write_text("old\n", encoding="utf-8")
                (installed / "SKILL.md").write_text("old\n", encoding="utf-8")
            env = dict(os.environ)
            env["HOME"] = str(home)

            upgraded = subprocess.run(
                [
                    str(kit / "install.sh"),
                    "--upgrade-only",
                    "--target",
                    "both",
                ],
                env=env,
                text=True,
                capture_output=True,
                timeout=10,
                check=False,
            )
            self.assertEqual(upgraded.returncode, 0, upgraded.stderr)
            self.assertNotIn("membership or davet", upgraded.stderr)
            expected_version = (SKILL_DIR / "VERSION").read_text(
                encoding="utf-8"
            )
            for runtime in (".claude", ".codex"):
                parent = home / runtime / "skills"
                backup_parent = (
                    home / ".loca" / "skill-backups" / runtime.removeprefix(".")
                )
                self.assertEqual(
                    (parent / "loca" / "VERSION").read_text(encoding="utf-8"),
                    expected_version,
                )
                self.assertEqual(
                    (parent / "loca-care" / "VERSION").read_text(encoding="utf-8"),
                    (CARE_SKILL_DIR / "VERSION").read_text(encoding="utf-8"),
                )
                self.assertEqual(len(list(parent.glob("loca.backup.*"))), 0)
                self.assertEqual(len(list(backup_parent.glob("loca.backup.*"))), 1)

            rolled_back = subprocess.run(
                [
                    str(kit / "install.sh"),
                    "--rollback",
                    "--target",
                    "both",
                ],
                env=env,
                text=True,
                capture_output=True,
                timeout=10,
                check=False,
            )
            self.assertEqual(rolled_back.returncode, 0, rolled_back.stderr)
            for runtime in (".claude", ".codex"):
                restored = home / runtime / "skills" / "loca"
                self.assertEqual(
                    (restored / "VERSION").read_text(encoding="utf-8"),
                    "old\n",
                )
                self.assertFalse((home / runtime / "skills" / "loca-care").exists())
                parent = home / runtime / "skills"
                self.assertEqual(len(list(parent.glob("loca.backup.*"))), 0)
            self.assertTrue((home / ".loca" / "skill-backups").is_dir())

    def test_upgrade_moves_legacy_backups_out_of_skill_discovery_path(self):
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp) / "home"
            kit = Path(tmp) / "loca-remote-agent"
            home.mkdir()
            shutil.copytree(PACKAGE_DIR, kit)
            copy_skills(kit)
            parent = home / ".codex" / "skills"
            installed = parent / "loca"
            legacy = parent / "loca.backup.20260101T000000Z-1"
            installed.mkdir(parents=True)
            legacy.mkdir()
            (installed / "SKILL.md").write_text("current\n", encoding="utf-8")
            (legacy / "SKILL.md").write_text("legacy\n", encoding="utf-8")
            env = dict(os.environ)
            env["HOME"] = str(home)

            result = subprocess.run(
                [
                    str(kit / "install.sh"),
                    "--upgrade-only",
                    "--target",
                    "codex",
                ],
                env=env,
                text=True,
                capture_output=True,
                timeout=10,
                check=False,
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(len(list(parent.glob("loca.backup.*"))), 0)
            backup_parent = home / ".loca" / "skill-backups" / "codex"
            self.assertEqual(len(list(backup_parent.glob("loca.backup.*"))), 2)

    def test_skill_does_not_prescribe_project_local_tail_wake_bridge(self):
        skill_text = (SKILL_DIR / "SKILL.md").read_text(encoding="utf-8")
        self.assertNotIn("tail of `.agent-room/", skill_text)
        self.assertNotIn("tail -F `.agent-room/", skill_text)
        runtime_text = (SKILL_DIR / "references/runtimes.md").read_text(
            encoding="utf-8"
        )
        self.assertIn("monitor_listener.py", runtime_text)
        self.assertNotIn("2>/dev/null',", runtime_text)

    def test_installer_creates_skill_identity_and_lobby_listener(self):
        PackageServerHandler.paths = []
        server = DaemonHTTPServer(("127.0.0.1", 0), PackageServerHandler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            with tempfile.TemporaryDirectory() as tmp:
                home = Path(tmp) / "home"
                kit = Path(tmp) / "loca-remote-agent"
                home.mkdir()
                shutil.copytree(PACKAGE_DIR, kit)
                copy_skills(kit)
                env = dict(os.environ)
                env["HOME"] = str(home)
                result = subprocess.run(
                    [
                        str(kit / "install.sh"),
                        "--name",
                        "worker",
                        "--server",
                        f"http://127.0.0.1:{server.server_port}",
                        "--target",
                        "claude",
                    ],
                    input="mb_worker\n",
                    env=env,
                    text=True,
                    capture_output=True,
                    timeout=15,
                    check=False,
                )

                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn("LOBBY", result.stdout)
                self.assertTrue(
                    (home / ".claude/skills/loca/SKILL.md").is_file()
                )
                monitor_listener = (
                    home / ".claude/skills/loca/monitor_listener.py"
                )
                self.assertTrue(monitor_listener.is_file())
                self.assertTrue(monitor_listener.stat().st_mode & 0o100)
                identity = home / ".loca/worker.env"
                self.assertIn(
                    "LOCA_MEMBERSHIP=mb_worker",
                    identity.read_text(encoding="utf-8"),
                )
                self.assertNotIn(
                    "ROOM_TOKEN=", identity.read_text(encoding="utf-8")
                )
                pid = int((home / ".loca/run/worker.pid").read_text().strip())
                os.kill(pid, 0)
                command = (
                    Path(f"/proc/{pid}/cmdline")
                    .read_bytes()
                    .replace(b"\0", b" ")
                    .decode(errors="replace")
                )
                self.assertIn("--turn-log", command)
                self.assertIn(
                    str(home / ".loca/inbox/worker.jsonl"),
                    command,
                )

                stopped = subprocess.run(
                    [str(home / ".local/bin/loca-stop"), "worker"],
                    env=env,
                    text=True,
                    capture_output=True,
                    timeout=5,
                    check=False,
                )
                self.assertEqual(stopped.returncode, 0, stopped.stderr)
                room_attempts = [
                    path
                    for path in PackageServerHandler.paths
                    if urlparse(path).path == "/ws"
                ]
                self.assertEqual(
                    room_attempts,
                    [],
                    "membership-only install must open the lobby, not a fake room",
                )
                self.assertTrue(
                    any(
                        urlparse(path).path == "/lobby/ws"
                        for path in PackageServerHandler.paths
                    )
                )
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)


if __name__ == "__main__":
    unittest.main()
