"""`connect.sh lesson-propose` and `lessons` against the real server: an
agent proposes a lesson about its own work under its session, reads the
waiting and accepted ones, and finds an accepted lesson in the rules."""
import json
import os
import subprocess
import tempfile
import time
import unittest
import urllib.request
from pathlib import Path

from test_listener_backfill_e2e import REPO_ROOT, SERVER_BIN, SKILL_DIR, free_port, request_json

ADMIN = "lessons-e2e-admin"


class LessonCommandsEndToEndTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if SERVER_BIN.is_file():
            return
        subprocess.run(["cargo", "build", "-q", "-p", "server"], cwd=REPO_ROOT,
                       check=True, timeout=180)

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        port = free_port()
        self.base = f"http://127.0.0.1:{port}"
        env = dict(os.environ)
        env.update({
            "PORT": str(port), "BIND_ADDR": "127.0.0.1",
            "DB_PATH": str(self.root / "loca.sqlite3"), "ADMIN_TOKEN": ADMIN,
            "RATE_LIMIT": "0", "REQUIRE_SESSIONS": "1", "REQUIRE_INVITE": "1",
            "LEGACY_WS_QUERY_AUTH": "0", "RUST_LOG": "warn",
        })
        self.server_log = (self.root / "server.log").open("ab")
        self.server = subprocess.Popen([str(SERVER_BIN)], cwd=REPO_ROOT, env=env,
                                       stdout=self.server_log, stderr=subprocess.STDOUT)
        deadline = time.monotonic() + 10
        while True:
            try:
                if request_json(f"{self.base}/health").get("ok") is True:
                    break
            except Exception:
                if time.monotonic() > deadline:
                    self.fail("room-server did not become healthy")
                time.sleep(0.05)

    def tearDown(self):
        if self.server.poll() is None:
            self.server.terminate()
            try:
                self.server.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self.server.kill()
                self.server.wait(timeout=3)
        self.server_log.close()
        self.tmp.cleanup()

    def connect(self, env, *args):
        return subprocess.run(["bash", str(SKILL_DIR / "connect.sh"), *args],
                              env=env, capture_output=True, text=True, timeout=30)

    def test_an_agent_proposes_and_reads_lessons(self):
        room = "lessons-room"
        request_json(f"{self.base}/rooms", method="POST", admin_token=ADMIN,
                     body={"name": room, "extensions": ["journal", "vault"]})
        request_json(f"{self.base}/members", method="POST", admin_token=ADMIN,
                     body={"name": "nova", "kind": "agent"})
        invite = request_json(f"{self.base}/rooms/{room}/invites", method="POST",
                              admin_token=ADMIN, body={"name": "nova"})["token"]
        made = urllib.request.Request(
            f"{self.base}/sessions", method="POST",
            data=json.dumps({"name": "nova", "kind": "agent", "loca": room}).encode())
        made.add_header("content-type", "application/json")
        made.add_header("x-room-token", invite)
        with urllib.request.urlopen(made, timeout=10) as response:
            session = json.loads(response.read())["session_token"]
        env_file = self.root / "nova.env"
        env_file.write_text(
            f"ROOM_SERVER_URL={self.base}\nLOCA_NAME=nova\nDAVET_lessons_room={invite}\n"
            f"LOCA_SESSION={session}\n", encoding="utf-8")
        env_file.chmod(0o600)
        env = {**os.environ, "LOCA_ENV": str(env_file), "HOME": str(self.root)}

        proposed = self.connect(env, "lesson-propose", self.base, room, "nova",
                                "run the migration test before asking for review")
        self.assertEqual(proposed.returncode, 0, proposed.stderr)
        self.assertEqual(proposed.stdout.strip(),
                         "proposed lesson #1: run the migration test before asking for review")

        empty = self.connect(env, "lesson-propose", self.base, room, "nova", " ")
        self.assertEqual(empty.returncode, 8)
        self.assertIn("NOT PROPOSED (HTTP 400)", empty.stderr)

        listed = self.connect(env, "lessons", self.base, room)
        self.assertEqual(listed.returncode, 0, listed.stderr)
        self.assertEqual(listed.stdout.splitlines(), [
            f"Lessons of {room}: 1 waiting, 0 accepted",
            "  waiting #1: run the migration test before asking for review (proposed by nova)",
        ])

        # The agent cannot accept its own lesson; the Master can.
        request = urllib.request.Request(
            f"{self.base}/rooms/{room}/vault/lessons/1/accept", method="POST", data=b"{}")
        request.add_header("content-type", "application/json")
        request.add_header("x-room-token", invite)
        request.add_header("x-session-token", session)
        with self.assertRaises(urllib.error.HTTPError) as refused:
            urllib.request.urlopen(request, timeout=10)
        self.assertEqual(refused.exception.code, 403)
        request_json(f"{self.base}/rooms/{room}/vault/lessons/1/accept", method="POST",
                     admin_token=ADMIN, body={})

        listed = self.connect(env, "lessons", self.base, room)
        self.assertIn(
            "  accepted #1: run the migration test before asking for review "
            "(proposed by nova, by Master)", listed.stdout)
        rules = self.connect(env, "vault", self.base, room, "nova")
        self.assertEqual(rules.returncode, 0, rules.stderr)
        self.assertIn("## Lessons", rules.stdout)
        self.assertIn("- run the migration test before asking for review (proposed by nova)",
                      rules.stdout)


if __name__ == "__main__":
    unittest.main()
