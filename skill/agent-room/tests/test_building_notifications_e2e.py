import json
import hashlib
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


class BuildingNotificationEndToEndTests(unittest.TestCase):
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

    def test_offline_notifications_recover_without_a_turn_and_reach_next_room_context(self):
        server = listener = None
        server_log = None
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            port = free_port()
            base = f"http://127.0.0.1:{port}"
            try:
                server, server_log = harness.DurableBackfillEndToEndTests.start_server(self, port, root / "server.db", root / "server.log")
                member = request_json(f"{base}/members", method="POST", admin_token="e2e-admin",
                    body={"name":"notify-agent", "kind":"agent"})
                request_json(f"{base}/rooms", method="POST", admin_token="e2e-admin", body={"name":"notify-room"})
                env_file = root / "agent.env"
                env_file.write_text(f"ROOM_SERVER_URL={base}\nLOCA_NAME=notify-agent\nLOCA_MEMBERSHIP={member['token']}\nLOCA_SESSION=st_stale_fixture\n")
                env_file.chmod(0o600)
                notices = []
                for number in range(55):
                    notice = request_json(f"{base}/notifications", method="POST", admin_token="e2e-admin",
                        body={"title":f"notice-{number}", "body":"Recipient-only announcement", "audience":{"all":True}})
                    notices.append(notice["id"])
                cursor = root / "cursor.json"
                cache = root / "cursor.json.notifications.sqlite3"
                inbox = root / "turns.jsonl"
                stdout = root / "stdout.jsonl"
                env = {**os.environ, "LOCA_ENV":str(env_file)}

                def start():
                    return subprocess.Popen(["python3", str(SKILL_DIR / "listen.py"),
                        f"ws://127.0.0.1:{port}/ws?room=&name=notify-agent&type=agent", "-",
                        "--cursor", str(cursor), "--turn-log", str(inbox)],
                        cwd=REPO_ROOT, env=env, stdout=output, stderr=errors)

                def recovered(message_id):
                    if not cache.exists():
                        return False
                    with sqlite3.connect(cache) as connection:
                        row = connection.execute("SELECT last_id FROM checkpoint WHERE server=? AND identity=?",
                            (base, "notify-agent:" + hashlib.sha256(member["token"].encode()).hexdigest())).fetchone()
                    return row is not None and row[0] == message_id

                with stdout.open("ab") as output, (root / "listener.log").open("ab") as errors:
                    listener = start()
                    self.until(lambda: recovered(notices[-1]), "Lobby notification recovery never reached its durable checkpoint")
                    self.assertEqual(stdout.read_bytes(), b"", "a Building announcement must not create a model wake")
                    self.assertEqual(inbox.read_bytes(), b"", "announcements alone are not room attentions")
                    request_json(f"{base}/rooms/notify-room/call", method="POST", admin_token="e2e-admin", body={"name":"notify-agent"})
                    self.until(lambda: any(m["name"] == "notify-agent" for m in request_json(f"{base}/rooms/notify-room/members", admin_token="e2e-admin")), "room listener did not join")
                    request_json(f"{base}/rooms/notify-room/messages", method="POST", admin_token="e2e-admin",
                        body={"sender":"operator","sender_type":"user","target":"notify-agent","text":"first real turn"})
                    self.until(lambda: len(inbox.read_text().splitlines()) == 1, "real room attention was not delivered")
                    record = json.loads(inbox.read_text().splitlines()[0])
                    context = record["event"]["notification_context"]
                    self.assertEqual([n["id"] for n in context["items"]], notices[-4:])
                    self.assertTrue(context["more_available"])
                    self.assertEqual(json.loads(stdout.read_text().splitlines()[0])["notification_context"], context,
                        "Claude Monitor and the durable runtime must receive the same context")
                    harness.DurableBackfillEndToEndTests.stop_process(listener); listener = None
                    notice = request_json(f"{base}/notifications", method="POST", admin_token="e2e-admin",
                        body={"title":"after restart", "body":"offline gap", "audience":{"all":True}})
                    listener = start()
                    self.until(lambda: recovered(notice["id"]), "offline gap was not recovered after restart")
                    live = request_json(f"{base}/notifications", method="POST", admin_token="e2e-admin",
                        body={"title":"while online", "body":"live Lobby frame", "audience":{"all":True}})
                    self.until(lambda: recovered(live["id"]), "live notification was not recovered")
                    time.sleep(0.2)
                    self.assertEqual(len(inbox.read_text().splitlines()), 1, "recovery must not replay a room turn")
                    if os.name != "nt":
                        self.assertEqual(cache.stat().st_mode & 0o777, 0o600)
                    listed = subprocess.run(["bash", str(SKILL_DIR / "connect.sh"), "notifications", base, "notify-agent"],
                        cwd=REPO_ROOT, env=env, capture_output=True, text=True, timeout=10)
                    self.assertEqual(listed.returncode, 0, listed.stderr)
                    page = json.loads(listed.stdout)
                    self.assertEqual(page["notifications"][0]["id"], live["id"])
                    self.assertTrue(all(not n["read"] for n in page["notifications"]),
                        "delivery/context injection must not claim that the model read the notice")
                    second = request_json(f"{base}/rooms/notify-room/messages", method="POST", admin_token="e2e-admin",
                        body={"sender":"operator","sender_type":"user","target":"notify-agent","text":"second real turn"})
                    self.until(lambda: len(inbox.read_text().splitlines()) == 2, "notification IDs must not suppress a newer room message")
                    latest = json.loads(inbox.read_text().splitlines()[-1])
                    self.assertEqual(latest["event"]["notification_context"]["items"][-1]["id"], live["id"])
                    self.assertEqual(latest["priority"], "direct_user")
                    self.until(lambda: json.loads(cursor.read_text())["notify-room"] == second["id"],
                        "chat and notification checkpoints must remain separate")
            finally:
                harness.DurableBackfillEndToEndTests.stop_process(listener)
                harness.DurableBackfillEndToEndTests.stop_process(server, server_log)


if __name__ == "__main__":
    unittest.main()
