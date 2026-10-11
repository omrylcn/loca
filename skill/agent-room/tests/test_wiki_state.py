import importlib.util
import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

spec = importlib.util.spec_from_file_location(
    "wiki_state", Path(__file__).resolve().parents[1] / "wiki_state.py"
)
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)
WikiContext = module.WikiContext


class WikiContextTests(unittest.TestCase):
    def test_legacy_context_is_not_agent_work_and_user_text_is_preserved(self):
        event = {"id": 8, "text": "keep this instruction", "owner": "task-owner",
                 "memory": {"owner": None, "short": "obsolete role"},
                 "memory_trigger": "turn", "memory_freshness": "current"}
        public = module.agent_event(event)
        self.assertEqual(public, {"id": 8, "text": "keep this instruction", "owner": "task-owner"})
        self.assertIn("memory", event)  # No mutation of archived source data.
        self.assertIsNone(module.agent_event({"t": "memory", "owner": None}))
        self.assertIsNone(module.agent_event({"t": "memoryversion", "version": 3}))
        self.assertIsNone(module.agent_event({"t": "turn", "messages": [{"t": "memory", "owner": None}]}))
        plain = {"t": "turn", "messages": [{"text": "current instruction"}]}
        self.assertIs(module.agent_event(plain), plain)

    def test_old_inbox_skips_legacy_role_but_keeps_the_direct_call(self):
        import orchestrator_queue
        import tempfile
        with tempfile.TemporaryDirectory() as tmp:
            inbox = Path(tmp) / "inbox"
            cursor = Path(tmp) / "cursor"
            records = [
                {"delivery_id": "r:old", "room": "r", "priority": "addressed_agent",
                 "event": {"t": "memory", "owner": None}},
                {"delivery_id": "r:9", "room": "r", "priority": "direct_user", "last_id": 9,
                 "event": {"id": 9, "text": "@worker reply", "memory": {"owner": None}}},
            ]
            inbox.write_text("".join(json.dumps(row) + "\n" for row in records))
            actual = orchestrator_queue.next_record(inbox, cursor, 0, 0.1)
            self.assertEqual(actual["delivery_id"], "r:9")
            self.assertNotIn("memory", actual["event"])
            self.assertEqual(records[0]["event"]["t"], "memory")

    def test_other_room_and_stale_revision_never_replace_context(self):
        ctx = WikiContext("one", "turn-a")
        self.assertFalse(ctx.offer("two", 8))
        self.assertTrue(ctx.offer("one", 8))
        self.assertFalse(ctx.offer("one", 7))
        self.assertFalse(ctx.injected("one", "old-context", 8))
        self.assertEqual(ctx.injected_revision, -1)
        self.assertTrue(ctx.injected("one", "turn-a", 8))
        self.assertFalse(ctx.offer("one", 8))

    def test_compaction_requires_same_revision_again(self):
        ctx = WikiContext("one", "before")
        ctx.offer("one", 8)
        ctx.injected("one", "before", 8)
        ctx.reset_context("after")
        self.assertTrue(ctx.offer("one", 8))
        self.assertFalse(ctx.injected("one", "before", 8))
        self.assertTrue(ctx.injected("one", "after", 8))

    def test_new_offer_cannot_be_acknowledged_by_old_delivery(self):
        ctx = WikiContext("one", "context")
        ctx.offer("one", 8)
        ctx.offer("one", 9)
        self.assertFalse(ctx.injected("one", "context", 8))
        self.assertTrue(ctx.injected("one", "context", 9))


if __name__ == "__main__":
    unittest.main()
