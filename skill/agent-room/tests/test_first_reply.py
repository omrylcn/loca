"""`connect.sh first-reply`: connecting ends when a call gets an answer.

The kit says hello in the Lobby room, then waits until someone other than
the agent names it and the agent posts after that call. A green listener
alone is not the proof; the answer is.
"""
import json
import os
import subprocess
import sys
import tempfile
import time
import unittest
import urllib.request
from pathlib import Path

SKILL_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SKILL_DIR))
sys.path.insert(0, str(Path(__file__).resolve().parent))

import first_reply  # noqa: E402

HELLO = 10


def msg(id_, sender, text, ts=None, **extra):
    row = {"id": id_, "sender": sender, "text": text, "ts": ts if ts is not None else id_ * 1000}
    row.update(extra)
    return row


class JudgeTests(unittest.TestCase):
    def test_nobody_called_yet(self):
        code, call, answer = first_reply.judge([msg(11, "deniz", "morning all")], "nova", HELLO)
        self.assertEqual(code, first_reply.WAITING)
        self.assertIsNone(call)

    def test_a_call_with_no_answer(self):
        code, call, _ = first_reply.judge([msg(11, "deniz", "@nova are you there?")], "nova", HELLO)
        self.assertEqual(code, first_reply.CALLED)
        self.assertEqual(call["id"], 11)

    def test_the_agents_answer_after_the_call_proves_it(self):
        rows = [
            msg(12, "nova", "hi deniz", ts=20_500),
            msg(11, "deniz", "@Nova. hello", ts=8_000),
        ]
        code, call, answer = first_reply.judge(rows, "nova", HELLO)
        self.assertEqual(code, first_reply.PROVEN)
        self.assertEqual((call["id"], answer["id"]), (11, 12))
        line = first_reply.line(code, call, answer, "nova")
        self.assertIn("deniz called @nova", line)
        self.assertIn("in 12 s", line)

    def test_a_target_or_a_reply_to_the_hello_is_a_call_too(self):
        self.assertTrue(first_reply.calls(msg(11, "deniz", "hi", target="NOVA"), "nova"))
        self.assertTrue(first_reply.calls(msg(11, "deniz", "hi", reply_to=HELLO), "nova", HELLO))
        self.assertTrue(first_reply.calls(msg(11, "deniz", "hi", reply_to_sender="nova"), "nova"))
        self.assertFalse(first_reply.calls(msg(11, "deniz", "@novak hi"), "nova", HELLO))
        self.assertFalse(first_reply.calls(msg(11, "deniz", "@all hi", target="all"), "nova", HELLO))

    def test_the_agent_naming_itself_is_not_a_call(self):
        rows = [msg(11, "nova", "@nova testing"), msg(12, "nova", "ok")]
        self.assertEqual(first_reply.judge(rows, "nova", HELLO)[0], first_reply.WAITING)

    def test_an_agent_post_before_the_call_does_not_count(self):
        rows = [msg(11, "nova", "still here"), msg(12, "deniz", "@nova hi")]
        self.assertEqual(first_reply.judge(rows, "nova", HELLO)[0], first_reply.CALLED)

    def test_the_command_line_reads_stdin_and_exits_with_the_verdict(self):
        rows = [msg(11, "deniz", "@nova hi"), msg(12, "nova", "hello")]
        done = subprocess.run(
            [sys.executable, str(SKILL_DIR / "first_reply.py"), "nova", str(HELLO)],
            input=json.dumps(rows), capture_output=True, text=True, timeout=10)
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertTrue(done.stdout.startswith("PROVEN: deniz called @nova"))
        broken = subprocess.run(
            [sys.executable, str(SKILL_DIR / "first_reply.py"), "nova"],
            input="not json", capture_output=True, text=True, timeout=10)
        self.assertEqual(broken.returncode, 2)


class FirstReplyEndToEndTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        import test_listener_backfill_e2e as harness
        cls.harness = harness
        harness.DurableBackfillEndToEndTests.setUpClass()

    def test_hello_then_a_call_then_the_answer_ends_connecting(self):
        harness = self.harness
        request_json = harness.request_json
        server = server_log = waiter = None
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            port = harness.free_port()
            base = f"http://127.0.0.1:{port}"
            try:
                server, server_log = harness.DurableBackfillEndToEndTests.start_server(
                    self, port, root / "server.db", root / "server.log",
                    extra_env={"REQUIRE_SESSIONS": "1", "REQUIRE_INVITE": "1"})
                agent = request_json(f"{base}/members", method="POST", admin_token="e2e-admin",
                                     body={"name": "nova", "kind": "agent"})
                caller = request_json(f"{base}/members", method="POST", admin_token="e2e-admin",
                                      body={"name": "deniz", "kind": "agent"})
                env_file = root / "nova.env"
                env_file.write_text(f"ROOM_SERVER_URL={base}\nLOCA_NAME=nova\n"
                                    f"LOCA_MEMBERSHIP={agent['token']}\n", encoding="utf-8")
                env_file.chmod(0o600)
                env = {**os.environ, "LOCA_ENV": str(env_file), "HOME": str(root)}
                connect = ["bash", str(SKILL_DIR / "connect.sh")]

                # Nobody there: it says hello, waits, and says how to resume.
                alone = subprocess.run(
                    connect + ["first-reply", base, "nova", "--wait", "0"],
                    env=env, capture_output=True, text=True, timeout=60)
                self.assertEqual(alone.returncode, 2, alone.stdout + alone.stderr)
                self.assertIn("WAITING: nobody has written @nova", alone.stdout)
                hello = [m for m in request_json(f"{base}/rooms/lobby/messages",
                                                 admin_token="e2e-admin")
                         if m["sender"] == "nova"]
                self.assertEqual(len(hello), 1)
                self.assertIn("Write @nova here", hello[0]["text"])
                hello_id = hello[0]["id"]
                self.assertIn(f"--after {hello_id}", alone.stdout)

                # Resumed: someone calls, the agent answers, it is proven.
                waiter = subprocess.Popen(
                    connect + ["first-reply", base, "nova", "--after", str(hello_id),
                               "--wait", "40"],
                    env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                session = urllib.request.Request(
                    f"{base}/sessions", method="POST",
                    data=json.dumps({"name": "deniz", "kind": "agent"}).encode())
                session.add_header("content-type", "application/json")
                session.add_header("x-room-token", caller["token"])
                with urllib.request.urlopen(session, timeout=10) as response:
                    caller_session = json.loads(response.read())["session_token"]
                post = urllib.request.Request(
                    f"{base}/rooms/lobby/messages", method="POST",
                    data=json.dumps({"sender": "deniz", "sender_type": "agent",
                                     "text": "@nova welcome, are you awake?"}).encode())
                post.add_header("content-type", "application/json")
                post.add_header("x-session-token", caller_session)
                with urllib.request.urlopen(post, timeout=10):
                    pass
                time.sleep(1)
                answered = subprocess.run(
                    connect + ["send", base, "lobby", "nova", "deniz", "awake and listening"],
                    env=env, capture_output=True, text=True, timeout=30)
                self.assertEqual(answered.returncode, 0, answered.stderr)
                out, err = waiter.communicate(timeout=40)
                self.assertEqual(waiter.returncode, 0, out + err)
                self.assertIn("PROVEN: deniz called @nova", out)
                self.assertIn("RESULT: CONNECTED", out)
                # Resuming said no second hello.
                hellos = [m for m in request_json(f"{base}/rooms/lobby/messages",
                                                  admin_token="e2e-admin")
                          if m["sender"] == "nova" and "Write @nova" in m["text"]]
                self.assertEqual(len(hellos), 1)
            finally:
                if waiter is not None and waiter.poll() is None:
                    waiter.kill()
                harness.DurableBackfillEndToEndTests.stop_process(server, server_log)


if __name__ == "__main__":
    unittest.main()
