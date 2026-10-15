import sys
from copy import deepcopy
from pathlib import Path
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from wiki_context import ContextBudgetExceeded, WikiPageContext


def snapshot(revision=1, body="## Constraint\nold rule\n## Unrelated\nunchanged\n"):
    return {"room": "a", "revision": revision, "pages": [
        {"slug": "deploy", "title": "Deploy", "body": body, "sources": [1], "revision": revision},
        {"slug": "ui", "title": "UI", "body": "private unrelated body", "sources": [], "revision": 1}]}


class WikiPageContextTests(unittest.TestCase):
    def baseline(self):
        ctx = WikiPageContext("a", "thread-a")
        packet = ctx.prepare(snapshot(), ["deploy"])
        self.assertTrue(ctx.acknowledge_injection(packet["packet_id"], "a", "thread-a"))
        return ctx

    def test_delta_contains_changed_section_not_whole_page_or_unrelated_topic(self):
        ctx = self.baseline()
        packet = ctx.prepare(snapshot(2, snapshot()["pages"][0]["body"].replace("old rule", "new rule")), ["deploy"])
        delta = packet["pages"][0]
        self.assertEqual(delta["kind"], "delta")
        self.assertNotIn("body", delta)
        self.assertEqual(delta["sections"], [{"operation": "replace", "start_line": 1,
                                             "end_line": 2, "removed": "old rule\n", "inserted": "new rule\n"}])
        self.assertEqual(list(ctx.injected_pages), ["deploy"])

    def test_page_removal_is_explicit_and_fences_pending_old_content(self):
        ctx = self.baseline()
        old = ctx.prepare(snapshot(2, "old pending update"), ["deploy"])
        removed = {"room": "a", "revision": 3, "pages": [], "missing": ["deploy"]}
        packet = ctx.prepare(removed, ["deploy"])
        self.assertEqual(packet["pages"][0]["kind"], "removed")
        self.assertIn("deploy", ctx.injected_pages)
        self.assertFalse(ctx.acknowledge_injection(old["packet_id"], "a", "thread-a"))
        self.assertTrue(ctx.acknowledge_injection(packet["packet_id"], "a", "thread-a"))
        self.assertNotIn("deploy", ctx.injected_pages)
        self.assertIsNone(ctx.prepare(removed, ["deploy"]))

    def test_removed_decision_is_explicit_and_not_left_as_current(self):
        ctx = self.baseline()
        packet = ctx.prepare(snapshot(2, "## Unrelated\nunchanged\n"), ["deploy"])
        self.assertEqual(packet["pages"][0]["sections"][0]["operation"], "delete")
        self.assertIn("old rule", packet["pages"][0]["sections"][0]["removed"])

    def test_transport_offer_and_index_revision_do_not_ack_unread_page(self):
        ctx = WikiPageContext("a", "thread-a")
        ctx.prepare(snapshot(), ["deploy"])
        self.assertEqual(ctx.injected_pages, {})
        packet = ctx.prepare(snapshot(), ["ui"])
        ctx.acknowledge_injection(packet["packet_id"], "a", "thread-a")
        self.assertNotIn("deploy", ctx.injected_pages)

    def test_compaction_bootstraps_instead_of_applying_delta_to_missing_baseline(self):
        ctx = self.baseline()
        ctx.reset("thread-b")
        packet = ctx.prepare(snapshot(2, "new content"), ["deploy"])
        self.assertEqual(packet["pages"][0]["kind"], "bootstrap")
        self.assertFalse(ctx.acknowledge_injection(packet["packet_id"], "a", "thread-a"))

    def test_newer_offer_fences_older_response(self):
        ctx = self.baseline()
        older = ctx.prepare(snapshot(2, "revision two"), ["deploy"])
        newer = ctx.prepare(snapshot(3, "revision three"), ["deploy"])
        self.assertFalse(ctx.acknowledge_injection(older["packet_id"], "a", "thread-a"))
        self.assertTrue(ctx.acknowledge_injection(newer["packet_id"], "a", "thread-a"))
        self.assertFalse(ctx.acknowledge_injection(newer["packet_id"], "a", "thread-a"))
        self.assertIsNone(ctx.prepare(snapshot(3, "revision three"), ["deploy"]))

    def test_same_thread_compaction_fences_late_ack_and_requires_bootstrap(self):
        ctx = self.baseline()
        old = ctx.prepare(snapshot(2, "revision two"), ["deploy"])
        ctx.reset("thread-a")
        new = ctx.prepare(snapshot(2, "revision two"), ["deploy"])
        self.assertEqual(new["pages"][0]["kind"], "bootstrap")
        self.assertNotEqual(old["packet_id"], new["packet_id"])
        self.assertFalse(ctx.acknowledge_injection(old["packet_id"], "a", "thread-a"))
        self.assertTrue(ctx.acknowledge_injection(new["packet_id"], "a", "thread-a"))

    def test_room_stale_content_missing_page_and_budget_fail_without_false_injection(self):
        ctx = self.baseline()
        wrong = deepcopy(snapshot()); wrong["room"] = "other"
        with self.assertRaises(ValueError): ctx.prepare(wrong, ["deploy"])
        with self.assertRaises(ValueError): ctx.prepare(snapshot(0), ["deploy"])
        with self.assertRaises(ValueError): ctx.prepare(snapshot(1, "rewritten"), ["deploy"])
        with self.assertRaises(KeyError): ctx.prepare(snapshot(), ["missing"])
        with self.assertRaises(ContextBudgetExceeded): ctx.prepare(snapshot(2, "x"*9000), ["deploy"])
        self.assertEqual(ctx.injected_pages["deploy"]["revision"], 1)
        self.assertFalse(ctx.pending)

    def test_unchanged_page_is_not_resent_for_an_unrelated_global_revision(self):
        ctx = self.baseline()
        value = snapshot(); value["revision"] = 99
        self.assertIsNone(ctx.prepare(value, ["deploy"]))

    def test_offer_does_not_mutate_sources(self):
        value = snapshot()
        before = deepcopy(value)
        ctx = WikiPageContext("a", "t")
        packet = ctx.prepare(value, ["deploy"])
        packet["pages"][0]["sources"].append(9)
        self.assertEqual(value, before)
        self.assertTrue(ctx.acknowledge_injection(packet["packet_id"], "a", "t"))
        self.assertEqual(ctx.injected_pages["deploy"]["sources"], [1])

    def test_unacknowledged_offers_are_bounded_without_discarding_ack_state(self):
        ctx = WikiPageContext("a","t",max_pages=1)
        first = ctx.prepare(snapshot(),["deploy"])
        with self.assertRaises(ContextBudgetExceeded): ctx.prepare(snapshot(),["ui"])
        self.assertEqual(len(ctx.pending),1)
        self.assertTrue(ctx.acknowledge_injection(first["packet_id"],"a","t"))

    def test_evicted_page_requires_bootstrap_not_delta_against_unknown_baseline(self):
        ctx = WikiPageContext("a","t",max_pages=1)
        first=ctx.prepare(snapshot(),["deploy"])
        ctx.acknowledge_injection(first["packet_id"],"a","t")
        second=ctx.prepare(snapshot(),["ui"])
        ctx.acknowledge_injection(second["packet_id"],"a","t")
        self.assertEqual(list(ctx.injected_pages),["ui"])
        self.assertEqual(ctx.prepare(snapshot(2),["deploy"])["pages"][0]["kind"],"bootstrap")


if __name__ == "__main__":
    unittest.main()
