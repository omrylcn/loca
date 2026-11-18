"""Codex v2: a Building notification wakes the thread this identity used last.

A notification belongs to no loca and Codex keeps one thread per loca, so the
store routes it to the most recently used thread as a no-reply attention; the
prompt says what woke the agent. With no thread yet it stays context for the
next loca turn.
"""
import json
import sys
import tempfile
import unittest
from pathlib import Path

SKILL_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SKILL_DIR))

from attention_store import AttentionStore  # noqa: E402
from codex_adapter_v2 import attention_prompt  # noqa: E402

SERVER = "https://loca.example"


def notification(notice_id: int, title: str = "kit update") -> dict:
    notice = {"id": notice_id, "title": title, "body": "restart your Monitor",
              "from": "omrylcn", "created_at": notice_id * 1000, "scope": "building"}
    return {
        "protocol_version": "1",
        "delivery_id": f"notification:{notice_id}",
        "server": SERVER,
        "room": "",
        "identity": "reviewer",
        "priority": "notification",
        "received_at_ms": notice_id * 1000,
        "last_id": None,
        "event": {"t": "notification", "notification_context": {
            "recipient": "reviewer", "items": [notice], "more_available": False}},
    }


def append(inbox: Path, record: dict) -> None:
    with inbox.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(record) + "\n")


def add_thread(store: AttentionStore, room: str, updated_at_ms: int) -> None:
    with store.connect() as connection:
        connection.execute(
            "INSERT INTO threads (server, room, identity, thread_id, lease_epoch, created_at_ms, updated_at_ms)"
            " VALUES (?, ?, 'reviewer', ?, 1, ?, ?)",
            (SERVER, room, f"thread-{room}", updated_at_ms, updated_at_ms),
        )


class CodexNotificationWakeTests(unittest.TestCase):
    def store(self, root: Path):
        inbox = root / "inbox.v2.jsonl"
        store = AttentionStore(root / "state.sqlite3")
        store.initialize_ingestion_source(inbox, "reviewer")
        return inbox, store

    def test_with_no_thread_it_stays_context_for_the_next_loca_turn(self):
        with tempfile.TemporaryDirectory() as tmp:
            inbox, store = self.store(Path(tmp))
            append(inbox, notification(5))
            result = store.ingest_inbox(inbox, "reviewer")
            self.assertEqual(result["inserted"], 0)
            self.assertEqual(result["context_only"], 1)
            self.assertEqual(store.snapshot("reviewer")["attentions"], [])

    def test_it_wakes_the_thread_used_last_without_requiring_a_reply(self):
        with tempfile.TemporaryDirectory() as tmp:
            inbox, store = self.store(Path(tmp))
            add_thread(store, "older-loca", 1_000)
            add_thread(store, "busy-loca", 2_000)
            append(inbox, notification(6))
            result = store.ingest_inbox(inbox, "reviewer")
            self.assertEqual(result["inserted"], 1)
            [attention] = store.snapshot("reviewer")["attentions"]
            self.assertEqual(attention["room"], "busy-loca")
            self.assertEqual(attention["priority"], "notification")
            self.assertEqual(attention["delivery_id"], "notification:6")
            self.assertEqual(int(attention["reply_required"]), 0)
            # The same delivery again is a duplicate, not a second wake.
            append(inbox, notification(6))
            self.assertEqual(store.ingest_inbox(inbox, "reviewer")["duplicate"], 1)

    def test_the_prompt_says_a_notification_woke_the_agent_and_carries_it(self):
        with tempfile.TemporaryDirectory() as tmp:
            inbox, store = self.store(Path(tmp))
            add_thread(store, "busy-loca", 2_000)
            append(inbox, notification(7, "kit update"))
            store.ingest_inbox(inbox, "reviewer")
            [attention] = store.snapshot("reviewer")["attentions"]
            prompt = attention_prompt("reviewer", dict(attention), [])
            self.assertIn("A Building notification woke you", prompt)
            self.assertIn("kit update", prompt)
            self.assertIn("do not acknowledge or repeat it in the room", prompt)
            self.assertNotIn("A user-visible reply is required", prompt)


if __name__ == "__main__":
    unittest.main()
