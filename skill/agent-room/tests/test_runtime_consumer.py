import importlib.util
import json
import shlex
import stat
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock


SKILL_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SKILL_DIR))
SPEC = importlib.util.spec_from_file_location(
    "loca_runtime_consumer", SKILL_DIR / "runtime_consumer.py"
)
assert SPEC and SPEC.loader
CONSUMER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CONSUMER)


def append(path, record):
    with path.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(record) + "\n")


class RuntimeConsumerTests(unittest.TestCase):
    def test_failed_wake_is_not_acked_and_successful_retry_is(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inbox = root / "inbox.jsonl"
            cursor = root / "cursor.json"
            append(
                inbox,
                {
                    "protocol_version": 1,
                    "delivery_id": "sb-dev:81",
                    "priority": "direct_user",
                    "room": "sb-dev",
                    "identity": "lead-engineer",
                    "last_id": 81,
                    "event": {
                        "id": 81,
                        "sender": "operator",
                        "sender_type": "user",
                        "target": "lead-engineer",
                        "text": "hey",
                    },
                },
            )

            with mock.patch.object(
                CONSUMER,
                "run_delivery",
                side_effect=subprocess.CalledProcessError(1, "hook"),
            ):
                with self.assertRaises(subprocess.CalledProcessError):
                    CONSUMER.process_one(
                        inbox,
                        cursor,
                        "hook",
                        wait_seconds=0,
                        poll_seconds=0.01,
                        timeout_seconds=1,
                    )

            replay = CONSUMER.orchestrator_queue.next_record(
                inbox, cursor, 0, 0.01
            )
            self.assertEqual(replay["delivery_id"], "sb-dev:81")

            with mock.patch.object(CONSUMER, "run_delivery"):
                completed = CONSUMER.process_one(
                    inbox,
                    cursor,
                    "hook",
                    wait_seconds=0,
                    poll_seconds=0.01,
                    timeout_seconds=1,
                    attempt=2,
                )
            self.assertEqual(completed["delivery_id"], "sb-dev:81")
            self.assertIsNone(
                CONSUMER.orchestrator_queue.next_record(
                    inbox, cursor, 0, 0.01
                )
            )

    def test_retry_uses_stable_idempotency_key_and_attempt_number(self):
        record = {
            "protocol_version": 1,
            "delivery_id": "reviewer:44",
            "priority": "direct_user",
            "room": "reviewer",
            "last_id": 44,
            "event": {
                "id": 44,
                "sender": "operator",
                "sender_type": "user",
                "target": "reviewer",
                "text": "incele",
            },
        }
        first = CONSUMER.delivery_environment(record, 1)
        retry = CONSUMER.delivery_environment(record, 2)
        self.assertEqual(first["LOCA_OP_ID"], "loca-reviewer:44")
        self.assertEqual(first["LOCA_OP_ID"], retry["LOCA_OP_ID"])
        self.assertEqual(first["LOCA_ATTEMPT"], "1")
        self.assertEqual(retry["LOCA_ATTEMPT"], "2")
        self.assertEqual(first["LOCA_PROTOCOL_VERSION"], "1")

    def test_second_consumer_cannot_claim_the_same_identity_cursor(self):
        with tempfile.TemporaryDirectory() as tmp:
            cursor = Path(tmp) / "worker.json"
            first = CONSUMER.acquire_consumer_lock(cursor)
            try:
                with self.assertRaisesRegex(
                    RuntimeError, "another consumer already owns"
                ):
                    CONSUMER.acquire_consumer_lock(cursor)
            finally:
                first.close()

    def test_command_receives_full_v1_envelope_and_legacy_event_env(self):
        record = {
            "protocol_version": "1",
            "delivery_id": "reviewer:45",
            "priority": "direct_user",
            "room": "reviewer",
            "identity": "reviewer",
            "last_id": 45,
            "event": {
                "id": 45,
                "sender": "operator",
                "sender_type": "user",
                "target": "reviewer",
                "text": "bak",
            },
        }
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "env.json"
            adapter = Path(tmp) / "adapter.py"
            adapter.write_text(
                """import json, os, pathlib, sys
pathlib.Path(sys.argv[1]).write_text(json.dumps({
  "stdin": sys.stdin.read(),
  "msg": os.environ["LOCA_MSG"],
  "delivery": os.environ["LOCA_DELIVERY"],
}), encoding="utf-8")
""",
                encoding="utf-8",
            )
            CONSUMER.run_delivery(
                f"{sys.executable} {adapter} {output}", record, 1, 10
            )
            call = json.loads(output.read_text(encoding="utf-8"))
        self.assertEqual(json.loads(call["stdin"])["delivery_id"], "reviewer:45")
        self.assertEqual(json.loads(call["msg"])["id"], 45)
        self.assertEqual(json.loads(call["delivery"])["protocol_version"], "1")

    def test_health_state_is_atomic_and_private(self):
        with tempfile.TemporaryDirectory() as tmp:
            health = Path(tmp) / "run" / "worker.health.json"
            CONSUMER.save_health(
                health,
                {
                    "delivery": "OK",
                    "wake": "RUNNING",
                    "ack": "PENDING",
                },
            )
            self.assertEqual(
                json.loads(health.read_text(encoding="utf-8"))["wake"],
                "RUNNING",
            )
            self.assertEqual(stat.S_IMODE(health.stat().st_mode), 0o600)
            self.assertEqual(stat.S_IMODE(health.parent.stat().st_mode), 0o700)

    def test_server_accepted_reply_releases_queue_before_turn_finishes(self):
        record = {
            "protocol_version": "1",
            "delivery_id": "reviewer:88",
            "room": "reviewer",
            "identity": "reviewer",
            "last_id": 88,
            "event": {
                "id": 88,
                "room": "reviewer",
                "sender": "operator",
                "text": "answer now",
            },
        }
        with tempfile.TemporaryDirectory() as tmp:
            adapter = Path(tmp) / "adapter.py"
            adapter.write_text(
                """import os, pathlib, time
pathlib.Path(os.environ["LOCA_REPLY_RECEIPT"]).write_text("accepted\\n")
time.sleep(1.0)
""",
                encoding="utf-8",
            )
            replies = []
            started = time.monotonic()
            CONSUMER.run_delivery(
                f"{sys.executable} {adapter}",
                record,
                1,
                10,
                on_reply=lambda: replies.append("OK"),
            )
            elapsed = time.monotonic() - started
            self.assertLess(elapsed, 0.7)
            self.assertEqual(replies, ["OK"])

    def test_unanswered_room_turn_yields_to_new_direct_operator_call(self):
        room_record = {
            "protocol_version": "1",
            "delivery_id": "sb-dev:90",
            "priority": "broadcast",
            "room": "sb-dev",
            "identity": "lead-engineer",
            "last_id": 90,
            "event": {
                "id": 90,
                "room": "sb-dev",
                "sender": "operator",
                "sender_type": "user",
                "target": "all",
                "text": "continue the long build",
            },
        }
        with tempfile.TemporaryDirectory() as tmp:
            started_marker = Path(tmp) / "adapter-started"
            adapter = Path(tmp) / "adapter.py"
            adapter.write_text(
                """import os, pathlib, sys, time
pathlib.Path(sys.argv[1]).write_text("started\\n")
pathlib.Path(os.environ["LOCA_WAKE_RECEIPT"]).write_text("accepted\\n")
time.sleep(0.4)
""",
                encoding="utf-8",
            )
            started = time.monotonic()
            polls = 0

            def direct_waiting():
                nonlocal polls
                polls += 1
                return (
                    "sb-dev:91"
                    if polls >= 2 and started_marker.is_file()
                    else ""
                )

            with self.assertRaises(CONSUMER.DeliveryYielded) as raised:
                CONSUMER.run_delivery(
                    f"{sys.executable} {adapter} {started_marker}",
                    room_record,
                    1,
                    10,
                    urgent_delivery=direct_waiting,
                )
            elapsed = time.monotonic() - started
            self.assertLess(elapsed, 0.7)
            self.assertEqual(raised.exception.delivery_id, "sb-dev:91")
            # The yielded adapter is intentionally reaped in the background.
            # Keep its script directory alive until that process exits.
            time.sleep(0.5)

    def test_reply_receipt_wins_over_simultaneous_urgent_delivery(self):
        record = {
            "protocol_version": "1",
            "delivery_id": "sb-dev:92",
            "priority": "broadcast",
            "room": "sb-dev",
            "identity": "lead-engineer",
            "last_id": 92,
            "event": {"id": 92, "room": "sb-dev", "text": "status"},
        }
        with tempfile.TemporaryDirectory() as tmp:
            adapter = Path(tmp) / "adapter.py"
            adapter.write_text(
                """import os, pathlib, time
pathlib.Path(os.environ["LOCA_REPLY_RECEIPT"]).write_text("accepted\\n")
time.sleep(0.5)
""",
                encoding="utf-8",
            )
            replies = []
            started = time.monotonic()
            CONSUMER.run_delivery(
                f"{sys.executable} {adapter}",
                record,
                1,
                10,
                on_reply=lambda: replies.append("OK"),
                urgent_delivery=lambda: (
                    "sb-dev:93" if time.monotonic() - started >= 0.2 else ""
                ),
            )
            self.assertEqual(replies, ["OK"])

    def test_urgent_probe_reads_only_complete_new_records(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inbox = root / "inbox.jsonl"
            cursor = root / "cursor.json"
            append(
                inbox,
                {
                    "protocol_version": "1",
                    "delivery_id": "sb-dev:94",
                    "priority": "broadcast",
                    "room": "sb-dev",
                    "identity": "lead-engineer",
                    "last_id": 94,
                    "event": {"id": 94, "room": "sb-dev", "text": "work"},
                },
            )
            current = CONSUMER.orchestrator_queue.next_record(
                inbox, cursor, 0, 0.01
            )
            probe = CONSUMER.UrgentInboxProbe(inbox, current)
            append(
                inbox,
                {
                    "protocol_version": "1",
                    "delivery_id": "sb-dev:95",
                    "priority": "informational",
                    "room": "sb-dev",
                    "last_id": 95,
                    "event": {"id": 95, "room": "sb-dev", "text": "noise"},
                },
            )
            self.assertEqual(probe.next_delivery_id(), "")

            direct = json.dumps(
                {
                    "protocol_version": "1",
                    "delivery_id": "sb-dev:96",
                    "priority": "direct_user",
                    "room": "sb-dev",
                    "last_id": 96,
                    "event": {
                        "id": 96,
                        "room": "sb-dev",
                        "sender": "operator",
                        "sender_type": "user",
                        "target": "lead-engineer",
                        "text": "hey",
                    },
                }
            ).encode()
            with inbox.open("ab") as handle:
                handle.write(direct)
            partial_offset = probe.offset
            self.assertEqual(probe.next_delivery_id(), "")
            self.assertEqual(probe.offset, partial_offset)
            with inbox.open("ab") as handle:
                handle.write(b"\n")
            self.assertEqual(probe.next_delivery_id(), "sb-dev:96")

    def test_urgent_probe_prefers_newest_direct_user_call(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inbox = root / "inbox.jsonl"
            cursor = root / "cursor.json"
            append(
                inbox,
                {
                    "protocol_version": "1",
                    "delivery_id": "sb-dev:97",
                    "priority": "broadcast",
                    "room": "sb-dev",
                    "last_id": 97,
                    "event": {"id": 97, "room": "sb-dev", "text": "work"},
                },
            )
            current = CONSUMER.orchestrator_queue.next_record(
                inbox, cursor, 0, 0.01
            )
            probe = CONSUMER.UrgentInboxProbe(inbox, current)
            append(
                inbox,
                {
                    "protocol_version": "1",
                    "delivery_id": "sb-dev:98",
                    "priority": "direct_user",
                    "room": "sb-dev",
                    "last_id": 98,
                    "event": {"id": 98, "room": "sb-dev", "text": "hey"},
                },
            )
            append(
                inbox,
                {
                    "protocol_version": "1",
                    "delivery_id": "sb-dev:99",
                    "priority": "direct_user",
                    "room": "sb-dev",
                    "last_id": 99,
                    "event": {"id": 99, "room": "sb-dev", "text": "newer"},
                },
            )
            self.assertEqual(probe.next_delivery_id(), "sb-dev:99")

    def test_consumer_launches_direct_call_while_room_adapter_is_in_flight(self):
        """Cover the consumer-to-nudge seam missed by the preemption tests."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inbox = root / "inbox.jsonl"
            cursor = root / "cursor.json"
            health = root / "health.json"
            calls = root / "calls.jsonl"
            room_started = root / "room-started"
            direct_started = root / "direct-started"
            adapter = root / "adapter.py"
            adapter.write_text(
                """import json, os, pathlib, sys, time
calls = pathlib.Path(sys.argv[1])
room_started = pathlib.Path(sys.argv[2])
direct_started = pathlib.Path(sys.argv[3])
delivery = json.loads(os.environ["LOCA_DELIVERY"])
delivery_id = delivery["delivery_id"]
with calls.open("a", encoding="utf-8") as handle:
    handle.write(json.dumps({"event": "start", "id": delivery_id}) + "\\n")
pathlib.Path(os.environ["LOCA_WAKE_RECEIPT"]).write_text("accepted\\n")
if delivery_id == "sb-dev:100":
    room_started.write_text("started\\n")
    deadline = time.monotonic() + 3
    while not direct_started.exists() and time.monotonic() < deadline:
        time.sleep(0.02)
else:
    direct_started.write_text("started\\n")
    pathlib.Path(os.environ["LOCA_REPLY_RECEIPT"]).write_text("accepted\\n")
with calls.open("a", encoding="utf-8") as handle:
    handle.write(json.dumps({"event": "end", "id": delivery_id}) + "\\n")
""",
                encoding="utf-8",
            )
            room_record = {
                "protocol_version": "1",
                "delivery_id": "sb-dev:100",
                "priority": "broadcast",
                "room": "sb-dev",
                "identity": "lead-engineer",
                "last_id": 100,
                "event": {
                    "id": 100,
                    "room": "sb-dev",
                    "sender": "operator",
                    "sender_type": "user",
                    "target": "all",
                    "text": "long room turn",
                },
            }
            direct_record = {
                "protocol_version": "1",
                "delivery_id": "sb-dev:101",
                "priority": "direct_user",
                "room": "sb-dev",
                "identity": "lead-engineer",
                "last_id": 101,
                "event": {
                    "id": 101,
                    "room": "sb-dev",
                    "sender": "operator",
                    "sender_type": "user",
                    "target": "lead-engineer",
                    "text": "answer me now",
                },
            }
            append(inbox, room_record)
            command = " ".join(
                shlex.quote(str(part))
                for part in (
                    sys.executable,
                    adapter,
                    calls,
                    room_started,
                    direct_started,
                )
            )
            consumer = subprocess.Popen(
                [
                    sys.executable,
                    str(SKILL_DIR / "runtime_consumer.py"),
                    "--inbox",
                    str(inbox),
                    "--cursor",
                    str(cursor),
                    "--exec",
                    command,
                    "--wait-seconds",
                    "0.1",
                    "--poll-seconds",
                    "0.02",
                    "--timeout-seconds",
                    "5",
                    "--health-file",
                    str(health),
                    "--preempt-direct-user",
                    "--once",
                ],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )
            deadline = time.monotonic() + 2
            while not room_started.exists() and time.monotonic() < deadline:
                time.sleep(0.02)
            self.assertTrue(room_started.exists(), "room adapter did not start")

            append(inbox, direct_record)
            stdout, stderr = consumer.communicate(timeout=4)
            self.assertEqual(consumer.returncode, 0, stdout + stderr)
            self.assertTrue(direct_started.exists())
            call_log = [
                json.loads(line)
                for line in calls.read_text(encoding="utf-8").splitlines()
            ]
            direct_start = call_log.index(
                {"event": "start", "id": "sb-dev:101"}
            )
            room_end = call_log.index({"event": "end", "id": "sb-dev:100"})
            self.assertLess(direct_start, room_end)
            state = CONSUMER.orchestrator_queue.load_state(cursor)
            self.assertEqual(state["delivery_id"], "sb-dev:101")
            self.assertIn(
                "delivery yielded (not acked): sb-dev:100 -> sb-dev:101",
                stderr,
            )


if __name__ == "__main__":
    unittest.main()
