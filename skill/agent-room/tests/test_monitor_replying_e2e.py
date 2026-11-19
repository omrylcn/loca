"""A Claude Monitor agent shows "replying" while it owes an answer.

A Monitor session has no adapter that knows when its model turn ends, so its
listener says "replying" (the room's ephemeral ``typing`` frame) from the
moment it hands the session a message that asks it for an answer, repeats it
while the room would otherwise forget it, and stops once the agent's own
message reaches the room. A room message that does not ask it says nothing.
"""
import json
import os
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path

import test_listener_backfill_e2e as harness
from test_listener_backfill_e2e import SKILL_DIR, free_port, request_json

SKILL_DIR = Path(os.environ.get("LOCA_TEST_SKILL_DIR", str(SKILL_DIR)))
sys.path.insert(0, str(SKILL_DIR))

import listen  # noqa: E402


class ReplyingPulseTests(unittest.TestCase):
    def test_on_is_repeated_until_stopped_or_too_old(self):
        pulse = listen.ReplyingPulse(refresh=3, limit=90)
        self.assertEqual(pulse.tick(0), [])
        pulse.start("r", 0)
        self.assertEqual(pulse.tick(0), [("r", True)])
        self.assertEqual(pulse.tick(1), [])
        self.assertEqual(pulse.tick(3), [("r", True)])
        pulse.stop("r")
        self.assertEqual(pulse.tick(4), [("r", False)])
        self.assertEqual(pulse.tick(10), [])
        pulse.start("r", 20)
        self.assertEqual(pulse.tick(20), [("r", True)])
        self.assertEqual(pulse.tick(111), [("r", False)])


class MonitorReplyingEndToEndTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        harness.DurableBackfillEndToEndTests.setUpClass()

    def test_a_direct_question_shows_replying_until_the_answer_arrives(self):
        server = listener = watcher = None
        server_log = None
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            port = free_port()
            base = f"http://127.0.0.1:{port}"
            try:
                server, server_log = harness.DurableBackfillEndToEndTests.start_server(
                    self, port, root / "server.db", root / "server.log")
                request_json(f"{base}/rooms", method="POST", admin_token="e2e-admin",
                             body={"name": "reply-room"})
                identity = root / "agent.env"
                identity.write_text(f"ROOM_SERVER_URL={base}\nLOCA_NAME=reply-agent\n"
                                    "ROOM_TOKEN=e2e-room\n", encoding="utf-8")
                identity.chmod(0o600)

                watcher, leftover = listen.connect(
                    f"ws://127.0.0.1:{port}/ws?room=reply-room&name=watcher&type=user",
                    ["loca.v1", "loca.room.e2e-room"])
                typing = []

                def read():
                    try:
                        for raw in listen.frames(watcher, leftover):
                            frame = json.loads(raw)
                            if frame.get("t") == "typing" and frame.get("name") == "reply-agent":
                                typing.append((time.monotonic(), frame["on"]))
                    except Exception:
                        return

                threading.Thread(target=read, daemon=True).start()

                env = dict(os.environ, LOCA_ENV=str(identity))
                stdout = root / "stdout.jsonl"
                with stdout.open("ab") as output, (root / "listener.log").open("ab") as errors:
                    listener = subprocess.Popen(["python3", "-u", str(SKILL_DIR / "listen.py"),
                        f"ws://127.0.0.1:{port}/ws?room=reply-room&name=reply-agent&type=agent"
                        "&filter=mentions&turn_idle_ms=50",
                        "-", "--skip-own", "reply-agent"],
                        env=env, stdout=output, stderr=errors)
                self.until(lambda: any(m["name"] == "reply-agent" for m in request_json(
                    f"{base}/rooms/reply-room/members", token="e2e-room")), "agent never seated")

                def say(sender, kind, text, target=None):
                    body = {"sender": sender, "sender_type": kind, "text": text}
                    if target:
                        body["target"] = target
                    request_json(f"{base}/rooms/reply-room/messages", method="POST",
                                 token="e2e-room", body=body)

                say("someone", "user", "a room note that asks nobody")
                time.sleep(1.5)
                self.assertEqual(typing, [], "an unaddressed message must not show replying")

                say("operator", "user", "@reply-agent can you check this?", target="reply-agent")
                self.until(lambda: [on for _, on in typing] == [True], "no replying signal")
                self.until(lambda: [on for _, on in typing] == [True, True],
                           "replying was not repeated while the answer was owed")
                first, second = typing[0][0], typing[1][0]
                self.assertLess(second - first, 4.0, "repeated before the room forgets it")

                say("reply-agent", "agent", "checked: all good")
                self.until(lambda: typing and typing[-1][1] is False,
                           "the agent's own answer did not end replying")
                ended = len(typing)
                time.sleep(4)
                self.assertEqual(len(typing), ended, "nothing more after the answer")
            finally:
                if watcher is not None:
                    try:
                        watcher.close()
                    except OSError:
                        pass
                harness.DurableBackfillEndToEndTests.stop_process(listener)
                harness.DurableBackfillEndToEndTests.stop_process(server, server_log)

    def until(self, check, message, seconds=10):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            try:
                if check():
                    return
            except (OSError, ValueError):
                pass
            time.sleep(0.05)
        self.fail(message)


if __name__ == "__main__":
    unittest.main()
