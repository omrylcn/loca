"""Real HTTP + shipped send wrapper: never confuse process exit with acceptance."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from unittest.mock import patch

SKILL_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SKILL_DIR))
from codex_adapter_v2 import ConnectShRelay
from test_codex_adapter_v2 import AdapterFixture, delivery


class SendAcceptanceTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.status = 503
        self.body = {"error": "fault injection"}
        self.operations = []
        self.raw_bodies = []
        case = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_POST(self):
                raw = self.rfile.read(int(self.headers["content-length"]))
                case.raw_bodies.append(raw)
                data = json.loads(raw)
                case.operations.append(data.get("op_id"))
                body = json.dumps(case.body).encode()
                self.send_response(case.status)
                self.send_header("content-type", "application/json")
                self.send_header("content-length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.origin = f"http://127.0.0.1:{self.server.server_port}"
        env_file = self.root / "reviewer.env"
        env_file.write_text(f"ROOM_SERVER_URL={self.origin}\nLOCA_NAME=reviewer\nDAVET_sb_dev=dv_test\nLOCA_SESSION=st_test\n")
        env_file.chmod(0o600)
        self.env = {key: value for key, value in os.environ.items()
                    if not key.startswith(("LOCA_", "DAVET_", "ROOM_"))}
        self.env.update(HOME=str(self.root), LOCA_ENV=str(env_file))
        self.relay = ConnectShRelay(SKILL_DIR / "connect.sh")
        self.record = dict(server=self.origin, room="sb-dev", identity="reviewer",
                           target="-", text="reply", op_id="stable-test-op")

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=2)
        self.tmp.cleanup()

    def test_non_success_http_never_exits_zero_or_writes_receipt(self):
        for code in (400, 404, 413, 500, 502, 503):
            with self.subTest(status=code):
                self.status = code
                receipt = self.root / "receipt"
                result = subprocess.run(
                    [str(SKILL_DIR / "connect.sh"), "send", self.origin, "sb-dev", "reviewer", "-", "reply"],
                    env={**self.env, "LOCA_REPLY_RECEIPT": str(receipt)}, capture_output=True, text=True, timeout=15,
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(receipt.exists())

    def test_a_message_body_reaches_the_server_as_utf8_and_never_as_an_argument(self):
        # Windows curl.exe reads its arguments in the ANSI code page, so a
        # body passed with -d turned "çığ" into e7 fd f0 and the server
        # refused it (400 invalid unicode code point). The body must ride on
        # stdin; a shim in front of the real curl proves it is never argv.
        real_curl = shutil.which("curl")
        self.assertIsNotNone(real_curl)
        shim_dir = self.root / "bin"
        shim_dir.mkdir()
        argv_log = self.root / "curl-argv.jsonl"
        shim = shim_dir / "curl"
        shim.write_text(
            "#!/usr/bin/env python3\n"
            "import json, os, sys\n"
            f"with open({str(argv_log)!r}, 'a', encoding='utf-8') as log:\n"
            "    log.write(json.dumps(sys.argv[1:]) + '\\n')\n"
            f"os.execv({real_curl!r}, [{real_curl!r}] + sys.argv[1:])\n"
        )
        shim.chmod(0o755)
        env = {**self.env, "PATH": f"{shim_dir}{os.pathsep}{self.env.get('PATH', '')}"}
        text = "çığ ÖŞÜ — merhaba"
        self.status = 201
        self.body = {"id": 7, "room": "sb-dev", "sender": "reviewer"}
        for command in ("send", "announce"):
            with self.subTest(command=command):
                self.raw_bodies.clear()
                argv_log.write_text("")
                args = ["sb-dev", "reviewer", "-", text] if command == "send" else ["sb-dev", "reviewer", text]
                result = subprocess.run(
                    [str(SKILL_DIR / "connect.sh"), command, self.origin, *args],
                    env=env, capture_output=True, text=True, timeout=15,
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(len(self.raw_bodies), 1)
                self.assertIn(text.encode("utf-8"), self.raw_bodies[0])
                self.assertEqual(json.loads(self.raw_bodies[0])["text"], text)
                for line in argv_log.read_text(encoding="utf-8").splitlines():
                    argv = json.loads(line)
                    self.assertNotIn("-d", argv)
                    self.assertFalse(any("merhaba" in arg for arg in argv), argv)

    def test_an_unclear_operation_says_to_send_again_as_a_new_one(self):
        # 409: the server cannot prove an earlier use of this op_id was this
        # sender's, posts nothing, and asks for a new operation. Retrying the
        # same op_id can never post.
        self.status = 409
        self.body = "nothing was posted - send it again as a new operation"
        result = subprocess.run(
            [str(SKILL_DIR / "connect.sh"), "send", self.origin, "sb-dev", "reviewer", "-", "reply"],
            env={**self.env, "LOCA_OP_ID": "old-op"}, capture_output=True, text=True, timeout=15,
        )
        self.assertEqual(result.returncode, 8, result.stderr)
        self.assertIn("new LOCA_OP_ID", result.stderr)
        self.assertNotIn("retain the same operation id", result.stderr)
        self.assertEqual(self.operations, ["old-op"])

    def test_a_refusal_says_the_servers_own_sentence(self):
        # Every 403 used to read "chat mode forbids you right now", whether
        # the agent was muted, banned or the loca was closed. The server
        # answers a refusal in plain text.
        def plain(handler):
            handler.rfile.read(int(handler.headers["content-length"]))
            raw = self.body.encode()
            handler.send_response(self.status)
            handler.send_header("content-type", "text/plain; charset=utf-8")
            handler.send_header("content-length", str(len(raw)))
            handler.end_headers()
            handler.wfile.write(raw)

        self.server.RequestHandlerClass.do_POST = plain
        for sentence in ("you are muted by the admin",
                         "this room is closed (archived) — read-only",
                         "round-robin: it is operator's turn"):
            with self.subTest(sentence=sentence):
                self.status, self.body = 403, sentence
                result = subprocess.run(
                    [str(SKILL_DIR / "connect.sh"), "send", self.origin, "sb-dev", "reviewer", "-", "reply"],
                    env=self.env, capture_output=True, text=True, timeout=15,
                )
                self.assertEqual(result.returncode, 4, result.stderr)
                self.assertIn(f"REJECTED (403): {sentence}", result.stderr)
                self.assertNotIn("chat mode forbids", result.stderr)

    def test_a_renewal_does_not_guess_why_the_session_ended(self):
        # The first post meets 401 (the session ended: 12 hours, a newer
        # session, a restart...); send renews from the invite and posts once.
        answers = [(401, "session expired"), (201, {"id": 7, "room": "sb-dev", "sender": "reviewer"})]
        renewals = []

        def handle(handler):
            length = int(handler.headers["content-length"])
            handler.rfile.read(length)
            if handler.path == "/sessions":
                renewals.append(handler.headers.get("x-room-token"))
                status, body = 201, {"session_token": "st_renewed"}
            else:
                status, body = answers.pop(0)
            raw = (json.dumps(body) if isinstance(body, dict) else body).encode()
            handler.send_response(status)
            handler.send_header("content-length", str(len(raw)))
            handler.end_headers()
            handler.wfile.write(raw)

        self.server.RequestHandlerClass.do_POST = handle
        result = subprocess.run(
            [str(SKILL_DIR / "connect.sh"), "send", self.origin, "sb-dev", "reviewer", "-", "reply"],
            env=self.env, capture_output=True, text=True, timeout=15,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(renewals, ["dv_test"])
        self.assertIn("session renewed", result.stderr)
        self.assertNotIn("restart", result.stderr)
        self.assertEqual(answers, [])

    def test_relay_requires_server_message_identity_not_just_exit_zero(self):
        self.status = 201
        for response in ({}, {"id": 1, "room": "other", "sender": "reviewer"},
                         {"id": True, "room": "sb-dev", "sender": "reviewer"},
                         {"id": 1, "room": "sb-dev", "sender": "someone-else"}):
            with self.subTest(response=response), patch.dict(os.environ, self.env, clear=True):
                self.body = response
                with self.assertRaises(RuntimeError):
                    self.relay(self.record)

    def test_real_adapter_does_not_mark_failed_http_as_final_response(self):
        with patch.dict(os.environ, self.env, clear=True):
            fixture = AdapterFixture(self.root / "adapter", relay=self.relay)
            try:
                event = delivery(400)
                event["server"] = self.origin
                fixture.ingest(event)
                fixture.adapter.dispatch_one()
                fixture.adapter.handle_agent_item("thread-1", "turn-1", {
                    "id": "final-1", "type": "agentMessage", "phase": "final_answer", "text": "reply",
                })
                fixture.adapter.process_relays()
                self.assertEqual(fixture.store.progress_summary("reviewer")["reply_required_pending"], 1)
                with fixture.store.connect() as connection:
                    count = connection.execute("SELECT count(*) FROM relay_items WHERE accepted_at_ms IS NOT NULL").fetchone()[0]
                self.assertEqual(count, 0)
                first_op = self.operations[-1]
                self.status = 201
                self.body = {"id": 42, "room": "sb-dev", "sender": "reviewer", "sender_type": "agent", "text": "reply"}
                with fixture.store.connect() as connection:
                    connection.execute("UPDATE relay_items SET next_attempt_at_ms=0")
                fixture.adapter.process_relays()
                self.assertEqual(self.operations[-1], first_op)
                self.assertEqual(fixture.store.progress_summary("reviewer")["reply_required_pending"], 0)
            finally:
                fixture.adapter.close()

    def test_a_409_ends_the_relay_without_resending_or_reposting(self):
        # The server cannot prove an earlier use of this op_id was ours and
        # posted nothing; the same id will never post. The relay used to be
        # retried with it for ever.
        self.status = 409
        self.body = "this operation id was used before the upgrade under your name - send it again as a new operation"
        with patch.dict(os.environ, self.env, clear=True):
            fixture = AdapterFixture(self.root / "adapter", relay=self.relay)
            try:
                event = delivery(400)
                event["server"] = self.origin
                fixture.ingest(event)
                fixture.adapter.dispatch_one()
                fixture.adapter.handle_agent_item("thread-1", "turn-1", {
                    "id": "final-1", "type": "agentMessage", "phase": "final_answer", "text": "reply",
                })
                fixture.adapter.process_relays()
                self.assertEqual(len(self.operations), 1)
                with fixture.store.connect() as connection:
                    connection.execute("UPDATE relay_items SET next_attempt_at_ms=0")
                    row = connection.execute(
                        "SELECT status, last_error FROM relay_items WHERE relay_kind='final'").fetchone()
                self.assertEqual(row["status"], "refused")
                self.assertIn("NOT POSTED (409)", row["last_error"])
                # Not retried with the same id, not reposted with a new one.
                fixture.adapter.process_relays()
                fixture.adapter.process_relays()
                self.assertEqual(len(self.operations), 1)
                # The required reply stays open, so health shows it pending.
                self.assertEqual(fixture.store.progress_summary("reviewer")["reply_required_pending"], 1)
                self.assertIn("409", fixture.adapter.health["last_error"])
            finally:
                fixture.adapter.close()
