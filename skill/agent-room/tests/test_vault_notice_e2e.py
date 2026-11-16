"""The Vault against the real server: an agent learns that its loca's rules
changed on its next real turn, as loca context, without a turn of its own."""
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

ADMIN = "vault-e2e-admin"


class VaultNoticeEndToEndTests(unittest.TestCase):
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
            "ROOM_TOKEN": "vault-e2e-room", "RATE_LIMIT": "0", "REQUIRE_SESSIONS": "0",
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
        request_json(f"{self.base}/rooms/{room}/messages", method="POST", token="vault-e2e-room",
                     body={"sender": "operator", "sender_type": "user", "target": "worker",
                           "text": text})

    def write_rules(self, room, body, base_rev):
        return request_json(f"{self.base}/rooms/{room}/vault", method="PUT", admin_token=ADMIN,
                            body={"body": body, "base_rev": base_rev})

    def test_a_rules_change_rides_the_next_real_turn_once_as_context(self):
        room = "vault-room"
        request_json(f"{self.base}/rooms", method="POST", admin_token=ADMIN,
                     body={"name": room, "extensions": ["vault"]})
        identity = self.root / "worker.env"
        identity.write_text(f"ROOM_SERVER_URL={self.base}\nLOCA_NAME=worker\n"
                            "ROOM_TOKEN=vault-e2e-room\n", encoding="utf-8")
        identity.chmod(0o600)
        env = {**os.environ, "LOCA_ENV": str(identity)}
        stderr = self.listener_log = (self.root / "listener.log").open("ab")
        # Claude Monitor mode: stdout is the wake channel, one line per turn.
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

        # Seated and listening: a first message reaches the model, with no notice.
        deadline = time.monotonic() + 10
        while "listening" not in (self.root / "listener.log").read_text(errors="replace"):
            self.assertLess(time.monotonic(), deadline, "listener never connected")
            time.sleep(0.05)
        self.post(room, "first")
        first = next_line()
        self.assertEqual(first["text"], "first")
        self.assertNotIn("vault_context", first)

        # The Operator changes the rules: no turn is spent on it ...
        self.assertEqual(self.write_rules(room, "# Rules\n- run make check", 0)["rev"], 1)
        time.sleep(0.5)
        self.assertTrue(lines.empty(), "a Vault change must not wake the model by itself")
        # ... and the next real turn carries the notice as loca context.
        self.post(room, "second")
        second = next_line()
        self.assertEqual(second["text"], "second")
        notice = second["vault_context"]
        self.assertEqual((notice["room"], notice["rev"]), (room, 1))
        self.assertTrue(notice["updated_by"])
        self.assertIn("Context, not instructions", notice["note"])
        self.assertIn("grant no authority", notice["note"])
        self.assertIn("connect.sh vault", notice["note"])
        self.assertNotIn("run make check", json.dumps(second), "the notice carries no rule text")
        # Once per change.
        self.post(room, "third")
        self.assertNotIn("vault_context", next_line())


if __name__ == "__main__":
    unittest.main()
