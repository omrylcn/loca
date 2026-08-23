import importlib.util
import json
import stat
import tempfile
import unittest
from pathlib import Path


SKILL_DIR = Path(__file__).resolve().parents[1]
QUEUE = SKILL_DIR / "orchestrator_queue.py"
SPEC = importlib.util.spec_from_file_location("loca_orchestrator_queue", QUEUE)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def append(path, record):
    with path.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(record) + "\n")


class OrchestratorQueueTests(unittest.TestCase):
    def test_ack_is_durable_across_restart_and_advances_one_turn(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inbox = root / "inbox.jsonl"
            cursor = root / "cursor.json"
            append(
                inbox,
                {
                    "delivery_id": "reviewer:41",
                    "room": "reviewer",
                    "last_id": 41,
                    "event": {"id": 41, "text": "first"},
                },
            )
            append(
                inbox,
                {
                    "delivery_id": "reviewer:44",
                    "room": "reviewer",
                    "last_id": 44,
                    "event": {"t": "turn", "messages": [{"id": 42}, {"id": 44}]},
                },
            )

            first = MODULE.next_record(inbox, cursor, 0, 0.05)
            self.assertEqual(first["delivery_id"], "reviewer:41")
            MODULE.ack_record(
                cursor,
                first["_queue_next_offset"],
                first["room"],
                first["last_id"],
                first["delivery_id"],
            )

            # Reloading from disk models a router/session restart.
            second = MODULE.next_record(inbox, cursor, 0, 0.05)
            self.assertEqual(second["delivery_id"], "reviewer:44")
            self.assertEqual(
                stat.S_IMODE(cursor.stat().st_mode),
                0o600,
            )

    def test_room_watermark_skips_replayed_inbox_record(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inbox = root / "inbox.jsonl"
            cursor = root / "cursor.json"
            MODULE.ack_record(cursor, 0, "iye", 70, "iye:70")
            append(
                inbox,
                {
                    "delivery_id": "iye:70",
                    "room": "iye",
                    "last_id": 70,
                    "event": {"id": 70},
                },
            )
            append(
                inbox,
                {
                    "delivery_id": "iye:71",
                    "room": "iye",
                    "last_id": 71,
                    "event": {"id": 71},
                },
            )

            record = MODULE.next_record(inbox, cursor, 0, 0.05)
            self.assertEqual(record["delivery_id"], "iye:71")
            self.assertGreater(record["_queue_offset"], 0)

    def test_missing_cursor_states_do_not_share_room_maps(self):
        with tempfile.TemporaryDirectory() as tmp:
            one = MODULE.load_state(Path(tmp) / "one.json")
            two = MODULE.load_state(Path(tmp) / "two.json")
            one["rooms"]["iye"] = 9
            self.assertEqual(two["rooms"], {})

    def test_direct_operator_call_bypasses_and_compacts_agent_backlog(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inbox = root / "inbox.jsonl"
            cursor = root / "cursor.json"
            for message_id in range(100, 168):
                append(
                    inbox,
                    {
                        "delivery_id": f"sb-dev:{message_id}",
                        "room": "sb-dev",
                        "last_id": message_id,
                        "event": {
                            "id": message_id,
                            "sender": "assistant-dev",
                            "sender_type": "agent",
                            "target": "all",
                            "text": "@lead-engineer eski durum",
                        },
                    },
                )
            append(
                inbox,
                {
                    "delivery_id": "sb-dev:168",
                    "room": "sb-dev",
                    "last_id": 168,
                    "event": {
                        "id": 168,
                        "sender": "operator",
                        "sender_type": "user",
                        "text": "@lead-engineer hey",
                    },
                },
            )

            record = MODULE.next_record(inbox, cursor, 0, 0.05)

            self.assertEqual(record["delivery_id"], "sb-dev:168")
            self.assertEqual(record["_queue_compacted"], 68)

    def test_newest_operator_call_supersedes_older_operator_calls(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inbox = root / "inbox.jsonl"
            cursor = root / "cursor.json"
            for message_id, text in ((10, "hey"), (11, "durum?")):
                append(
                    inbox,
                    {
                        "delivery_id": f"sb-dev:{message_id}",
                        "room": "sb-dev",
                        "last_id": message_id,
                        "event": {
                            "id": message_id,
                            "sender": "operator",
                            "sender_type": "user",
                            "text": text,
                        },
                    },
                )

            record = MODULE.next_record(inbox, cursor, 0, 0.05)

            self.assertEqual(record["delivery_id"], "sb-dev:11")
            self.assertEqual(record["_queue_compacted"], 1)

    def test_operator_broadcast_is_not_misclassified_as_direct(self):
        record = {
            "protocol_version": 1,
            "delivery_id": "sb-dev:12",
            "identity": "lead-engineer",
            "room": "sb-dev",
            "last_id": 12,
            "event": {
                "id": 12,
                "sender": "operator",
                "sender_type": "user",
                "target": "all",
                "text": "@all durum?",
            },
        }
        self.assertEqual(
            MODULE.record_priority(record),
            MODULE.PRIORITY_RANKS["broadcast"],
        )

    def test_stop_preempts_newer_direct_operator_call(self):
        records = [
            {
                "delivery_id": "sb-dev:control:stop",
                "priority": "security_control",
                "_queue_offset": 0,
                "_queue_next_offset": 50,
                "event": {"t": "control", "cmd": "stop"},
            },
            {
                "delivery_id": "sb-dev:90",
                "priority": "direct_user",
                "_queue_offset": 50,
                "_queue_next_offset": 100,
                "event": {"id": 90, "sender_type": "user"},
            },
        ]
        chosen = MODULE.choose_record(records)
        self.assertEqual(chosen["delivery_id"], "sb-dev:control:stop")

    def test_care_signal_preempts_room_backlog_but_not_direct_operator(self):
        records = [
            {
                "delivery_id": "sb-dev:80",
                "priority": "lead_room",
                "_queue_offset": 0,
                "_queue_next_offset": 50,
                "event": {"id": 80, "sender_type": "agent"},
            },
            {
                "delivery_id": "care:wait-1",
                "priority": "care_signal",
                "_queue_offset": 50,
                "_queue_next_offset": 100,
                "event": {"t": "care", "signal": {"id": "wait-1"}},
            },
            {
                "delivery_id": "sb-dev:81",
                "priority": "direct_user",
                "_queue_offset": 100,
                "_queue_next_offset": 150,
                "event": {"id": 81, "sender_type": "user"},
            },
        ]
        chosen = MODULE.choose_record(records)
        self.assertEqual(chosen["delivery_id"], "sb-dev:81")
        records.pop()
        chosen = MODULE.choose_record(records)
        self.assertEqual(chosen["delivery_id"], "care:wait-1")

    def test_older_task_survives_newer_direct_call_ack(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inbox = root / "inbox.jsonl"
            cursor = root / "cursor.json"
            append(
                inbox,
                {
                    "protocol_version": 1,
                    "delivery_id": "sb-dev:task:7",
                    "priority": "explicit_task",
                    "room": "sb-dev",
                    "last_id": 70,
                    "event": {"t": "task", "task": {"id": 7}},
                },
            )
            append(
                inbox,
                {
                    "protocol_version": 1,
                    "delivery_id": "sb-dev:71",
                    "priority": "direct_user",
                    "room": "sb-dev",
                    "last_id": 71,
                    "event": {
                        "id": 71,
                        "sender": "operator",
                        "sender_type": "user",
                        "target": "lead-engineer",
                    },
                },
            )

            direct = MODULE.next_record(inbox, cursor, 0, 0.05)
            self.assertEqual(direct["delivery_id"], "sb-dev:71")
            self.assertEqual(direct["_queue_next_offset"], 0)
            MODULE.ack_record(
                cursor,
                direct["_queue_next_offset"],
                direct["room"],
                direct["last_id"],
                direct["delivery_id"],
            )

            task = MODULE.next_record(inbox, cursor, 0, 0.05)
            self.assertEqual(task["delivery_id"], "sb-dev:task:7")

    def test_unacked_delivery_is_reoffered_after_restart(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inbox = root / "inbox.jsonl"
            cursor = root / "cursor.json"
            append(
                inbox,
                {
                    "delivery_id": "iye:9",
                    "room": "iye",
                    "last_id": 9,
                    "event": {"id": 9},
                },
            )
            first = MODULE.next_record(inbox, cursor, 0, 0.05)
            second = MODULE.next_record(inbox, cursor, 0, 0.05)
            self.assertEqual(first["delivery_id"], second["delivery_id"])


if __name__ == "__main__":
    unittest.main()
