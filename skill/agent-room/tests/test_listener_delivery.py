import json
import io
import importlib.util
import os
import socket
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest import mock
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse


SKILL_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SKILL_DIR))
LISTENER_SPEC = importlib.util.spec_from_file_location(
    "loca_listener", SKILL_DIR / "listen.py"
)
assert LISTENER_SPEC and LISTENER_SPEC.loader
LISTENER = importlib.util.module_from_spec(LISTENER_SPEC)
LISTENER_SPEC.loader.exec_module(LISTENER)


class TurnServer(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_):
        pass

    def do_GET(self):
        if urlparse(self.path).path != "/ws":
            self.send_response(404)
            self.end_headers()
            return
        self.send_response(101, "Switching Protocols")
        self.send_header("Upgrade", "websocket")
        self.send_header("Connection", "Upgrade")
        self.end_headers()
        event = {
            "t": "turn",
            "messages": [
                {
                    "id": 201,
                    "room": "reviewer",
                    "sender": "operator",
                    "sender_type": "user",
                    "target": "worker",
                    "text": "one",
                    "ts": 1,
                },
                {
                    "id": 202,
                    "room": "reviewer",
                    "sender": "operator",
                    "sender_type": "user",
                    "target": "worker",
                    "text": "two",
                    "ts": 2,
                },
                {
                    "id": 203,
                    "room": "reviewer",
                    "sender": "operator",
                    "sender_type": "user",
                    "target": "worker",
                    "text": "three",
                    "ts": 3,
                },
            ],
        }
        payload = json.dumps(event).encode()
        assert len(payload) < 65536
        header = bytes([0x81, 126]) + len(payload).to_bytes(2, "big")
        self.wfile.write(header + payload)
        self.wfile.flush()
        time.sleep(1)


class ReconnectBackfillServer(BaseHTTPRequestHandler):
    """Fail one REST resync; the listener must reconnect and try it again."""

    protocol_version = "HTTP/1.1"

    def log_message(self, *_):
        pass

    def reply(self, status, body, content_type="application/json"):
        self.send_response(status)
        self.send_header("content-type", content_type)
        self.send_header("content-length", str(len(body)))
        self.send_header("connection", "close")
        self.end_headers()
        self.wfile.write(body)
        self.wfile.flush()

    def do_POST(self):
        self.reply(404, b"not found", "text/plain")

    def do_GET(self):
        path = urlparse(self.path).path
        if path == "/rooms/reviewer/settings":
            self.reply(200, b'{"lead":null}')
            return
        if path == "/rooms/reviewer/messages":
            self.server.backfill_calls += 1
            if self.server.backfill_calls == 1:
                self.reply(503, b"retry", "text/plain")
                return
            self.reply(
                200,
                json.dumps(
                    [
                        {
                            "id": 101,
                            "room": "reviewer",
                            "sender": "operator",
                            "sender_type": "user",
                            "target": "worker",
                            "text": "missed while disconnected",
                            "ts": 3,
                        }
                    ]
                ).encode(),
            )
            return
        if path != "/ws":
            self.reply(404, b"not found", "text/plain")
            return
        self.server.ws_connections += 1
        self.send_response(101, "Switching Protocols")
        self.send_header("Upgrade", "websocket")
        self.send_header("Connection", "Upgrade")
        self.end_headers()
        self.wfile.flush()
        time.sleep(0.5)


class ListenerDeliveryTests(unittest.TestCase):
    def test_backfill_paginates_the_complete_durable_gap(self):
        messages = [
            {
                "id": message_id,
                "room": "reviewer",
                "sender": "operator",
                "sender_type": "user",
                "target": "worker",
                "text": f"missed {message_id}",
                "ts": message_id,
            }
            for message_id in range(101, 506)
        ]
        requested_since = []

        def page(request, timeout=10):
            del timeout
            query = parse_qs(urlparse(request.full_url).query)
            since = int(query["since"][0])
            limit = int(query["limit"][0])
            requested_since.append(since)
            body = [message for message in messages if message["id"] > since][:limit]
            return io.BytesIO(json.dumps(body).encode())

        with mock.patch.object(LISTENER.urllib.request, "urlopen", side_effect=page):
            pages = list(
                LISTENER.backfill_pages(
                    "wss://loca.example/ws?"
                    "room=reviewer&name=worker&filter=mentions",
                    100,
                    "worker",
                    False,
                )
            )

        recovered = [message for _, page in pages for message in page]
        self.assertEqual([message["id"] for message in recovered], list(range(101, 506)))
        self.assertEqual(requested_since, [100, 300, 500])
        self.assertEqual([cursor for cursor, _ in pages], [300, 500, 505])

    def test_backfill_defers_filtering_so_a_lead_recovers_plain_room_messages(self):
        messages = [
            {
                "id": 101,
                "room": "reviewer",
                "sender": "operator",
                "sender_type": "user",
                "text": "plain room context",
                "ts": 3,
            }
        ]
        response = io.BytesIO(json.dumps(messages).encode())
        with mock.patch.object(
            LISTENER.urllib.request, "urlopen", return_value=response
        ):
            pages = list(
                LISTENER.backfill_pages(
                    "wss://loca.example/ws?"
                    "room=reviewer&name=lead-engineer&filter=mentions",
                    100,
                    "lead-engineer",
                    True,
                )
            )
        recovered = [message for _, page in pages for message in page]
        self.assertEqual(recovered, messages)
        self.assertTrue(
            LISTENER.should_deliver_message(
                recovered[0],
                skip_own="lead-engineer",
                only_direct="lead-engineer",
                current_lead=True,
            )
        )

        response = io.BytesIO(json.dumps(messages).encode())
        with mock.patch.object(
            LISTENER.urllib.request, "urlopen", return_value=response
        ):
            ordinary_pages = list(
                LISTENER.backfill_pages(
                    "wss://loca.example/ws?"
                    "room=reviewer&name=worker&filter=mentions",
                    100,
                    "worker",
                    False,
                )
            )
        ordinary_agent = [message for _, page in ordinary_pages for message in page]
        self.assertEqual(ordinary_agent, [])
        self.assertEqual([cursor for cursor, _ in ordinary_pages], [101])

    def test_runtime_turn_limit_uses_query_then_settings_and_is_bounded(self):
        base = "wss://loca.example/ws?room=reviewer&name=worker"
        self.assertEqual(LISTENER.runtime_turn_max(base), 4)
        self.assertEqual(
            LISTENER.runtime_turn_max(base, {"turn_max_messages": 7}), 7
        )
        self.assertEqual(LISTENER.runtime_turn_max(base + "&turn_max=2"), 2)
        self.assertEqual(LISTENER.runtime_turn_max(base + "&turn_max=99"), 16)
        self.assertEqual(LISTENER.runtime_turn_max(base + "&turn_max=0"), 1)
        self.assertEqual(LISTENER.runtime_turn_max(base + "&turn_max=nope"), 4)

    def test_chunk_messages_preserves_order_and_bounds_every_delivery(self):
        batches = list(LISTENER.chunk_messages(list(range(11)), 4))
        self.assertEqual(batches, [[0, 1, 2, 3], [4, 5, 6, 7], [8, 9, 10]])

    def test_lobby_listener_restores_dynamic_room_cursor(self):
        with tempfile.TemporaryDirectory() as tmp:
            cursor = Path(tmp) / "cursor.json"
            cursor.write_text(
                json.dumps({"__lobby__": 0, "iye": 27602}),
                encoding="utf-8",
            )
            state = LISTENER.load_cursor_state(cursor, ["__lobby__"])
        self.assertEqual(state["__lobby__"], 0)
        self.assertEqual(state["iye"], 27602)

    def test_websocket_ping_gets_a_masked_pong(self):
        client, server = socket.socketpair()
        try:
            server.settimeout(1)
            server.sendall(b"\x89\x02hi\x81\x02ok")
            self.assertEqual(next(LISTENER.frames(client)), "ok")
            pong = server.recv(64)
            self.assertEqual(pong[0] & 0x0F, 0x0A)
            self.assertTrue(pong[1] & 0x80)
            length = pong[1] & 0x7F
            mask = pong[2:6]
            payload = bytes(
                value ^ mask[index % 4]
                for index, value in enumerate(pong[6 : 6 + length])
            )
            self.assertEqual(payload, b"hi")
        finally:
            client.close()
            server.close()

    def test_websocket_idle_timeout_is_not_swallowed(self):
        client, server = socket.socketpair()
        try:
            client.settimeout(0.02)
            with self.assertRaises(TimeoutError):
                next(LISTENER.frames(client))
        finally:
            client.close()
            server.close()

    def test_direct_name_matching_is_exact(self):
        self.assertTrue(
            LISTENER.directly_names(
                {"target": "lead-engineer", "text": ""}, "lead-engineer"
            )
        )
        self.assertTrue(
            LISTENER.directly_names(
                {"target": "", "text": "bak @LEAD-engineer şimdi"}, "lead-engineer"
            )
        )
        self.assertFalse(
            LISTENER.directly_names(
                {"target": "", "text": "@lead-engineer-old bak"}, "lead-engineer"
            )
        )
        self.assertFalse(
            LISTENER.directly_names(
                {"target": "all", "text": "@all bak"}, "lead-engineer"
            )
        )
        self.assertTrue(
            LISTENER.should_deliver_message(
                {"sender": "operator", "target": "all", "text": "@all bak"},
                skip_own="lead-engineer",
                only_direct="lead-engineer",
                current_lead=False,
            )
        )
        self.assertTrue(
            LISTENER.directly_names(
                {
                    "target": "all",
                    "text": "@all karar için @lead-engineer bakar mısın?",
                },
                "lead-engineer",
            )
        )
        self.assertTrue(
            LISTENER.directly_names(
                {
                    "target": "assistant-dev",
                    "text": "@lead-engineer bunu da gözden geçir",
                },
                "lead-engineer",
            )
        )

    def test_only_direct_does_not_hide_room_messages_from_current_lead(self):
        room_message = {
            "sender": "debug",
            "sender_type": "agent",
            "target": "assistant-dev",
            "text": "review sonucu",
        }
        self.assertFalse(
            LISTENER.should_deliver_message(
                room_message,
                skip_own="lead-engineer",
                only_direct="lead-engineer",
                current_lead=False,
            )
        )
        self.assertTrue(
            LISTENER.should_deliver_message(
                room_message,
                skip_own="lead-engineer",
                only_direct="lead-engineer",
                current_lead=True,
            )
        )

    def test_protocol_v1_priorities_distinguish_direct_broadcast_and_task(self):
        direct = LISTENER.make_delivery(
            "sb-dev",
            {
                "id": 1,
                "sender": "operator",
                "sender_type": "user",
                "target": "lead-engineer",
                "text": "hey",
            },
            "lead-engineer",
            "https://loca.example",
        )
        broadcast = LISTENER.make_delivery(
            "sb-dev",
            {
                "id": 2,
                "sender": "operator",
                "sender_type": "user",
                "target": "all",
                "text": "@all durum",
            },
            "lead-engineer",
            "https://loca.example",
        )
        task = LISTENER.make_delivery(
            "sb-dev",
            {"t": "task", "task": {"id": 7}},
            "lead-engineer",
            "https://loca.example",
        )
        lead_room = LISTENER.make_delivery(
            "sb-dev",
            {
                "id": 3,
                "sender": "debug",
                "sender_type": "agent",
                "target": "assistant-dev",
                "text": "review sonucu",
            },
            "lead-engineer",
            "https://loca.example",
            is_lead=True,
        )
        care = LISTENER.make_delivery(
            "sb-dev",
            {
                "t": "care",
                "signal": {
                    "id": "sb-dev:WaitCycle:10:1",
                    "room": "sb-dev",
                    "owner": "loca-care",
                    "reason": "wait_cycle",
                },
            },
            "loca-care",
            "https://loca.example",
        )
        reaction = LISTENER.make_delivery(
            "sb-dev",
            {
                "t": "reaction",
                "reaction": {
                    "message_id": 9,
                    "owner": "lead-engineer",
                    "reactor": "operator",
                    "emoji": "✦",
                    "ts": 123,
                },
            },
            "lead-engineer",
            "https://loca.example",
        )
        self.assertEqual(direct["protocol_version"], "1")
        self.assertEqual(direct["identity"], "lead-engineer")
        self.assertEqual(direct["server"], "https://loca.example")
        self.assertEqual(direct["priority"], "direct_user")
        self.assertEqual(broadcast["priority"], "broadcast")
        self.assertEqual(task["priority"], "explicit_task")
        self.assertEqual(lead_room["priority"], "lead_room")
        self.assertEqual(care["priority"], "care_signal")
        self.assertEqual(reaction["priority"], "direct_user")
        self.assertEqual(
            reaction["delivery_id"], "sb-dev:reaction:9:operator:✦:123"
        )
        self.assertEqual(
            care["delivery_id"], "sb-dev:care:sb-dev:WaitCycle:10:1"
        )

    def test_care_delivery_rejects_a_cross_room_signal(self):
        with self.assertRaisesRegex(ValueError, "cross-room care signal"):
            LISTENER.make_delivery(
                "iye",
                {
                    "t": "care",
                    "signal": {
                        "id": "stale-delivery",
                        "room": "sb-dev",
                        "owner": "loca-dev",
                        "reason": "room_silence",
                    },
                },
                "loca-dev",
                "https://loca.example",
            )

    def test_three_messages_become_one_inbox_delivery_and_three_history_lines(self):
        server = ThreadingHTTPServer(("127.0.0.1", 0), TurnServer)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        process = None
        try:
            with tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                identity = root / "worker.env"
                history = root / "messages.jsonl"
                inbox = root / "inbox.jsonl"
                cursor = root / "listener-cursor.json"
                identity.write_text(
                    "\n".join(
                        [
                            f"ROOM_SERVER_URL=http://127.0.0.1:{server.server_port}",
                            "LOCA_NAME=worker",
                            "DAVET_reviewer=dv_worker",
                            "",
                        ]
                    ),
                    encoding="utf-8",
                )
                env = dict(os.environ)
                env["LOCA_ENV"] = str(identity)
                process = subprocess.Popen(
                    [
                        "python3",
                        str(SKILL_DIR / "listen.py"),
                        (
                            f"ws://127.0.0.1:{server.server_port}/ws?"
                            "room=reviewer&name=worker&type=agent&filter=mentions"
                        ),
                        str(history),
                        "--skip-own",
                        "worker",
                        "--cursor",
                        str(cursor),
                        "--turn-log",
                        str(inbox),
                    ],
                    env=env,
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                )
                deadline = time.monotonic() + 3
                while time.monotonic() < deadline:
                    if inbox.is_file() and inbox.stat().st_size:
                        break
                    time.sleep(0.05)
                else:
                    self.fail("listener did not persist the server turn")

                deliveries = [
                    json.loads(line)
                    for line in inbox.read_text(encoding="utf-8").splitlines()
                ]
                messages = [
                    json.loads(line)
                    for line in history.read_text(encoding="utf-8").splitlines()
                ]
                self.assertEqual(len(deliveries), 1)
                self.assertEqual(deliveries[0]["delivery_id"], "reviewer:203")
                self.assertEqual(deliveries[0]["protocol_version"], "1")
                self.assertEqual(deliveries[0]["identity"], "worker")
                self.assertEqual(
                    deliveries[0]["server"],
                    f"http://127.0.0.1:{server.server_port}",
                )
                self.assertEqual(len(deliveries[0]["event"]["messages"]), 3)
                self.assertEqual([message["id"] for message in messages], [201, 202, 203])
        finally:
            if process is not None and process.poll() is None:
                process.terminate()
                process.wait(timeout=2)
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)

    def test_failed_backfill_forces_reconnect_and_retries_the_gap(self):
        server = ThreadingHTTPServer(
            ("127.0.0.1", 0), ReconnectBackfillServer
        )
        server.backfill_calls = 0
        server.ws_connections = 0
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        process = None
        log_handle = None
        try:
            with tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                identity = root / "worker.env"
                history = root / "messages.jsonl"
                inbox = root / "inbox.jsonl"
                cursor = root / "listener-cursor.json"
                identity.write_text(
                    "\n".join(
                        [
                            f"ROOM_SERVER_URL=http://127.0.0.1:{server.server_port}",
                            "LOCA_NAME=worker",
                            "DAVET_reviewer=dv_worker",
                            "",
                        ]
                    ),
                    encoding="utf-8",
                )
                cursor.write_text('{"reviewer":100}\n', encoding="utf-8")
                env = dict(os.environ)
                env["LOCA_ENV"] = str(identity)
                listener_log = root / "listener.log"
                log_handle = listener_log.open("wb")
                process = subprocess.Popen(
                    [
                        "python3",
                        str(SKILL_DIR / "listen.py"),
                        (
                            f"ws://127.0.0.1:{server.server_port}/ws?"
                            "room=reviewer&name=worker&type=agent&filter=mentions"
                        ),
                        str(history),
                        "--skip-own",
                        "worker",
                        "--cursor",
                        str(cursor),
                        "--turn-log",
                        str(inbox),
                    ],
                    env=env,
                    stdout=subprocess.DEVNULL,
                    stderr=log_handle,
                )
                deadline = time.monotonic() + 7
                while time.monotonic() < deadline:
                    if inbox.is_file() and inbox.stat().st_size:
                        break
                    time.sleep(0.05)
                else:
                    log_handle.flush()
                    self.fail(
                        "listener did not retry the failed REST backfill:\n"
                        + listener_log.read_text(encoding="utf-8")
                    )

                delivery = json.loads(
                    inbox.read_text(encoding="utf-8").splitlines()[0]
                )
                self.assertEqual(delivery["delivery_id"], "reviewer:101")
                self.assertGreaterEqual(server.backfill_calls, 2)
                self.assertGreaterEqual(server.ws_connections, 2)

                # The inbox append is flushed before the cursor is advanced:
                # that ordering prevents a failed delivery from skipping the
                # message after restart.  Do not race the listener between
                # those two durable writes on fast CI runners.
                cursor_deadline = time.monotonic() + 2
                while time.monotonic() < cursor_deadline:
                    if (
                        json.loads(cursor.read_text(encoding="utf-8"))[
                            "reviewer"
                        ]
                        == 101
                    ):
                        break
                    time.sleep(0.01)
                self.assertEqual(
                    json.loads(cursor.read_text(encoding="utf-8"))["reviewer"],
                    101,
                )
                self.assertEqual(cursor.stat().st_mode & 0o777, 0o600)
        finally:
            if process is not None and process.poll() is None:
                process.terminate()
                process.wait(timeout=2)
            if log_handle is not None:
                log_handle.close()
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)


if __name__ == "__main__":
    unittest.main()
