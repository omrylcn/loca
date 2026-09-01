"""`connect.sh send --attach <file>` uploads each file, then posts a message
citing the returned ids. The invocation is identical for Claude Code and Codex
(one path for both runtimes), so one test covers both. The mock server stands in
for room-server: it issues a content-addressed id per upload and records the
message body so we can assert the citation wiring end to end.

Every case is HERMETIC: a temp HOME + temp LOCA_ENV, so the test never reads the
machine's real ~/.loca/*.env and cannot break on an identity mismatch.
"""

import hashlib
import json
import os
import subprocess
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

SKILL_DIR = Path(__file__).resolve().parents[1]


class AttachHandler(BaseHTTPRequestHandler):
    uploads = []          # (room, filename, content_type, body_bytes)
    messages = []         # parsed JSON message bodies

    def log_message(self, *_):
        pass

    def _json(self, body, status=200):
        payload = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def do_GET(self):
        if self.path == "/health":
            self._json({"ok": True})
        else:
            self._json({"error": "not found"}, status=404)

    def do_POST(self):
        length = int(self.headers.get("content-length", "0"))
        raw = self.rfile.read(length) if length else b""
        parts = self.path.strip("/").split("/")
        if len(parts) == 3 and parts[0] == "rooms" and parts[2] == "attachments":
            filename = self.headers.get("x-filename", "")
            AttachHandler.uploads.append(
                (parts[1], filename, self.headers.get("content-type", ""), raw)
            )
            sha = hashlib.sha256(raw).hexdigest()
            self._json(
                {"id": sha, "sha256": sha, "name": filename or "attachment.bin",
                 "mime": "image/png", "size": len(raw)},
                status=201,
            )
        elif len(parts) == 3 and parts[0] == "rooms" and parts[2] == "messages":
            body = json.loads(raw.decode() or "{}")
            AttachHandler.messages.append(body)
            self._json({"id": 1, "room": parts[1], "sender": body.get("sender"),
                        "sender_type": "agent", "text": body.get("text", ""), "ts": 1,
                        "attachments": [{"id": i, "sha256": i, "name": "x",
                                         "mime": "image/png", "size": 1}
                                        for i in body.get("attachments", [])]},
                       status=201)
        else:
            self._json({"error": "not found"}, status=404)


PNG = bytes([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A])


class SendAttachTest(unittest.TestCase):
    def setUp(self):
        AttachHandler.uploads = []
        AttachHandler.messages = []

    def _send(self, make_tail):
        """Run `connect.sh send ... -` against a live mock server, hermetic
        (temp HOME + LOCA_ENV). `make_tail(tmp, origin)` returns the args after
        the target `-`, and may create files under `tmp` first."""
        server = ThreadingHTTPServer(("127.0.0.1", 0), AttachHandler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            with tempfile.TemporaryDirectory() as tmp:
                loca_dir = Path(tmp) / ".loca"
                loca_dir.mkdir()
                origin = f"http://127.0.0.1:{server.server_port}"
                env_file = loca_dir / "debug.env"
                env_file.write_text(
                    f"ROOM_SERVER_URL={origin}\n"
                    "LOCA_NAME=debug\n"
                    "DAVET_alpha=dv_test\n"
                    "LOCA_SESSION=st_live\n",
                    encoding="utf-8",
                )
                env = dict(os.environ)
                env.update({"HOME": tmp, "LOCA_ENV": str(env_file)})
                args = [
                    str(SKILL_DIR / "connect.sh"), "send", origin, "alpha", "debug", "-",
                ] + make_tail(tmp, origin)
                return subprocess.run(
                    args, env=env, text=True, capture_output=True, timeout=15, check=False
                )
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)

    @staticmethod
    def _file(tmp, name, data):
        p = Path(tmp) / name
        p.write_bytes(data)
        return str(p)

    # ---- happy path ----

    def test_single_attachment_uploaded_and_cited(self):
        data = PNG + b"\x01" * 32
        r = self._send(lambda tmp, o: ["hello with a pic", "--attach",
                                       self._file(tmp, "pic.png", data)])
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(len(AttachHandler.uploads), 1)
        room, filename, _ct, body = AttachHandler.uploads[0]
        self.assertEqual((room, filename, body), ("alpha", "pic.png", data))
        msg = AttachHandler.messages[0]
        self.assertEqual(msg["text"], "hello with a pic")
        self.assertEqual(msg["attachments"], [hashlib.sha256(data).hexdigest()])

    def test_multiple_attachments_all_cited_in_order(self):
        a, b = PNG + b"a", b"%PDF-1.7\n" + b"b"
        r = self._send(lambda tmp, o: [
            "two files",
            "--attach", self._file(tmp, "one.png", a),
            "--attach", self._file(tmp, "two.pdf", b),
        ])
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual([u[1] for u in AttachHandler.uploads], ["one.png", "two.pdf"])
        self.assertEqual(
            AttachHandler.messages[0]["attachments"],
            [hashlib.sha256(a).hexdigest(), hashlib.sha256(b).hexdigest()],
        )

    # ---- client-side validation (loca-dev NO-GO): must fail BEFORE any upload ----

    def test_dangling_attach_is_an_error(self):
        r = self._send(lambda tmp, o: ["text", "--attach"])
        self.assertEqual(r.returncode, 2, r.stderr)
        self.assertIn("--attach requires a file path", r.stderr)
        self.assertEqual(AttachHandler.uploads, [])
        self.assertEqual(AttachHandler.messages, [])

    def test_more_than_four_attachments_rejected_before_upload(self):
        def make(tmp, o):
            tail = ["too many"]
            for i in range(5):
                tail += ["--attach", self._file(tmp, f"f{i}.png", PNG + bytes([i]))]
            return tail
        r = self._send(make)
        self.assertEqual(r.returncode, 2, r.stderr)
        self.assertIn("at most 4 attachments", r.stderr)
        # The whole point: nothing was uploaded, so no orphan pending blob.
        self.assertEqual(AttachHandler.uploads, [])
        self.assertEqual(AttachHandler.messages, [])

    def test_filename_with_control_char_rejected_before_upload(self):
        def make(tmp, o):
            bad = Path(tmp) / "bad\nname.png"
            bad.write_bytes(PNG)
            return ["text", "--attach", str(bad)]
        r = self._send(make)
        self.assertEqual(r.returncode, 2, r.stderr)
        self.assertIn("control characters", r.stderr)
        self.assertEqual(AttachHandler.uploads, [])
        self.assertEqual(AttachHandler.messages, [])

    def test_missing_file_aborts_without_posting(self):
        r = self._send(lambda tmp, o: ["text", "--attach", "/no/such/file.png"])
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("no such file", r.stderr)
        self.assertEqual(AttachHandler.messages, [])


if __name__ == "__main__":
    unittest.main()
