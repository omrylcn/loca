"""`connect.sh notify-agents`: the caretaker tells every agent about Loca
itself. It reaches every agent and the Master's copy, nobody else; an
ordinary agent cannot send it; and a recipient's own fetch is what the
Master sees as "reached"."""
import json
import os
import subprocess
import tempfile
import time
import unittest
import urllib.request
from pathlib import Path

from test_listener_backfill_e2e import REPO_ROOT, SERVER_BIN, SKILL_DIR, free_port, request_json

ADMIN = "notify-agents-admin"


class NotifyAgentsEndToEndTests(unittest.TestCase):
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
            "LOCA_CARETAKERS": "loca-care",
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

    def member(self, name, kind="agent"):
        made = request_json(f"{self.base}/members", method="POST", admin_token=ADMIN,
                            body={"name": name, "kind": kind})
        return next(v for v in made.values() if isinstance(v, str) and v.startswith("mb_"))

    def identity(self, name, membership):
        env_file = self.root / f"{name}.env"
        env_file.write_text(f"ROOM_SERVER_URL={self.base}\nLOCA_NAME={name}\n"
                            f"LOCA_MEMBERSHIP={membership}\n", encoding="utf-8")
        env_file.chmod(0o600)
        return {**os.environ, "LOCA_ENV": str(env_file), "HOME": str(self.root)}

    def notify(self, env, name, title):
        return subprocess.run(
            ["bash", str(SKILL_DIR / "connect.sh"), "notify-agents", self.base, name, title,
             "Update with ./install.sh --upgrade-only, then restart your runtime."],
            env=env, capture_output=True, text=True, timeout=30)

    def test_the_caretaker_tells_every_agent_and_the_master_sees_whom_it_reached(self):
        care = self.identity("loca-care", self.member("loca-care"))
        bot_key = self.member("bot")
        bot = self.identity("bot", bot_key)
        self.member("robo")
        self.member("ayse", "user")

        refused = self.notify(bot, "bot", "an agent may not")
        self.assertEqual(refused.returncode, 4, refused.stderr)
        self.assertIn("NOT SENT (HTTP 403)", refused.stderr)

        sent = self.notify(care, "loca-care", "Loca 0.24.2 is out: update your kit")
        self.assertEqual(sent.returncode, 0, sent.stderr)
        self.assertRegex(sent.stdout.strip(), r"^notification #\d+ sent to \d+ \(\d+ agents, and the Master\)$")
        number = int(sent.stdout.split("#")[1].split()[0])

        # The bot's own fetch of its list is what "reached" records.
        request_json(f"{self.base}/notifications", token=bot_key)
        sent_list = request_json(f"{self.base}/notifications/sent", admin_token=ADMIN)
        entry = next(n for n in sent_list if n["id"] == number)
        self.assertEqual(entry["from"], "loca-care")
        self.assertEqual(entry["audience"], {"agents": True})
        who = request_json(f"{self.base}/notifications/{number}/recipients", admin_token=ADMIN)
        rows = {r["name"]: r for r in who["recipients"]}
        self.assertIn("bot", rows)
        self.assertIn("robo", rows)
        self.assertNotIn("ayse", rows, "a person who is not the Master is not told")
        self.assertIsNotNone(rows["bot"]["delivered_at"])
        self.assertIsNone(rows["robo"]["delivered_at"])


if __name__ == "__main__":
    unittest.main()
