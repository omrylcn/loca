"""Real HTTP + shipped send wrapper: never confuse process exit with acceptance."""
import json
import os
from pathlib import Path
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
        case = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_POST(self):
                data = json.loads(self.rfile.read(int(self.headers["content-length"])))
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
