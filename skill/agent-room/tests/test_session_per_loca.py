"""An agent seated in two locas keeps one loca session at a time. A session
opens only the loca it was taken for, and at another loca's door the server
answers as if that loca did not exist. The kit must not present it there: a
write to the other loca takes a session from that loca's own invite first.
Reported by a production agent on 2026-11-28: called into a second loca, it
heard the room but every send answered 404 "loca does not exist"."""
import json
import os
import subprocess
import tempfile
import time
import unittest
import urllib.request
from pathlib import Path

from test_listener_backfill_e2e import REPO_ROOT, SERVER_BIN, SKILL_DIR, free_port, request_json

ADMIN = "session-loca-admin"
FIRST, SECOND = "first-loca", "second-loca"


class SessionPerLocaEndToEndTests(unittest.TestCase):
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

    def session(self, name, loca, invite):
        made = urllib.request.Request(
            f"{self.base}/sessions", method="POST",
            data=json.dumps({"name": name, "kind": "agent", "loca": loca}).encode())
        made.add_header("content-type", "application/json")
        made.add_header("x-room-token", invite)
        with urllib.request.urlopen(made, timeout=10) as response:
            return json.loads(response.read())["session_token"]

    def env_values(self, path):
        values = {}
        for line in path.read_text(encoding="utf-8").splitlines():
            key, _, value = line.partition("=")
            values[key] = value
        return values

    def test_a_write_to_the_second_loca_takes_its_own_session(self):
        for room in (FIRST, SECOND):
            request_json(f"{self.base}/rooms", method="POST", admin_token=ADMIN,
                         body={"name": room})
        request_json(f"{self.base}/members", method="POST", admin_token=ADMIN,
                     body={"name": "dev", "kind": "agent"})
        invites = {}
        for room in (FIRST, SECOND):
            invites[room] = request_json(f"{self.base}/rooms/{room}/invites", method="POST",
                                         admin_token=ADMIN, body={"name": "dev"})["token"]
        first_session = self.session("dev", FIRST, invites[FIRST])
        # A file from before the session's loca was kept: one session, two
        # invites, nothing saying which loca the session is for.
        env_file = self.root / "dev.env"
        env_file.write_text(
            f"ROOM_SERVER_URL={self.base}\nLOCA_NAME=dev\n"
            f"DAVET_first_loca={invites[FIRST]}\nDAVET_second_loca={invites[SECOND]}\n"
            f"LOCA_SESSION={first_session}\n", encoding="utf-8")
        env_file.chmod(0o600)
        env = {**os.environ, "LOCA_ENV": str(env_file), "HOME": str(self.root)}

        def send(room, text):
            return subprocess.run(
                ["bash", str(SKILL_DIR / "connect.sh"), "send", self.base, room, "dev", "-", text],
                env=env, capture_output=True, text=True, timeout=30)

        sent = send(SECOND, "hello from the second loca")
        self.assertEqual(sent.returncode, 0, sent.stderr)
        kept = self.env_values(env_file)
        self.assertEqual(kept.get("LOCA_SESSION_LOCA"), SECOND)
        self.assertNotEqual(kept.get("LOCA_SESSION"), first_session)

        # Back to the first loca: the kept session is the second's now, so a
        # fresh one is taken from the first loca's invite.
        sent = send(FIRST, "hello again from the first loca")
        self.assertEqual(sent.returncode, 0, sent.stderr)
        self.assertEqual(self.env_values(env_file).get("LOCA_SESSION_LOCA"), FIRST)

        # A read never needs the session: the invite opens it, whatever the
        # kept session is for.
        read = subprocess.run(
            ["bash", str(SKILL_DIR / "connect.sh"), "since", self.base, SECOND, "0"],
            env=env, capture_output=True, text=True, timeout=30)
        self.assertEqual(read.returncode, 0, read.stderr)
        texts = [m["text"] for m in json.loads(read.stdout)]
        self.assertIn("hello from the second loca", texts)


if __name__ == "__main__":
    unittest.main()
