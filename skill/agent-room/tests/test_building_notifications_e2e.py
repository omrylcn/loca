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

    def test_offline_notifications_recover_wake_once_and_reach_the_durable_runtime(self):
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
                post = lambda title, body="Recipient-only announcement": request_json(f"{base}/notifications",
                    method="POST", admin_token="e2e-admin", body={"title":title, "body":body, "audience":{"all":True}})["id"]
                notices = [post(f"notice-{number}") for number in range(55)]
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

                lines = lambda: [json.loads(line) for line in stdout.read_text().splitlines()]
                records = lambda: [json.loads(line) for line in inbox.read_text().splitlines()] if inbox.exists() else []
                ids = lambda packet: [n["id"] for n in packet["items"]]

                with stdout.open("ab") as output, (root / "listener.log").open("ab") as errors:
                    listener = start()
                    self.until(lambda: recovered(notices[-1]), "Lobby notification recovery never reached its durable checkpoint")
                    # Recovered while away: one wake with the recent window.
                    self.until(lambda: len(lines()) == 1, "recovered notifications did not wake the Monitor")
                    self.assertEqual(lines()[0]["t"], "notification")
                    self.assertEqual(ids(lines()[0]["notification_context"]), notices[-4:])
                    self.assertTrue(lines()[0]["notification_context"]["more_available"])
                    self.until(lambda: len(records()) == 1, "the durable runtime got no notification delivery")
                    self.assertEqual(records()[0]["priority"], "notification")
                    self.assertEqual(records()[0]["delivery_id"], f"notification:{notices[-1]}")
                    self.assertEqual(ids(records()[0]["event"]["notification_context"]), notices[-4:])
                    request_json(f"{base}/rooms/notify-room/call", method="POST", admin_token="e2e-admin", body={"name":"notify-agent"})
                    self.until(lambda: any(m["name"] == "notify-agent" for m in request_json(f"{base}/rooms/notify-room/members", admin_token="e2e-admin")), "room listener did not join")
                    request_json(f"{base}/rooms/notify-room/messages", method="POST", admin_token="e2e-admin",
                        body={"sender":"operator","sender_type":"user","target":"notify-agent","text":"first real turn"})
                    self.until(lambda: len(records()) == 2, "real room attention was not delivered")
                    self.assertEqual(ids(records()[1]["event"]["notification_context"]), notices[-4:],
                        "the durable runtime still receives the window and filters by its own model context")
                    self.until(lambda: len(lines()) == 2, "the room turn did not reach Monitor stdout")
                    self.assertNotIn("notification_context", lines()[1],
                        "Monitor stdout repeated notices this model context already holds")
                    harness.DurableBackfillEndToEndTests.stop_process(listener); listener = None
                    offline = post("after restart", "offline gap")
                    listener = start()
                    self.until(lambda: recovered(offline), "offline gap was not recovered after restart")
                    # A restarted listener may be a new session: it gives the
                    # recent window once more, the offline notice included.
                    self.until(lambda: len(lines()) == 3, "the offline notice did not wake the restarted Monitor")
                    self.assertEqual(ids(lines()[2]["notification_context"]), notices[-3:] + [offline])
                    live = post("while online", "live Lobby frame")
                    self.until(lambda: len(lines()) == 4, "a live notification did not wake the Monitor")
                    self.assertEqual(ids(lines()[3]["notification_context"]), [live])
                    self.until(lambda: records()[-1]["delivery_id"] == f"notification:{live}",
                        "the durable runtime got no delivery for the live notice")
                    if os.name != "nt":
                        self.assertEqual(cache.stat().st_mode & 0o777, 0o600)
                    listed = subprocess.run(["bash", str(SKILL_DIR / "connect.sh"), "notifications", base, "notify-agent"],
                        cwd=REPO_ROOT, env=env, capture_output=True, text=True, timeout=10)
                    self.assertEqual(listed.returncode, 0, listed.stderr)
                    page = json.loads(listed.stdout)
                    self.assertEqual(page["notifications"][0]["id"], live)
                    self.assertTrue(all(not n["read"] for n in page["notifications"]),
                        "a wake is not a claim that the model read the notice")
                    second = request_json(f"{base}/rooms/notify-room/messages", method="POST", admin_token="e2e-admin",
                        body={"sender":"operator","sender_type":"user","target":"notify-agent","text":"second real turn"})
                    self.until(lambda: len(lines()) == 5, "notification IDs must not suppress a newer room message")
                    self.assertNotIn("notification_context", lines()[4])
                    latest = [r for r in records() if r["priority"] != "notification"][-1]
                    self.assertEqual(latest["priority"], "direct_user")
                    self.assertEqual(ids(latest["event"]["notification_context"])[-1], live)
                    self.until(lambda: json.loads(cursor.read_text())["notify-room"] == second["id"],
                        "chat and notification checkpoints must remain separate")
                    # A burst of six while idle: every one is given, oldest
                    # first, at most four a line; none waits for a room turn.
                    burst = [post(f"burst-{n}", "while idle") for n in range(6)]
                    given = lambda: [n for line in lines()[5:] for n in ids(line["notification_context"])]
                    self.until(lambda: given() == burst, "a burst was not given in full, in order, once")
                    self.assertTrue(all(len(line["notification_context"]["items"]) <= 4 for line in lines()[5:]))
                    self.assertTrue(all(line["t"] == "notification" for line in lines()[5:]))
            finally:
                harness.DurableBackfillEndToEndTests.stop_process(listener)
                harness.DurableBackfillEndToEndTests.stop_process(server, server_log)

    def test_monitor_does_not_report_a_gap_when_the_cache_rotates_past_given_notices(self):
        # Claude Monitor: once the listener's 128-row cache rotates past
        # notices this context was already given, no line may say that older
        # ones were not kept.
        server = listener = None
        server_log = None
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            port = free_port()
            base = f"http://127.0.0.1:{port}"
            try:
                server, server_log = harness.DurableBackfillEndToEndTests.start_server(self, port, root / "server.db", root / "server.log")
                member = request_json(f"{base}/members", method="POST", admin_token="e2e-admin",
                    body={"name":"rotate-agent", "kind":"agent"})
                env_file = root / "agent.env"
                env_file.write_text(f"ROOM_SERVER_URL={base}\nLOCA_NAME=rotate-agent\nLOCA_MEMBERSHIP={member['token']}\n")
                env_file.chmod(0o600)
                post = lambda n: request_json(f"{base}/notifications", method="POST", admin_token="e2e-admin",
                    body={"title":f"notice-{n}", "body":"Recipient-only announcement", "audience":{"all":True}})["id"]
                notices = [post(n) for n in range(128)]
                cache = root / "cursor.json.notifications.sqlite3"
                stdout = root / "stdout.jsonl"
                key = "rotate-agent:" + hashlib.sha256(member["token"].encode()).hexdigest()

                def recovered(message_id):
                    if not cache.exists():
                        return False
                    with sqlite3.connect(cache) as connection:
                        row = connection.execute("SELECT last_id FROM checkpoint WHERE server=? AND identity=?",
                            (base, key)).fetchone()
                    return row is not None and row[0] == message_id

                lines = lambda: [json.loads(line) for line in stdout.read_text().splitlines()]

                with stdout.open("ab") as output, (root / "listener.log").open("ab") as errors:
                    listener = subprocess.Popen(["python3", str(SKILL_DIR / "listen.py"),
                        f"ws://127.0.0.1:{port}/ws?room=&name=rotate-agent&type=agent", "-",
                        "--cursor", str(root / "cursor.json"), "--turn-log", str(root / "turns.jsonl")],
                        cwd=REPO_ROOT, env={**os.environ, "LOCA_ENV":str(env_file)}, stdout=output, stderr=errors)
                    self.until(lambda: recovered(notices[-1]) and len(lines()) == 1,
                        "recovered notices did not wake the Monitor")
                    self.assertEqual([n["id"] for n in lines()[0]["notification_context"]["items"]], notices[-4:])
                    # Four new notices a wake, each wake given exactly them,
                    # until the cache holds none of the first window.
                    for number in range(2, 34):
                        fresh = [post(n) for n in range(4)]
                        self.until(lambda: recovered(fresh[-1]), "notices were not recovered")
                        self.until(lambda: sum(len(line["notification_context"]["items"]) for line in lines()[1:])
                            == 4 * (number - 1), f"wake {number} did not give its notices")
                    given = [n["id"] for line in lines()[1:] for n in line["notification_context"]["items"]]
                    self.assertEqual(len(given), len(set(given)), "a notice was given twice")
                    self.assertEqual(len(given), 4 * 32)
                    for line in lines():
                        self.assertNotIn("older_unavailable", line["notification_context"])
                    with sqlite3.connect(cache) as connection:
                        oldest = connection.execute("SELECT MIN(id) FROM notices WHERE identity=?", (key,)).fetchone()[0]
                    self.assertGreater(oldest, notices[-1], "the cache rotated past the first window")
            finally:
                harness.DurableBackfillEndToEndTests.stop_process(listener)
                harness.DurableBackfillEndToEndTests.stop_process(server, server_log)


if __name__ == "__main__":
    unittest.main()
