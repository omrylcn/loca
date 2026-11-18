"""A Building notification wakes the agent it was sent to, once.

PRINCIPLES.md, "Building notifications": a notification reaches each agent
it was sent to as a turn of its own, as context; it is still no task, no
authority and no reason to answer in a loca. Claude Monitor gets one stdout
line; a durable runtime gets one delivery with priority ``notification``.
"""
import hashlib
import json
import os
import sqlite3
import subprocess
import tempfile
import time
import unittest
from pathlib import Path

import test_listener_backfill_e2e as harness
from test_listener_backfill_e2e import SKILL_DIR, REPO_ROOT, free_port, request_json

SKILL_DIR = Path(os.environ.get("LOCA_TEST_SKILL_DIR", str(SKILL_DIR)))


class NotificationWakeEndToEndTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        harness.DurableBackfillEndToEndTests.setUpClass()

    def until(self, check, message):
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            try:
                if check():
                    return
            except (OSError, sqlite3.Error, ValueError):
                pass
            time.sleep(0.05)
        self.fail(message)

    def test_a_notification_wakes_a_lobby_agent_once_and_is_not_repeated(self):
        server = listener = None
        server_log = None
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            port = free_port()
            base = f"http://127.0.0.1:{port}"
            try:
                server, server_log = harness.DurableBackfillEndToEndTests.start_server(
                    self, port, root / "server.db", root / "server.log")
                member = request_json(f"{base}/members", method="POST", admin_token="e2e-admin",
                    body={"name": "wake-agent", "kind": "agent"})
                request_json(f"{base}/rooms", method="POST", admin_token="e2e-admin", body={"name": "wake-room"})
                env_file = root / "agent.env"
                env_file.write_text(f"ROOM_SERVER_URL={base}\nLOCA_NAME=wake-agent\nLOCA_MEMBERSHIP={member['token']}\n")
                env_file.chmod(0o600)
                cache = root / "cursor.json.notifications.sqlite3"
                stdout = root / "stdout.jsonl"
                inbox = root / "turns.jsonl"
                key = "wake-agent:" + hashlib.sha256(member["token"].encode()).hexdigest()

                def post(title):
                    return request_json(f"{base}/notifications", method="POST", admin_token="e2e-admin",
                        body={"title": title, "body": "for every agent", "audience": {"all": True}})["id"]

                def recovered(message_id):
                    if not cache.exists():
                        return False
                    with sqlite3.connect(cache) as connection:
                        row = connection.execute("SELECT last_id FROM checkpoint WHERE server=? AND identity=?",
                            (base, key)).fetchone()
                    return row is not None and row[0] == message_id

                lines = lambda: [json.loads(line) for line in stdout.read_text().splitlines()]
                records = lambda: [json.loads(line) for line in inbox.read_text().splitlines()] if inbox.exists() else []
                shown = lambda: [n["id"] for line in lines() for n in (line.get("notification_context") or {}).get("items", [])]

                with stdout.open("ab") as output, (root / "listener.log").open("ab") as errors:
                    listener = subprocess.Popen(["python3", str(SKILL_DIR / "listen.py"),
                        f"ws://127.0.0.1:{port}/ws?room=&name=wake-agent&type=agent", "-",
                        "--cursor", str(root / "cursor.json"), "--turn-log", str(inbox)],
                        cwd=REPO_ROOT, env={**os.environ, "LOCA_ENV": str(env_file)}, stdout=output, stderr=errors)
                    # In the Lobby, seated nowhere: nothing else could carry it.
                    first = post("kit update")
                    self.until(lambda: recovered(first), "the notification was not recovered")
                    self.until(lambda: len(lines()) == 1, "a Building notification did not wake the Monitor")
                    wake = lines()[0]
                    self.assertEqual(wake["t"], "notification")
                    self.assertEqual([n["id"] for n in wake["notification_context"]["items"]], [first])
                    self.assertEqual(wake["notification_context"]["items"][0]["title"], "kit update")
                    self.until(lambda: len(records()) == 1, "the durable runtime got no notification delivery")
                    delivery = records()[0]
                    self.assertEqual(delivery["priority"], "notification")
                    self.assertEqual(delivery["room"], "")
                    self.assertEqual(delivery["delivery_id"], f"notification:{first}")
                    self.assertEqual([n["id"] for n in delivery["event"]["notification_context"]["items"]], [first])

                    # A second one wakes again, with only itself.
                    second = post("second")
                    self.until(lambda: len(lines()) == 2, "the second notification did not wake the Monitor")
                    self.assertEqual([n["id"] for n in lines()[1]["notification_context"]["items"]], [second])

                    # A room turn after them does not repeat what was shown.
                    request_json(f"{base}/rooms/wake-room/call", method="POST", admin_token="e2e-admin", body={"name": "wake-agent"})
                    self.until(lambda: any(m["name"] == "wake-agent" for m in request_json(
                        f"{base}/rooms/wake-room/members", admin_token="e2e-admin")), "room listener did not join")
                    request_json(f"{base}/rooms/wake-room/messages", method="POST", admin_token="e2e-admin",
                        body={"sender": "operator", "sender_type": "user", "target": "wake-agent", "text": "hello"})
                    self.until(lambda: len(lines()) == 3, "the room turn did not reach Monitor stdout")
                    self.assertNotIn("notification_context", lines()[2])
                    self.assertEqual(shown(), [first, second], "each notice is shown exactly once")

                    # Delivering is not reading: the server still says unread.
                    listed = subprocess.run(["bash", str(SKILL_DIR / "connect.sh"), "notifications", base, "wake-agent"],
                        cwd=REPO_ROOT, env={**os.environ, "LOCA_ENV": str(env_file)},
                        capture_output=True, text=True, timeout=10)
                    self.assertEqual(listed.returncode, 0, listed.stderr)
                    self.assertTrue(all(not n["read"] for n in json.loads(listed.stdout)["notifications"]))
            finally:
                harness.DurableBackfillEndToEndTests.stop_process(listener)
                harness.DurableBackfillEndToEndTests.stop_process(server, server_log)


if __name__ == "__main__":
    unittest.main()
