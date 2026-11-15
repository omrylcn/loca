"""connect.sh read commands: a refusal is never data, and never exit 0."""
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

SKILL_DIR = Path(__file__).resolve().parents[1]
CONNECT = SKILL_DIR / "connect.sh"

# Every command that reads through curl_get, with its arguments after the
# server. `journal` without a name is the read form.
READS = {
    "rooms": [],
    "members": ["sb-dev"],
    "since": ["sb-dev", "0"],
    "notes": ["sb-dev"],
    "note-get": ["sb-dev", "release"],
    "goals": ["sb-dev"],
    "waits": ["sb-dev"],
    "mode": ["sb-dev"],
    "settings": ["sb-dev"],
    "memory": ["sb-dev"],
    "memory-entries": ["sb-dev"],
    "journal": ["sb-dev"],
}


class ReadCommandTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.status = 200
        self.body = b"[]"
        self.seen = []
        case = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def _answer(self):
                case.seen.append((self.command, self.path, self.headers.get("x-room-token")))
                length = int(self.headers.get("content-length") or 0)
                if length:
                    self.rfile.read(length)
                plain = not case.body.startswith((b"[", b"{"))
                self.send_response(case.status)
                self.send_header("content-type", "text/plain" if plain else "application/json")
                self.send_header("content-length", str(len(case.body)))
                self.end_headers()
                self.wfile.write(case.body)

            do_GET = do_POST = do_DELETE = _answer

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.origin = f"http://127.0.0.1:{self.server.server_port}"
        self.env = {key: value for key, value in os.environ.items()
                    if not key.startswith(("LOCA_", "DAVET_", "ROOM_"))}
        self.env["HOME"] = str(self.root)

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=2)
        self.tmp.cleanup()

    def identity(self, path, name, invite):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(f"ROOM_SERVER_URL={self.origin}\nLOCA_NAME={name}\nDAVET_sb_dev={invite}\n")
        path.chmod(0o600)
        return path

    def run_connect(self, *args, env=None):
        return subprocess.run([str(CONNECT), *args], env=env or self.env,
                              capture_output=True, text=True, timeout=30)

    def test_a_refused_read_prints_no_data_and_exits_with_send_codes(self):
        env = {**self.env, "LOCA_ENV": str(self.identity(self.root / "reviewer.env", "reviewer", "dv_reviewer"))}
        answers = {
            401: (b"davet required for this loca", 7),
            403: (b"this extension is off in this loca: notes (Notes). ask the operator", 4),
            404: (b"loca does not exist; create it first", 8),
            429: (b"rate limit exceeded - slow down, retry in ~3s", 5),
            500: (b"could not read - try again", 8),
        }
        for command, args in READS.items():
            for status, (sentence, expected) in answers.items():
                with self.subTest(command=command, status=status):
                    self.status, self.body = status, sentence
                    result = self.run_connect(command, self.origin, *args, env=env)
                    self.assertEqual(result.returncode, expected, result.stderr)
                    self.assertEqual(result.stdout, "")
                    self.assertIn(sentence.decode(), result.stderr)
                    self.assertIn(str(status), result.stderr)

    def test_an_accepted_read_prints_exactly_the_server_answer(self):
        env = {**self.env, "LOCA_ENV": str(self.identity(self.root / "reviewer.env", "reviewer", "dv_reviewer"))}
        self.body = json.dumps([{"key": "release", "body": "two words"}]).encode()
        for command, args in READS.items():
            with self.subTest(command=command):
                result = self.run_connect(command, self.origin, *args, env=env)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout, self.body.decode())
        self.assertTrue(all(token == "dv_reviewer" for _, path, token in self.seen if path != "/rooms"))

    def test_an_unreachable_server_is_a_failure_not_curl_exit_status(self):
        # A closed port: curl itself exits 7 ("could not connect"), which a
        # caller would read as send's 401 code. A read says it failed (8).
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 0))
            closed = f"http://127.0.0.1:{probe.getsockname()[1]}"
        result = self.run_connect("notes", closed, "sb-dev")
        self.assertEqual(result.returncode, 8, result.stderr)
        self.assertEqual(result.stdout, "")
        self.assertIn("nothing was read", result.stderr)

    def test_wait_and_wait_clear_use_the_named_identity_file(self):
        # ~/.loca/env belongs to another agent on the same machine; the name
        # on the command line must select ~/.loca/reviewer.env, as `send` does.
        self.identity(self.root / ".loca" / "env", "other-agent", "dv_other")
        self.identity(self.root / ".loca" / "reviewer.env", "reviewer", "dv_reviewer")
        self.status, self.body = 201, b"{}"
        for args in (["wait", self.origin, "sb-dev", "reviewer", "operator", "needs a decision"],
                     ["wait-clear", self.origin, "sb-dev", "reviewer"]):
            with self.subTest(command=args[0]):
                self.seen.clear()
                result = self.run_connect(*args)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual([token for _, _, token in self.seen], ["dv_reviewer"])


if __name__ == "__main__":
    unittest.main()
