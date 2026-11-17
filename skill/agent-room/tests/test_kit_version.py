"""This kit's version: the listener sends it when it connects, and the status
commands say, in one line, when this kit is older than the server. Equal,
newer, missing or unparsable versions add nothing, and no exit code moves."""
import json
import os
import stat
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

SKILL_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SKILL_DIR))

import kit_version  # noqa: E402
import test_listener_backfill_e2e as harness  # noqa: E402

KIT = (SKILL_DIR / "VERSION").read_text(encoding="utf-8").strip()
UPDATE = "./install.sh --upgrade-only --target both"


def bumped(version, major=0, minor=0, patch=0):
    a, b, c = (int(part) for part in version.split("-")[0].split("."))
    return f"{a + major}.{b + minor}.{c + patch}"


OLDER_SERVER = bumped(KIT, minor=1)
NEWER_KIT_SERVER = "0.0.1"


class NoticeTests(unittest.TestCase):
    def test_the_line_names_both_versions_and_the_documented_update(self):
        line = kit_version.older_notice("0.15.0", "0.16.0")
        self.assertIn("kit 0.15.0", line)
        self.assertIn("server 0.16.0", line)
        self.assertIn(UPDATE, line)
        self.assertNotIn("\n", line)

    def test_only_an_older_kit_gets_a_line(self):
        for kit, server in [
            ("0.16.0", "0.16.0"),
            ("0.17.0", "0.16.0"),
            ("0.16.0-rc.1", "0.16.0"),
            (None, "0.16.0"),
            ("0.16.0", None),
            ("0.16.0", ""),
            ("0.16.0", "unknown"),
            ("0.16.0", "v0.17.0"),
            ("0.16.0", "0.17"),
            ("garbage", "0.17.0"),
        ]:
            self.assertIsNone(kit_version.older_notice(kit, server), (kit, server))
        for kit, server in [("0.15.9", "0.16.0"), ("0.16.0", "1.0.0"), ("0.9.0", "0.10.0")]:
            self.assertIsNotNone(kit_version.older_notice(kit, server), (kit, server))

    def test_the_kit_reads_its_own_version_file(self):
        self.assertEqual(kit_version.kit_version(), KIT)
        with tempfile.TemporaryDirectory() as tmp:
            self.assertIsNone(kit_version.kit_version(tmp), "no VERSION file")
            (Path(tmp) / "VERSION").write_text("not a version\n", encoding="utf-8")
            self.assertIsNone(kit_version.kit_version(tmp))

    def test_the_version_rides_the_query_and_nothing_else_changes(self):
        url = "wss://loca.example/ws?room=r&name=a%20b&type=agent"
        sent = kit_version.with_kit_version(url, "0.16.0")
        self.assertEqual(sent, url.replace("a%20b", "a+b") + "&kit_version=0.16.0")
        self.assertEqual(
            kit_version.with_kit_version("wss://loca.example/lobby/ws", "0.16.0"),
            "wss://loca.example/lobby/ws?kit_version=0.16.0",
        )
        self.assertEqual(kit_version.with_kit_version(sent, "9.9.9"), sent, "never twice")
        self.assertEqual(kit_version.with_kit_version(url, ""), url)


class RunningVersionTests(unittest.TestCase):
    def test_a_listener_keeps_reporting_the_version_it_started_with(self):
        # install.sh --upgrade-only replaces the kit on disk and does not
        # restart listeners; a reconnect must still say what is running.
        import importlib.util
        import shutil
        with tempfile.TemporaryDirectory() as tmp:
            kit = Path(tmp) / "kit"
            kit.mkdir()
            shutil.copy(SKILL_DIR / "kit_version.py", kit / "kit_version.py")
            (kit / "VERSION").write_text("0.15.0\n", encoding="utf-8")
            spec = importlib.util.spec_from_file_location("kit_version_copy", kit / "kit_version.py")
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
            first = module.with_kit_version("ws://127.0.0.1:1/ws?room=a")
            (kit / "VERSION").write_text("0.99.0\n", encoding="utf-8")
            again = module.with_kit_version("ws://127.0.0.1:1/ws?room=a")
            self.assertIn("kit_version=0.15.0", first)
            self.assertEqual(first, again, "a reconnect reports the running version, not the file")
            # The status commands describe the installed kit: they read the file.
            self.assertEqual(module.kit_version(kit), "0.99.0")


class HealthHandler(BaseHTTPRequestHandler):
    """A Building whose /health says `version` (None: no field; a non-JSON
    string: not JSON at all) and that knows one member key."""

    version = None

    def log_message(self, *_):
        pass

    def _send(self, status, body, content_type="application/json"):
        payload = body.encode()
        self.send_response(status)
        self.send_header("content-type", content_type)
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def do_GET(self):
        token = self.headers.get("x-room-token")
        if self.path == "/health":
            version = type(self).version
            if isinstance(version, str) and version.startswith("<"):
                self._send(200, version, "text/html")
                return
            health = {"ok": True}
            if version is not None:
                health["version"] = version
            self._send(200, json.dumps(health))
        elif self.path == "/whoami" and token == "mb_worker":
            self._send(200, json.dumps({"kind": "member", "name": "worker", "locas": []}))
        else:
            self._send(401, json.dumps({"error": "unauthorized"}))


class StatusNoticeTests(unittest.TestCase):
    def run_command(self, command, server_version, *, membership="mb_worker"):
        handler = type("Health", (HealthHandler,), {"version": server_version})
        server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            with tempfile.TemporaryDirectory() as tmp:
                home = Path(tmp)
                origin = f"http://127.0.0.1:{server.server_port}"
                env_file = home / ".loca/worker.env"
                env_file.parent.mkdir()
                env_file.write_text(
                    f"ROOM_SERVER_URL={origin}\nLOCA_NAME=worker\nLOCA_MEMBERSHIP={membership}\n",
                    encoding="utf-8",
                )
                env_file.chmod(0o600)
                # A hermetic user systemd: no unit is active.
                bin_dir = home / "bin"
                bin_dir.mkdir()
                systemctl = bin_dir / "systemctl"
                systemctl.write_text("#!/bin/sh\nexit 1\n", encoding="utf-8")
                systemctl.chmod(systemctl.stat().st_mode | stat.S_IXUSR)
                env = {
                    key: value for key, value in os.environ.items()
                    if not key.startswith(("LOCA_", "ROOM_", "DAVET_"))
                }
                env.update({"HOME": tmp, "LOCA_ENV": str(env_file),
                            "PATH": f"{bin_dir}:{os.environ['PATH']}"})
                if command == "runtime-status":
                    args = ["bash", str(SKILL_DIR / "runtime.sh"), "status", "worker",
                            "--env", str(env_file)]
                elif command == "status":
                    args = [str(SKILL_DIR / "connect.sh"), "status", origin, "worker"]
                else:
                    args = [str(SKILL_DIR / "connect.sh"), "doctor", origin]
                return subprocess.run(args, env=env, text=True, capture_output=True,
                                      timeout=30, check=False)
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)

    def notices(self, result):
        return [line for line in result.stdout.splitlines() if "KIT OLDER THAN SERVER" in line]

    def test_each_status_command_says_it_once_only_when_the_kit_is_older(self):
        for command in ("status", "doctor", "runtime-status"):
            with self.subTest(command=command):
                quiet = {}
                for label, server_version in [
                    ("equal", KIT),
                    ("newer kit", NEWER_KIT_SERVER),
                    ("no version", None),
                    ("unparsable", "banana"),
                    ("not json", "<html>proxy</html>"),
                ]:
                    result = self.run_command(command, server_version)
                    self.assertEqual(self.notices(result), [], f"{label}:\n{result.stdout}")
                    self.assertNotIn("KIT OLDER", result.stderr, label)
                    quiet[label] = result.returncode
                older = self.run_command(command, OLDER_SERVER)
                lines = self.notices(older)
                self.assertEqual(len(lines), 1, older.stdout)
                self.assertIn(f"kit {KIT}, server {OLDER_SERVER}", lines[0])
                self.assertIn(UPDATE, lines[0])
                # The notice never moves an exit code.
                self.assertEqual(set(quiet.values()), {older.returncode}, (quiet, older.returncode))
                if command == "status":
                    self.assertEqual(older.returncode, 0, older.stderr)

    def test_a_refused_status_keeps_its_exit_code(self):
        quiet = self.run_command("status", KIT, membership="mb_unknown")
        older = self.run_command("status", OLDER_SERVER, membership="mb_unknown")
        self.assertEqual((quiet.returncode, older.returncode), (3, 3))
        self.assertIn("membership rejected", older.stderr)


class ListenerSendsItsVersionEndToEnd(unittest.TestCase):
    """The shipped listener against the real server: its Lobby socket and the
    seat it is called into both carry this kit's VERSION."""

    @classmethod
    def setUpClass(cls):
        harness.DurableBackfillEndToEndTests.setUpClass()

    def until(self, check, message, timeout=15):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                if check():
                    return
            except (OSError, ValueError):
                pass
            time.sleep(0.05)
        self.fail(message)

    def test_the_listener_sends_this_kits_version(self):
        server = listener = server_log = None
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            port = harness.free_port()
            base = f"http://127.0.0.1:{port}"
            admin = "e2e-admin"
            try:
                server, server_log = harness.DurableBackfillEndToEndTests.start_server(
                    self, port, root / "server.db", root / "server.log")
                member = harness.request_json(f"{base}/members", method="POST", admin_token=admin,
                                              body={"name": "kit-agent", "kind": "agent"})
                harness.request_json(f"{base}/rooms", method="POST", admin_token=admin,
                                     body={"name": "kit-room"})
                env_file = root / "agent.env"
                env_file.write_text(f"ROOM_SERVER_URL={base}\nLOCA_NAME=kit-agent\n"
                                    f"LOCA_MEMBERSHIP={member['token']}\n")
                env_file.chmod(0o600)
                env = {**os.environ, "LOCA_ENV": str(env_file)}

                def resident():
                    rows = harness.request_json(f"{base}/residents", admin_token=admin)
                    return next(row for row in rows if row["name"] == "kit-agent")

                def seat():
                    rows = harness.request_json(f"{base}/rooms/kit-room/members", admin_token=admin)
                    return next((row for row in rows if row["name"] == "kit-agent"), None)

                with (root / "listener.log").open("ab") as errors:
                    listener = subprocess.Popen(
                        ["python3", str(SKILL_DIR / "listen.py"),
                         f"ws://127.0.0.1:{port}/ws?room=&name=kit-agent&type=agent", "-"],
                        cwd=harness.REPO_ROOT, env=env, stdout=subprocess.DEVNULL, stderr=errors)
                    # In the Lobby, seated nowhere.
                    self.until(lambda: resident()["online"] and resident().get("kit_version") == KIT,
                               "the Lobby socket did not carry the kit version:\n"
                               + (root / "listener.log").read_text(errors="replace"))
                    self.assertEqual(resident()["locas"], [])
                    # Called into a loca: the seat carries it too.
                    harness.request_json(f"{base}/rooms/kit-room/call", method="POST",
                                         admin_token=admin, body={"name": "kit-agent"})
                    self.until(lambda: (seat() or {}).get("kit_version") == KIT,
                               "the seat did not carry the kit version:\n"
                               + (root / "listener.log").read_text(errors="replace"))
                    harness.DurableBackfillEndToEndTests.stop_process(listener)
                    listener = None
                    # Gone with the connection.
                    self.until(lambda: "kit_version" not in resident() and seat() is None,
                               "the kit version outlived the listener")
            finally:
                if listener is not None:
                    harness.DurableBackfillEndToEndTests.stop_process(listener)
                if server is not None:
                    harness.DurableBackfillEndToEndTests.stop_process(server)
                if server_log is not None:
                    server_log.close()


if __name__ == "__main__":
    unittest.main()
