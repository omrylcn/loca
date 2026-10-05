import importlib.util
from pathlib import Path
import sys
import unittest

spec = importlib.util.spec_from_file_location(
    "wiki_state", Path(__file__).resolve().parents[1] / "wiki_state.py"
)
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)
WikiContext = module.WikiContext


class WikiContextTests(unittest.TestCase):
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
