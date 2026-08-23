import json
import os
import socket
import subprocess
import tempfile
import time
import unittest
import urllib.request
from pathlib import Path


SKILL_DIR = Path(__file__).resolve().parents[1]
REPO_ROOT = SKILL_DIR.parents[1]
SERVER_BIN = REPO_ROOT / "target" / "debug" / "room-server"


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def request_json(url, *, method="GET", token=None, body=None):
    encoded = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(url, data=encoded, method=method)
    if encoded is not None:
        request.add_header("content-type", "application/json")
    if token:
        request.add_header("x-room-token", token)
    with urllib.request.urlopen(request, timeout=10) as response:
        payload = response.read()
    return json.loads(payload.decode()) if payload else None


class DurableBackfillEndToEndTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if SERVER_BIN.is_file():
            return
        subprocess.run(
            ["cargo", "build", "-q", "-p", "server"],
            cwd=REPO_ROOT,
            check=True,
            timeout=180,
        )

    def start_server(self, port, database, log):
        env = dict(os.environ)
        env.update(
            {
                "PORT": str(port),
                "BIND_ADDR": "127.0.0.1",
                "DB_PATH": str(database),
                "ADMIN_TOKEN": "e2e-admin",
                "ROOM_TOKEN": "e2e-room",
                "RATE_LIMIT": "0",
                "REQUIRE_SESSIONS": "0",
                "LEGACY_WS_QUERY_AUTH": "0",
                "RUST_LOG": "warn",
            }
        )
        handle = log.open("ab")
        process = subprocess.Popen(
            [str(SERVER_BIN)],
            cwd=REPO_ROOT,
            env=env,
            stdout=handle,
            stderr=subprocess.STDOUT,
        )
        deadline = time.monotonic() + 10
        health = f"http://127.0.0.1:{port}/health"
        while time.monotonic() < deadline:
            if process.poll() is not None:
                handle.close()
                self.fail(
                    "room-server exited during boot:\n"
                    + log.read_text(encoding="utf-8", errors="replace")
                )
            try:
                if request_json(health).get("ok") is True:
                    return process, handle
            except Exception:
                time.sleep(0.05)
        process.terminate()
        process.wait(timeout=3)
        handle.close()
        self.fail("room-server did not become healthy")

    @staticmethod
    def stop_process(process, handle=None):
        if process is not None and process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=3)
        if handle is not None:
            handle.close()

    def test_restart_gap_is_bounded_checkpointed_and_not_replayed(self):
        """A 1,005-row outage becomes 252 durable turns, never one giant turn."""
        server = listener = None
        server_log_handle = None
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            port = free_port()
            base = f"http://127.0.0.1:{port}"
            database = root / "loca.sqlite3"
            server_log = root / "server.log"
            listener_log = root / "listener.log"
            history = root / "messages.jsonl"
            inbox = root / "inbox.jsonl"
            cursor = root / "cursor.json"
            identity = root / "worker.env"

            try:
                server, server_log_handle = self.start_server(
                    port, database, server_log
                )
                endpoint = f"{base}/rooms/reviewer/messages"
                baseline = request_json(
                    endpoint,
                    method="POST",
                    token="e2e-room",
                    body={
                        "sender": "operator",
                        "sender_type": "user",
                        "target": "worker",
                        "text": "cursor baseline",
                    },
                )
                cursor.write_text(
                    json.dumps({"reviewer": baseline["id"]}) + "\n",
                    encoding="utf-8",
                )

                expected_ids = []
                payload = "x" * 1024
                for index in range(1005):
                    message = request_json(
                        endpoint,
                        method="POST",
                        token="e2e-room",
                        body={
                            "sender": "operator",
                            "sender_type": "user",
                            "target": "worker",
                            "text": f"missed-{index:04d} {payload}",
                        },
                    )
                    expected_ids.append(message["id"])

                # Prove the REST archive, not an in-memory hot tail, owns the
                # recovery by restarting the actual Rust server before listen.
                self.stop_process(server, server_log_handle)
                server = server_log_handle = None
                server, server_log_handle = self.start_server(
                    port, database, server_log
                )

                identity.write_text(
                    "\n".join(
                        [
                            f"ROOM_SERVER_URL={base}",
                            "LOCA_NAME=worker",
                            "ROOM_TOKEN=e2e-room",
                            "",
                        ]
                    ),
                    encoding="utf-8",
                )
                identity.chmod(0o600)
                env = dict(os.environ)
                env["LOCA_ENV"] = str(identity)
                log_handle = listener_log.open("ab")
                listener = subprocess.Popen(
                    [
                        "python3",
                        "-u",
                        str(SKILL_DIR / "listen.py"),
                        (
                            f"ws://127.0.0.1:{port}/ws?room=reviewer&"
                            "name=worker&type=agent&filter=mentions&turn_max=4"
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

                deadline = time.monotonic() + 20
                while time.monotonic() < deadline:
                    try:
                        state = json.loads(cursor.read_text(encoding="utf-8"))
                    except (FileNotFoundError, json.JSONDecodeError):
                        state = {}
                    if state.get("reviewer") == expected_ids[-1]:
                        break
                    if listener.poll() is not None:
                        self.fail(
                            "listener exited during backfill:\n"
                            + listener_log.read_text(
                                encoding="utf-8", errors="replace"
                            )
                        )
                    time.sleep(0.05)
                else:
                    self.fail(
                        "listener did not checkpoint the complete durable gap:\n"
                        + listener_log.read_text(encoding="utf-8", errors="replace")
                    )

                self.stop_process(listener, log_handle)
                listener = None
                history_rows = [
                    json.loads(line)
                    for line in history.read_text(encoding="utf-8").splitlines()
                ]
                deliveries = [
                    json.loads(line)
                    for line in inbox.read_text(encoding="utf-8").splitlines()
                ]
                delivered_ids = [
                    message["id"]
                    for delivery in deliveries
                    for message in delivery["event"].get(
                        "messages", [delivery["event"]]
                    )
                ]
                self.assertEqual(
                    [message["id"] for message in history_rows], expected_ids
                )
                self.assertEqual(delivered_ids, expected_ids)
                self.assertEqual(len(deliveries), 252)
                self.assertTrue(
                    all(
                        len(delivery["event"].get("messages", [])) <= 4
                        for delivery in deliveries
                    )
                )
                self.assertLess(
                    max(len(json.dumps(delivery)) for delivery in deliveries),
                    10_000,
                )

                # The persisted cursor is the replay boundary. A clean restart
                # must not append any duplicate history or inbox delivery.
                history_size = history.stat().st_size
                inbox_size = inbox.stat().st_size
                log_handle = listener_log.open("ab")
                listener = subprocess.Popen(
                    [
                        "python3",
                        "-u",
                        str(SKILL_DIR / "listen.py"),
                        (
                            f"ws://127.0.0.1:{port}/ws?room=reviewer&"
                            "name=worker&type=agent&filter=mentions&turn_max=4"
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
                time.sleep(1)
                self.assertIsNone(listener.poll())
                self.assertEqual(history.stat().st_size, history_size)
                self.assertEqual(inbox.stat().st_size, inbox_size)
                self.stop_process(listener, log_handle)
                listener = None
            finally:
                self.stop_process(listener)
                self.stop_process(server, server_log_handle)


if __name__ == "__main__":
    unittest.main()
