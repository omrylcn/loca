"""The kit's mentat commands against the real server: a room founded as a
mentat room is listed in the Building's directory, two agents ask the mentat
in turn under their sessions, a third joins and withdraws, the mentat merges
and pauses, an asker ends saying it was answered, and the line reads back as
the server keeps it."""
import json
import os
import subprocess
import tempfile
import time
import unittest
import urllib.request
from pathlib import Path

from test_listener_backfill_e2e import REPO_ROOT, SERVER_BIN, SKILL_DIR, free_port, request_json

ADMIN = "mentat-e2e-admin"
ROOM = "sage-room"


class MentatCommandsEndToEndTests(unittest.TestCase):
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

    def seat(self, name):
        admitted = request_json(f"{self.base}/members", method="POST", admin_token=ADMIN,
                                body={"name": name, "kind": "agent"})
        membership = next((v for v in admitted.values()
                           if isinstance(v, str) and v.startswith("mb_")), "")
        invite = request_json(f"{self.base}/rooms/{ROOM}/invites", method="POST",
                              admin_token=ADMIN, body={"name": name})["token"]
        made = urllib.request.Request(
            f"{self.base}/sessions", method="POST",
            data=json.dumps({"name": name, "kind": "agent", "loca": ROOM}).encode())
        made.add_header("content-type", "application/json")
        made.add_header("x-room-token", invite)
        with urllib.request.urlopen(made, timeout=10) as response:
            session = json.loads(response.read())["session_token"]
        env_file = self.root / f"{name}.env"
        env_file.write_text(
            f"ROOM_SERVER_URL={self.base}\nLOCA_NAME={name}\nDAVET_sage_room={invite}\n"
            f"LOCA_SESSION={session}\nLOCA_MEMBERSHIP={membership}\n", encoding="utf-8")
        env_file.chmod(0o600)
        return {**os.environ, "LOCA_ENV": str(env_file), "HOME": str(self.root)}

    def connect(self, env, *args):
        return subprocess.run(["bash", str(SKILL_DIR / "connect.sh"), *args],
                              env=env, capture_output=True, text=True, timeout=30)

    def test_agents_ask_in_turn_join_withdraw_merge_and_end(self):
        request_json(f"{self.base}/rooms", method="POST", admin_token=ADMIN,
                     body={"name": ROOM, "mentat": {"topic": "sessions, keys and releases"}})
        sage, ada, bob, cem, dan = (self.seat(n) for n in ("sage", "ada", "bob", "cem", "dan"))

        # A room with no mentat yet: listed, and nobody can ask.
        listed = self.connect(ada, "mentats", self.base, "ada")
        self.assertEqual(listed.returncode, 0, listed.stderr)
        self.assertEqual(listed.stdout.strip(),
                         f"{ROOM}: sessions, keys and releases — no mentat yet, 0 waiting")
        refused = self.connect(ada, "mentat-ask", self.base, ROOM, "ada", "how sessions renew")
        self.assertEqual(refused.returncode, 8)
        self.assertIn("no mentat yet", refused.stderr)

        request_json(f"{self.base}/rooms/{ROOM}/mentat", method="PUT", admin_token=ADMIN,
                     body={"mentat": "sage"})
        asked = self.connect(ada, "mentat-ask", self.base, ROOM, "ada", "how sessions renew")
        self.assertEqual(asked.returncode, 0, asked.stderr)
        self.assertEqual(asked.stdout.strip(),
                         "request #1: your turn now — write your question here")
        asked = self.connect(bob, "mentat-ask", self.base, ROOM, "bob", "the release order")
        self.assertEqual(asked.stdout.strip(), "request #2 is waiting in the line; a line "
                         "naming you says when it is your turn")
        again = self.connect(bob, "mentat-ask", self.base, ROOM, "bob", "something else again")
        self.assertEqual(again.returncode, 8)
        self.assertIn("NOT OPENED (HTTP 409)", again.stderr)
        self.connect(dan, "mentat-ask", self.base, ROOM, "dan", "also the release order")

        joined = self.connect(cem, "mentat-join", self.base, ROOM, "cem", "2")
        self.assertEqual(joined.returncode, 0, joined.stderr)
        self.assertEqual(joined.stdout.strip(), "joined request #2: @bob @cem")
        left = self.connect(cem, "mentat-withdraw", self.base, ROOM, "cem", "2")
        self.assertEqual(left.returncode, 0, left.stderr)
        self.assertEqual(left.stdout.strip(), "you left request #2")

        # Only the mentat merges; Dan's request joins Bob's.
        refused = self.connect(bob, "mentat-merge", self.base, ROOM, "bob", "3", "2")
        self.assertEqual(refused.returncode, 4)
        merged = self.connect(sage, "mentat-merge", self.base, ROOM, "sage", "3", "2")
        self.assertEqual(merged.returncode, 0, merged.stderr)
        self.assertEqual(merged.stdout.strip(), "request #2 now: @bob @dan")

        line = self.connect(bob, "mentat", self.base, ROOM)
        self.assertEqual(line.returncode, 0, line.stderr)
        self.assertEqual(line.stdout.splitlines()[0], "topic: sessions, keys and releases")
        self.assertEqual(line.stdout.splitlines()[1], "mentat: @sage")
        self.assertIn("inside: #1 @ada (called, 0 questions): how sessions renew",
                      line.stdout)
        self.assertIn("waiting: #2 @bob @dan: the release order", line.stdout)

        # Bob cannot end Ada's consultation; Ada can, saying it was answered.
        refused = self.connect(bob, "mentat-end", self.base, ROOM, "bob")
        self.assertEqual(refused.returncode, 4)
        ended = self.connect(ada, "mentat-end", self.base, ROOM, "ada", "--answered", "partly")
        self.assertEqual(ended.returncode, 0, ended.stderr)
        self.assertEqual(ended.stdout.strip(), "consultation #1 ended")
        history = request_json(f"{self.base}/rooms/{ROOM}/mentat/history", admin_token=ADMIN)
        first = next(r for r in history["requests"] if r["id"] == 1)
        self.assertEqual(first["answered"], "partly")
        line = self.connect(ada, "mentat", self.base, ROOM)
        self.assertIn("inside: #2 @bob @dan (called, 0 questions)", line.stdout)
        self.assertIn("waiting: nobody", line.stdout)

        # The mentat pauses its line and lets it move again.
        paused = self.connect(sage, "mentat-pause", self.base, ROOM, "sage")
        self.assertEqual(paused.returncode, 0, paused.stderr)
        self.assertEqual(paused.stdout.strip(), "the line is paused")
        moved = self.connect(sage, "mentat-resume", self.base, ROOM, "sage")
        self.assertEqual(moved.stdout.strip(), "the line moves")
        listed = self.connect(ada, "mentats", self.base, "ada")
        self.assertIn(f"{ROOM}: sessions, keys and releases — mentat @sage", listed.stdout)

if __name__ == "__main__":
    unittest.main()
