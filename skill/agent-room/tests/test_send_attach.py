"""`connect.sh send --attach <file>` uploads each file, then posts a message
citing the returned ids. The invocation is identical for Claude Code and Codex
(one path for both runtimes), so one test covers both. The mock server stands in
for room-server: it issues a content-addressed id per upload and records the
message body so we can assert the citation wiring end to end.
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
        # /rooms/<room>/attachments
        if len(parts) == 3 and parts[0] == "rooms" and parts[2] == "attachments":
            room = parts[1]
            filename = self.headers.get("x-filename", "")
            content_type = self.headers.get("content-type", "")
            AttachHandler.uploads.append((room, filename, content_type, raw))
            sha = hashlib.sha256(raw).hexdigest()
            self._json(
                {
                    "id": sha,
                    "sha256": sha,
                    "name": filename or "attachment.bin",
                    "mime": "image/png",
                    "size": len(raw),
                },
                status=201,
            )
        # /rooms/<room>/messages
        elif len(parts) == 3 and parts[0] == "rooms" and parts[2] == "messages":
            body = json.loads(raw.decode() or "{}")
            AttachHandler.messages.append(body)
            self._json(
                {
                    "id": 1,
                    "room": parts[1],
                    "sender": body.get("sender"),
                    "sender_type": "agent",
                    "text": body.get("text", ""),
                    "ts": 1,
                    "attachments": [
                        {"id": i, "sha256": i, "name": "x", "mime": "image/png", "size": 1}
                        for i in body.get("attachments", [])
                    ],
                },
                status=201,
            )
        else:
            self._json({"error": "not found"}, status=404)


class SendAttachTest(unittest.TestCase):
    def setUp(self):
        AttachHandler.uploads = []
        AttachHandler.messages = []

    def _run(self, extra_args, files):
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
                paths = []
                for name, data in files:
                    p = Path(tmp) / name
                    p.write_bytes(data)
                    paths.append(str(p))
                env = dict(os.environ)
                env.update({"HOME": tmp, "LOCA_ENV": str(env_file)})
                args = [
                    str(SKILL_DIR / "connect.sh"),
                    "send",
                    origin,
                    "alpha",
                    "debug",
                    "-",
                ] + extra_args
                # Interleave --attach flags with the file paths.
                for p in paths:
                    args += ["--attach", p]
                return (
                    subprocess.run(
                        args, env=env, text=True, capture_output=True, timeout=15, check=False
                    ),
                    paths,
                )
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)

    def test_single_attachment_uploaded_and_cited(self):
        png = bytes([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) + b"\x01" * 32
        result, _ = self._run(["hello with a pic"], [("pic.png", png)])
        self.assertEqual(result.returncode, 0, result.stderr)
        # Exactly one upload, carrying the file's bytes + name.
        self.assertEqual(len(AttachHandler.uploads), 1)
        room, filename, _ctype, body = AttachHandler.uploads[0]
        self.assertEqual(room, "alpha")
        self.assertEqual(filename, "pic.png")
        self.assertEqual(body, png)
        # The message cites exactly the uploaded id (== sha256 of the bytes).
        self.assertEqual(len(AttachHandler.messages), 1)
        msg = AttachHandler.messages[0]
        self.assertEqual(msg["text"], "hello with a pic")
        self.assertEqual(msg["attachments"], [hashlib.sha256(png).hexdigest()])

    def test_multiple_attachments_all_cited_in_order(self):
        a = bytes([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) + b"a"
        b = b"%PDF-1.7\n" + b"b"
        result, _ = self._run(
            ["two files"], [("one.png", a), ("two.pdf", b)]
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(AttachHandler.uploads), 2)
        self.assertEqual([u[1] for u in AttachHandler.uploads], ["one.png", "two.pdf"])
        msg = AttachHandler.messages[0]
        self.assertEqual(
            msg["attachments"],
            [hashlib.sha256(a).hexdigest(), hashlib.sha256(b).hexdigest()],
        )

    def test_missing_file_aborts_without_posting(self):
        result = subprocess.run(
            [
                str(SKILL_DIR / "connect.sh"),
                "send",
                "http://127.0.0.1:1",  # unused; the missing file fails first
                "alpha",
                "debug",
                "-",
                "text",
                "--attach",
                "/no/such/file.png",
            ],
            env={**os.environ, "DAVET_alpha": "dv_test", "LOCA_SESSION": "st_live"},
            text=True,
            capture_output=True,
            timeout=15,
            check=False,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("no such file", result.stderr)
        # No message was posted (no server was even contacted for /messages).
        self.assertEqual(AttachHandler.messages, [])


if __name__ == "__main__":
    unittest.main()
