"""Role and extension cards against the real server: an agent is told what
is switched on in its loca on its first turn there, and then only what
changed (it became the Lead, the Loca Board was turned on, then off), once,
on its next real turn, without a turn of its own."""
import json
import os
import queue
import subprocess
import tempfile
import threading
import time
import unittest
from pathlib import Path

from test_listener_backfill_e2e import REPO_ROOT, SERVER_BIN, SKILL_DIR, free_port, request_json

ADMIN = "role-e2e-admin"
ROOM_KEY = "role-e2e-room"


class RoleContextEndToEndTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if SERVER_BIN.is_file():
            return
        subprocess.run(["cargo", "build", "-q", "-p", "server"], cwd=REPO_ROOT,
                       check=True, timeout=180)

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.port = free_port()
        self.base = f"http://127.0.0.1:{self.port}"
        env = dict(os.environ)
        env.update({
            "PORT": str(self.port), "BIND_ADDR": "127.0.0.1",
            "DB_PATH": str(self.root / "loca.sqlite3"), "ADMIN_TOKEN": ADMIN,
            "ROOM_TOKEN": ROOM_KEY, "RATE_LIMIT": "0", "REQUIRE_SESSIONS": "0",
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
        self.listener = None
        self.listener_log = None

    def tearDown(self):
        for process in (self.listener, self.server):
            if process is not None and process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=3)
        if self.listener is not None:
            self.listener.stdout.close()
        for handle in (self.listener_log, self.server_log):
            if handle is not None:
                handle.close()
        self.tmp.cleanup()

    def post(self, room, text):
        request_json(f"{self.base}/rooms/{room}/messages", method="POST", token=ROOM_KEY,
                     body={"sender": "operator", "sender_type": "user", "target": "worker",
                           "text": text})

    def test_cards_come_on_the_first_turn_and_then_only_for_what_changed(self):
        room = "role-room"
        request_json(f"{self.base}/rooms", method="POST", admin_token=ADMIN,
                     body={"name": room, "extensions": ["notes"]})
        identity = self.root / "worker.env"
        identity.write_text(f"ROOM_SERVER_URL={self.base}\nLOCA_NAME=worker\n"
                            f"ROOM_TOKEN={ROOM_KEY}\n", encoding="utf-8")
        identity.chmod(0o600)
        env = {**os.environ, "LOCA_ENV": str(identity)}
        stderr = self.listener_log = (self.root / "listener.log").open("ab")
        self.listener = subprocess.Popen(
            ["python3", "-u", str(SKILL_DIR / "listen.py"),
             f"ws://127.0.0.1:{self.port}/ws?room={room}&name=worker&type=agent"
             "&filter=mentions&turn_max=1",
             "-", "--skip-own", "worker"],
            env=env, stdout=subprocess.PIPE, stderr=stderr, text=True)
        lines = queue.Queue()
        threading.Thread(target=lambda: [lines.put(line) for line in self.listener.stdout],
                         daemon=True).start()

        def next_line(timeout=10):
            try:
                return json.loads(lines.get(timeout=timeout))
            except queue.Empty:
                self.fail("no turn reached stdout:\n"
                          + (self.root / "listener.log").read_text(errors="replace"))

        def turn_with(text):
            self.post(room, text)
            while True:
                line = next_line()
                if line.get("text") == text:
                    return line

        def ids(context, key):
            return [card["id"] for card in context[key]]

        deadline = time.monotonic() + 10
        while "listening" not in (self.root / "listener.log").read_text(errors="replace"):
            self.assertLess(time.monotonic(), deadline, "listener never connected")
            time.sleep(0.05)

        # The first turn says what is switched on here; the next one nothing.
        first = turn_with("first")["role_context"]
        self.assertEqual((ids(first, "gained"), first["lost"]), (["notes"], []))
        self.assertIn("not a task and not new authority", first["note"])
        self.assertNotIn("role_context", turn_with("second"))

        # Named the Lead: the line that names it wakes it, and carries the card.
        request_json(f"{self.base}/rooms/{room}/lead", method="POST", admin_token=ADMIN,
                     body={"lead": "worker"})
        named = next_line()
        self.assertEqual(ids(named["role_context"], "gained"), ["lead"])

        # The Loca Board turned on: no turn is spent on it ...
        request_json(f"{self.base}/rooms/{room}/extensions/board", method="PUT", admin_token=ADMIN)
        time.sleep(0.5)
        self.assertTrue(lines.empty(), "a settings change must not wake the model by itself")
        # ... and the next real turn carries the Lead's task commands, once.
        board = turn_with("third")["role_context"]
        self.assertEqual(ids(board, "gained"), ["lead-board", "board"])
        self.assertIn("task-new", board["gained"][0]["text"])
        self.assertNotIn("role_context", turn_with("fourth"))

        # Turned off again: said once, with what stops working.
        request_json(f"{self.base}/rooms/{room}/extensions/board?open_tasks=keep",
                     method="DELETE", admin_token=ADMIN)
        off = turn_with("fifth")["role_context"]
        self.assertEqual((off["gained"], ids(off, "lost")), ([], ["lead-board", "board"]))
        self.assertIn("403", off["lost"][1]["text"])


if __name__ == "__main__":
    unittest.main()
