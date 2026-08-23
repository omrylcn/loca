import contextlib
import importlib.util
import io
import json
import os
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from unittest import mock


SCRIPT = Path(__file__).parents[1] / "scripts" / "audit.py"
SPEC = importlib.util.spec_from_file_location("loca_care_audit", SCRIPT)
audit = importlib.util.module_from_spec(SPEC)
assert SPEC and SPEC.loader
SPEC.loader.exec_module(audit)


class AuditHandler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path != "/care/residents" or self.headers.get("x-room-token") != "mb_test":
            self.send_response(403)
            self.end_headers()
            return
        body = json.dumps(
            [
                {
                    "name": "seated",
                    "kind": "agent",
                    "locas": ["work"],
                    "online": True,
                    "runtime": {"ready": True, "wake": "OK", "ack": "OK"},
                },
                {"name": "waiting", "kind": "agent", "locas": [], "online": True},
                {
                    "name": "stalled",
                    "kind": "agent",
                    "locas": ["work"],
                    "online": True,
                    "runtime": {
                        "ready": False,
                        "attention_id": "attention:stalled:7",
                        "stored": True,
                        "accepted": True,
                        "turn_completed": True,
                    },
                },
                {"name": "missing", "kind": "agent", "locas": ["work"], "online": False},
                {"name": "idle", "kind": "agent", "locas": [], "online": False},
                {"name": "operator", "kind": "user", "locas": ["work"], "online": True},
            ]
        ).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, _format, *_args):
        pass


class AuditTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.server = ThreadingHTTPServer(("127.0.0.1", 0), AuditHandler)
        cls.thread = threading.Thread(target=cls.server.serve_forever, daemon=True)
        cls.thread.start()
        cls.origin = f"http://127.0.0.1:{cls.server.server_port}"

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.thread.join(timeout=2)

    def identity_file(self, directory):
        path = Path(directory) / "loca-care.env"
        path.write_text(
            f"ROOM_SERVER_URL={self.origin}\n"
            "LOCA_NAME=loca-care\n"
            "LOCA_MEMBERSHIP=mb_test\n",
            encoding="utf-8",
        )
        path.chmod(0o600)
        return path

    def run_audit(self, args):
        stdout, stderr = io.StringIO(), io.StringIO()
        clean_env = {key: value for key, value in os.environ.items() if key != "LOCA_ENV"}
        with mock.patch.dict(os.environ, clean_env, clear=True):
            with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                result = audit.main(args)
        return result, stdout.getvalue(), stderr.getvalue()

    def test_json_audit_classifies_every_presence_state(self):
        with tempfile.TemporaryDirectory() as tmp:
            result, output, _ = self.run_audit(
                ["--env", str(self.identity_file(tmp)), "--format", "json"]
            )
        self.assertEqual(result, 0)
        payload = json.loads(output)
        self.assertEqual((payload["total"], payload["online"], payload["away"]), (6, 4, 2))
        states = {row["name"]: row["state"] for row in payload["residents"]}
        self.assertEqual(
            states,
            {
                "idle": "away/lobby",
                "missing": "away/invited",
                "operator": "online/loca",
                "seated": "online/loca",
                "stalled": "online/loca",
                "waiting": "online/lobby",
            },
        )
        runtime = {row["name"]: row["runtime_state"] for row in payload["residents"]}
        self.assertEqual(runtime["seated"], "healthy")
        self.assertEqual(runtime["waiting"], "unverified")
        self.assertEqual(runtime["stalled"], "degraded")
        self.assertEqual(runtime["operator"], "not-applicable")
        self.assertEqual(payload["runtime_degraded"], 1)
        self.assertEqual(payload["runtime_unverified"], 1)
        self.assertNotIn("mb_test", output)

    def test_problem_mode_is_machine_actionable(self):
        with tempfile.TemporaryDirectory() as tmp:
            result, output, _ = self.run_audit(
                [
                    "--env",
                    str(self.identity_file(tmp)),
                    "--only-problems",
                    "--fail-on-away",
                ]
            )
        self.assertEqual(result, 3)
        self.assertIn("missing\taway/invited", output)
        self.assertIn("idle\taway/lobby", output)
        self.assertIn("stalled\tonline/loca", output)
        self.assertIn("waiting\tonline/lobby", output)
        self.assertNotIn("seated", output)

    def test_runtime_failure_has_a_distinct_machine_exit(self):
        with tempfile.TemporaryDirectory() as tmp:
            result, output, _ = self.run_audit(
                [
                    "--env",
                    str(self.identity_file(tmp)),
                    "--only-problems",
                    "--fail-on-degraded",
                ]
            )
        self.assertEqual(result, 4)
        self.assertIn("stalled\tonline/loca\twork\truntime=degraded/relay-pending", output)
        self.assertIn("waiting\tonline/lobby\tlobby\truntime=unverified", output)
        self.assertNotIn("operator", output)

    def test_server_origin_mismatch_fails_closed(self):
        with tempfile.TemporaryDirectory() as tmp:
            result, _, error = self.run_audit(
                [
                    "--env",
                    str(self.identity_file(tmp)),
                    "--server",
                    "https://elsewhere.example",
                ]
            )
        self.assertEqual(result, 2)
        self.assertIn("credential server mismatch", error)


if __name__ == "__main__":
    unittest.main()
