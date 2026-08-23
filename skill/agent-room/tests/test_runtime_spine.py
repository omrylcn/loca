import json
import os
import signal
import shlex
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest import mock
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlparse


SKILL_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SKILL_DIR))

from runtime_agent import report_runtime_health  # noqa: E402


def stop_process(process: subprocess.Popen, timeout: float = 8.0) -> None:
    """Bound cleanup without treating host scheduler delay as product failure."""
    process.terminate()
    try:
        process.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=2)


def isolated_runtime_env(root: Path) -> dict[str, str]:
    """Keep supervisor tests independent from a developer's live Loca identity."""
    identity = root / "isolated.env"
    identity.write_text("", encoding="utf-8")
    identity.chmod(0o600)
    env = dict(os.environ)
    env["LOCA_ENV"] = str(identity)
    return env


class SpineServer(BaseHTTPRequestHandler):
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
        if urlparse(self.path).path == "/runtime/health":
            length = int(self.headers.get("content-length", "0"))
            self.server.runtime_report = json.loads(self.rfile.read(length))
            self.reply(200, b"{}")
            return
        self.reply(404, b"not found", "text/plain")

    def do_GET(self):
        path = urlparse(self.path).path
        if path == "/rooms/reviewer/settings":
            self.reply(
                200,
                json.dumps(
                    {"lead": getattr(self.server, "room_lead", None)}
                ).encode(),
            )
            return
        if path != "/ws":
            self.reply(404, b"not found", "text/plain")
            return
        self.send_response(101, "Switching Protocols")
        self.send_header("Upgrade", "websocket")
        self.send_header("Connection", "Upgrade")
        self.end_headers()
        event = getattr(
            self.server,
            "room_event",
            {
                "t": "turn",
                "messages": [
                    {
                        "id": 301,
                        "room": "reviewer",
                        "sender": "operator",
                        "sender_type": "user",
                        "target": "worker",
                        "text": "wake once",
                        "ts": 1,
                    }
                ],
            },
        )
        payload = json.dumps(event).encode()
        self.wfile.write(bytes([0x81, 126]) + len(payload).to_bytes(2, "big"))
        self.wfile.write(payload)
        self.wfile.flush()
        time.sleep(1)


class RuntimeSpineTests(unittest.TestCase):
    def test_health_report_exposes_independent_accepted_milestones(self):
        server = ThreadingHTTPServer(("127.0.0.1", 0), SpineServer)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            with tempfile.TemporaryDirectory() as tmp:
                health = Path(tmp) / "health.json"
                health.write_text(
                    json.dumps(
                        {
                            "wake": "OK",
                            "ack": "OK",
                            "last_attention_id": "attention:test:8",
                            "last_accepted_attention_id": "attention:test:7",
                            "stored": True,
                            "accepted": True,
                            "first_response": True,
                            "final_response": False,
                            "turn_completed": True,
                        }
                    ),
                    encoding="utf-8",
                )
                with mock.patch.dict(
                    os.environ,
                    {
                        "ROOM_SERVER_URL": f"http://127.0.0.1:{server.server_port}",
                        "LOCA_MEMBERSHIP": "mb_test",
                    },
                    clear=False,
                ):
                    report_runtime_health(health, True)
                report = server.runtime_report
                self.assertEqual(report["attention_id"], "attention:test:8")
                self.assertTrue(report["stored"])
                self.assertTrue(report["accepted"])
                self.assertTrue(report["first_response"])
                self.assertFalse(report["final_response"])
                self.assertTrue(report["turn_completed"])
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)

    def test_listener_waits_for_shadow_ingestion_readiness(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            ready = root / "shadow-health.json"
            adapter_started = root / "adapter-started"
            listener_started = root / "listener-started"
            shadow = root / "shadow.py"
            shadow.write_text(
                """import json, pathlib, sys, time
ready = pathlib.Path(sys.argv[1])
started = pathlib.Path(sys.argv[2])
started.write_text(str(time.time_ns()))
time.sleep(0.2)
ready.write_text(json.dumps({"ingestion":"OK"}))
time.sleep(30)
""",
                encoding="utf-8",
            )
            listener = root / "listener.py"
            listener.write_text(
                """import pathlib, sys, time
pathlib.Path(sys.argv[1]).write_text(str(time.time_ns()))
time.sleep(30)
""",
                encoding="utf-8",
            )
            inbox = root / "inbox.jsonl"
            inbox.touch()
            process = subprocess.Popen(
                [
                    "python3",
                    str(SKILL_DIR / "runtime_agent.py"),
                    "--inbox",
                    str(inbox),
                    "--worker-cursor",
                    str(root / "cursor.json"),
                    "--shadow-persistent-exec",
                    shlex.join(
                        ["python3", str(shadow), str(ready), str(adapter_started)]
                    ),
                    "--shadow-ready-file",
                    str(ready),
                    "--",
                    "python3",
                    str(listener),
                    str(listener_started),
                ],
                env=isolated_runtime_env(root),
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            try:
                deadline = time.monotonic() + 3
                while time.monotonic() < deadline and not listener_started.exists():
                    time.sleep(0.05)
                self.assertTrue(listener_started.exists())
                self.assertLess(
                    int(adapter_started.read_text()),
                    int(listener_started.read_text()),
                )
                self.assertEqual(
                    json.loads(ready.read_text())["ingestion"], "OK"
                )
            finally:
                stop_process(process)

    def test_v1_responder_and_v2_shadow_are_supervised_in_parallel(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            listener = root / "listener.py"
            listener.write_text("import time\ntime.sleep(30)\n", encoding="utf-8")
            shadow_bridge = root / "shadow_bridge.py"
            shadow_bridge.write_text(
                "import time\ntime.sleep(30)\n", encoding="utf-8"
            )
            inbox = root / "inbox.jsonl"
            inbox.touch()
            process = subprocess.Popen(
                [
                    "python3",
                    str(SKILL_DIR / "runtime_agent.py"),
                    "--inbox",
                    str(inbox),
                    "--worker-cursor",
                    str(root / "cursor.json"),
                    "--exec",
                    "true",
                    "--shadow-persistent-exec",
                    f"exec python3 {shlex.quote(str(shadow_bridge))}",
                    "--",
                    "python3",
                    str(listener),
                ],
                env=isolated_runtime_env(root),
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            try:
                def children():
                    raw = Path(
                        f"/proc/{process.pid}/task/{process.pid}/children"
                    ).read_text().split()
                    return [int(value) for value in raw]

                old_shadow = old_consumer = old_listener = None
                deadline = time.monotonic() + 4
                while time.monotonic() < deadline:
                    for child in children():
                        cmd = Path(f"/proc/{child}/cmdline").read_bytes()
                        if b"shadow_bridge.py" in cmd:
                            old_shadow = child
                        elif b"runtime_consumer.py" in cmd:
                            old_consumer = child
                        elif b"listener.py" in cmd:
                            old_listener = child
                    if old_shadow and old_consumer and old_listener:
                        break
                    time.sleep(0.05)
                self.assertIsNotNone(old_shadow)
                self.assertIsNotNone(old_consumer)
                self.assertIsNotNone(old_listener)
                os.kill(old_shadow, signal.SIGKILL)

                replacement = None
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline:
                    for child in children():
                        cmd = Path(f"/proc/{child}/cmdline").read_bytes()
                        if b"shadow_bridge.py" in cmd and child != old_shadow:
                            replacement = child
                    if replacement:
                        break
                    time.sleep(0.05)
                self.assertIsNotNone(replacement)
                self.assertTrue(Path(f"/proc/{old_consumer}").exists())
                self.assertTrue(Path(f"/proc/{old_listener}").exists())
            finally:
                stop_process(process)

    def test_persistent_adapter_crash_restarts_without_dropping_listener(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            listener = root / "listener.py"
            listener.write_text("import time\ntime.sleep(30)\n", encoding="utf-8")
            bridge = root / "bridge.py"
            bridge.write_text("import time\ntime.sleep(30)\n", encoding="utf-8")
            inbox = root / "inbox.jsonl"
            inbox.touch()
            process = subprocess.Popen(
                [
                    "python3",
                    str(SKILL_DIR / "runtime_agent.py"),
                    "--inbox",
                    str(inbox),
                    "--worker-cursor",
                    str(root / "cursor.json"),
                    "--health-file",
                    str(root / "health.json"),
                    "--persistent-exec",
                    f"exec python3 {shlex.quote(str(bridge))}",
                    "--",
                    "python3",
                    str(listener),
                ],
                env=isolated_runtime_env(root),
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            try:
                def children():
                    raw = Path(
                        f"/proc/{process.pid}/task/{process.pid}/children"
                    ).read_text().split()
                    return [int(value) for value in raw]

                old_bridge = old_listener = None
                deadline = time.monotonic() + 3
                while time.monotonic() < deadline:
                    for child in children():
                        cmd = Path(f"/proc/{child}/cmdline").read_bytes()
                        if b"bridge.py" in cmd:
                            old_bridge = child
                        elif b"listener.py" in cmd:
                            old_listener = child
                    if old_bridge and old_listener:
                        break
                    time.sleep(0.05)
                self.assertIsNotNone(old_bridge)
                self.assertIsNotNone(old_listener)
                os.kill(old_bridge, signal.SIGKILL)

                replacement = None
                deadline = time.monotonic() + 4
                while time.monotonic() < deadline:
                    for child in children():
                        cmd = Path(f"/proc/{child}/cmdline").read_bytes()
                        if b"bridge.py" in cmd and child != old_bridge:
                            replacement = child
                    if replacement:
                        break
                    time.sleep(0.05)
                self.assertIsNotNone(replacement)
                self.assertTrue(Path(f"/proc/{old_listener}").exists())
            finally:
                stop_process(process)

    def test_wake_bridge_crash_restarts_without_dropping_listener(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            listener = root / "listener.py"
            listener.write_text("import time\ntime.sleep(30)\n", encoding="utf-8")
            inbox = root / "inbox.jsonl"
            inbox.touch()
            process = subprocess.Popen(
                [
                    "python3", str(SKILL_DIR / "runtime_agent.py"),
                    "--inbox", str(inbox),
                    "--worker-cursor", str(root / "cursor.json"),
                    "--health-file", str(root / "health.json"),
                    "--exec", "true", "--", "python3", str(listener),
                ],
                env=isolated_runtime_env(root),
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            try:
                def children():
                    raw = Path(
                        f"/proc/{process.pid}/task/{process.pid}/children"
                    ).read_text().split()
                    return [int(value) for value in raw]

                deadline = time.monotonic() + 3
                old_consumer = old_listener = None
                while time.monotonic() < deadline:
                    for child in children():
                        cmd = Path(f"/proc/{child}/cmdline").read_bytes()
                        if b"runtime_consumer.py" in cmd:
                            old_consumer = child
                        elif b"listener.py" in cmd:
                            old_listener = child
                    if old_consumer and old_listener:
                        break
                    time.sleep(0.05)
                self.assertIsNotNone(old_consumer)
                self.assertIsNotNone(old_listener)
                os.kill(old_consumer, signal.SIGKILL)

                replacement = None
                deadline = time.monotonic() + 4
                while time.monotonic() < deadline:
                    for child in children():
                        cmd = Path(f"/proc/{child}/cmdline").read_bytes()
                        if b"runtime_consumer.py" in cmd and child != old_consumer:
                            replacement = child
                    if replacement:
                        break
                    time.sleep(0.05)
                self.assertIsNotNone(replacement)
                self.assertTrue(Path(f"/proc/{old_listener}").exists())
            finally:
                stop_process(process)

    def test_persistent_readiness_failure_starts_listener_without_crashing(self):
        # FIX 1: an uncaught readiness failure used to escape main() and kill
        # the supervisor before the listener ever started (systemd restart
        # loop, agent never ONLINE). The listener must come up and the
        # supervisor must survive even when the adapter fails its handshake.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            never_ready = root / "adapter-health.json"
            listener_started = root / "listener-started"
            adapter = root / "adapter.py"
            adapter.write_text(
                "import sys\n"
                "sys.stderr.write('ADAPTER-BOOM readiness never signaled\\n')\n"
                "sys.stderr.flush()\n"
                "sys.exit(1)\n",
                encoding="utf-8",
            )
            listener = root / "listener.py"
            listener.write_text(
                "import pathlib, sys, time\n"
                "pathlib.Path(sys.argv[1]).write_text(str(time.time_ns()))\n"
                "time.sleep(30)\n",
                encoding="utf-8",
            )
            inbox = root / "inbox.jsonl"
            inbox.touch()
            process = subprocess.Popen(
                [
                    "python3", str(SKILL_DIR / "runtime_agent.py"),
                    "--inbox", str(inbox),
                    "--worker-cursor", str(root / "cursor.json"),
                    "--health-file", str(root / "health.json"),
                    "--persistent-exec",
                    f"exec python3 {shlex.quote(str(adapter))}",
                    "--persistent-ready-file", str(never_ready),
                    "--ready-timeout-seconds", "0.5",
                    "--", "python3", str(listener), str(listener_started),
                ],
                env=isolated_runtime_env(root),
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            try:
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline and not listener_started.exists():
                    time.sleep(0.05)
                self.assertTrue(
                    listener_started.exists(),
                    "listener must start even when the adapter fails readiness",
                )
                # The readiness failure must not have crashed the supervisor.
                self.assertIsNone(process.poll())
            finally:
                stop_process(process)

    def test_persistent_adapter_crash_loop_escalates_to_terminal(self):
        # FIX 2: a v2 adapter that keeps failing used to restart forever. After
        # the crash-loop budget it must escalate to a terminal diagnostic that
        # names the child's stderr tail and stop churning — while the listener
        # stays up so presence is honest.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            health = root / "health.json"
            listener_started = root / "listener-started"
            adapter = root / "adapter.py"
            adapter.write_text(
                "import sys\n"
                "sys.stderr.write('SANDBOX-SENTINEL netns denied\\n')\n"
                "sys.stderr.flush()\n"
                "sys.exit(1)\n",
                encoding="utf-8",
            )
            listener = root / "listener.py"
            listener.write_text(
                "import pathlib, sys, time\n"
                "pathlib.Path(sys.argv[1]).write_text('up')\n"
                "time.sleep(30)\n",
                encoding="utf-8",
            )
            inbox = root / "inbox.jsonl"
            inbox.touch()
            process = subprocess.Popen(
                [
                    "python3", str(SKILL_DIR / "runtime_agent.py"),
                    "--inbox", str(inbox),
                    "--worker-cursor", str(root / "cursor.json"),
                    "--health-file", str(health),
                    "--persistent-exec",
                    f"exec python3 {shlex.quote(str(adapter))}",
                    "--adapter-max-restarts", "1",
                    "--adapter-failure-window-seconds", "300",
                    "--", "python3", str(listener), str(listener_started),
                ],
                env=isolated_runtime_env(root),
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            try:
                terminal = None
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline:
                    if health.is_file():
                        try:
                            state = json.loads(health.read_text(encoding="utf-8"))
                        except (OSError, ValueError):
                            state = {}
                        if state.get("adapter_terminal"):
                            terminal = state
                            break
                    time.sleep(0.1)
                self.assertIsNotNone(
                    terminal, "adapter crash-loop must escalate to terminal"
                )
                self.assertIn(
                    "SANDBOX-SENTINEL",
                    terminal.get("last_adapter_stderr", ""),
                )
                self.assertIn("budget exhausted", terminal.get("last_error", ""))
                # Presence stays honest: listener up and supervisor alive.
                self.assertTrue(listener_started.exists())
                self.assertIsNone(process.poll())
            finally:
                stop_process(process)

    def run_spine_case(self, *, room_lead=None, room_event=None):
        server = ThreadingHTTPServer(("127.0.0.1", 0), SpineServer)
        server.room_lead = room_lead
        if room_event is not None:
            server.room_event = room_event
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        process = None
        try:
            with tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                identity = root / "worker.env"
                history = root / "messages.jsonl"
                inbox = root / "inbox.jsonl"
                received_cursor = root / "received.json"
                worker_cursor = root / "acked.json"
                health = root / "health.json"
                hook_output = root / "hook.jsonl"
                hook = root / "hook.py"
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
                hook.write_text(
                    """import os, pathlib, sys
path = pathlib.Path(sys.argv[1])
with path.open("a", encoding="utf-8") as out:
    out.write(sys.stdin.read().strip() + "\\n")
""",
                    encoding="utf-8",
                )
                url = (
                    f"ws://127.0.0.1:{server.server_port}/ws?"
                    "room=reviewer&name=worker&type=agent&filter=mentions"
                )
                listener = [
                    "python3",
                    str(SKILL_DIR / "listen.py"),
                    url,
                    str(history),
                    "--skip-own",
                    "worker",
                    "--cursor",
                    str(received_cursor),
                    "--turn-log",
                    str(inbox),
                ]
                env = dict(os.environ)
                env["LOCA_ENV"] = str(identity)
                process = subprocess.Popen(
                    [
                        "python3",
                        str(SKILL_DIR / "runtime_agent.py"),
                        "--inbox",
                        str(inbox),
                        "--worker-cursor",
                        str(worker_cursor),
                        "--health-file",
                        str(health),
                        "--exec",
                        shlex.join(["python3", str(hook), str(hook_output)]),
                        "--",
                        *listener,
                    ],
                    env=env,
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                )
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline:
                    if health.is_file():
                        state = json.loads(health.read_text(encoding="utf-8"))
                        if state.get("ack") == "OK":
                            break
                    time.sleep(0.05)
                else:
                    self.fail("runtime spine did not ACK the delivered turn")

                calls = hook_output.read_text(encoding="utf-8").splitlines()
                self.assertEqual(len(calls), 1)
                delivered = json.loads(calls[0])
                self.assertEqual(delivered["protocol_version"], "1")
                acked = json.loads(worker_cursor.read_text(encoding="utf-8"))
                self.assertEqual(
                    acked["last_acked_delivery_id"]
                    if "last_acked_delivery_id" in acked
                    else acked["delivery_id"],
                    delivered["delivery_id"],
                )
                self.assertEqual(
                    state["last_acked_delivery_id"],
                    delivered["delivery_id"],
                )
                return delivered
        finally:
            if process is not None and process.poll() is None:
                stop_process(process)
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)

    def test_delivery_runs_once_and_acks_only_after_hook_success(self):
        delivered = self.run_spine_case()
        self.assertEqual(delivered["delivery_id"], "reviewer:301")
        self.assertEqual(delivered["priority"], "direct_user")

    def test_lead_room_message_reaches_runtime_and_is_acked(self):
        delivered = self.run_spine_case(
            room_lead="worker",
            room_event={
                "t": "msg",
                "message": {
                    "id": 302,
                    "room": "reviewer",
                    "sender": "debug",
                    "sender_type": "agent",
                    "target": "assistant-dev",
                    "text": "room-wide status for the lead",
                    "ts": 2,
                },
            },
        )
        self.assertEqual(delivered["delivery_id"], "reviewer:302")
        self.assertEqual(delivered["priority"], "lead_room")
        self.assertEqual(
            delivered["event"]["text"],
            "room-wide status for the lead",
        )


if __name__ == "__main__":
    unittest.main()
