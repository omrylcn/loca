from copy import deepcopy
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from wiki_coordinator import WikiCoordinator


class WikiCoordinatorTests(unittest.TestCase):
    def setup_runner(self, worker):
        self.writes, self.inputs = [], []
        self.batch = {"room": "a", "after_id": 0, "through_id": 3, "lease": {"batch_id": 7, "epoch": 2},
                      "messages": [{"room": "a", "id": i, "text": "untrusted source"} for i in (1, 2, 3)]}
        self.snapshot = {"room": "a", "revision": 0, "reviewed_through": 0, "pages": []}
        def delegate(payload):
            self.inputs.append(deepcopy(payload))
            return worker(payload)
        def finish(value):
            self.writes.append(value)
            return {"revision": 1}
        return WikiCoordinator("a", lambda: self.batch, lambda: self.snapshot, finish, delegate)

    def review(self, _payload=None):
        return {"expected_revision": 0, "reviewed_through": 3, "reason": "sourced decision",
                "pages": [{"slug": "overview", "title": "Overview", "body": "supported finding", "sources": [2]}]}

    def test_delegated_complete_input_and_principal_transport_finish(self):
        runner = self.setup_runner(self.review)
        self.assertEqual(runner.run_once()["status"], "accepted")
        self.assertEqual(len(self.inputs[0]["messages"]), 3)
        self.assertNotIn("lease", self.inputs[0])
        self.assertIn("separate Loca Wiki worker", self.inputs[0]["instructions"])
        self.assertEqual(self.writes[0]["lease"], self.batch["lease"])

    def test_no_delegation_never_claims_or_publishes(self):
        def forbidden(): self.fail("unsupported runtime claimed a lease")
        runner = WikiCoordinator("a", forbidden, forbidden, forbidden, None)
        self.assertEqual(runner.run_once()["status"], "blocked")

    def test_worker_cannot_claim_newer_cursor_or_unseen_sources(self):
        for mutation in (lambda review: review.update(reviewed_through=4),
                         lambda review: review["pages"][0].update(sources=[99])):
            def worker(_payload):
                result = self.review(); mutation(result); return result
            runner = self.setup_runner(worker)
            with self.assertRaises(ValueError): runner.run_once()
            self.assertEqual(self.writes, [])

    def test_wrong_room_and_changed_cursor_fail_before_worker(self):
        runner = self.setup_runner(self.review)
        self.snapshot["room"] = "private-b"
        with self.assertRaises(ValueError): runner.run_once()
        self.assertFalse(self.inputs)
        self.snapshot["room"] = "a"; self.snapshot["reviewed_through"] = 1
        with self.assertRaises(ValueError): runner.run_once()
        self.assertFalse(self.writes)

    def test_no_change_is_accepted_without_chat_output(self):
        def worker(_payload): result = self.review(); result["pages"] = []; return result
        runner = self.setup_runner(worker)
        self.assertEqual(runner.run_once()["status"], "accepted")
        self.assertEqual(self.writes[0]["review"]["pages"], [])

    def test_worker_failure_does_not_publish_or_fallback_to_coordinator(self):
        def worker(_payload): raise RuntimeError("worker unavailable")
        runner = self.setup_runner(worker)
        with self.assertRaises(RuntimeError): runner.run_once()
        self.assertEqual(len(self.inputs), 1)
        self.assertFalse(self.writes)

    def test_worker_cannot_publish_unsourced_content_or_malformed_pages(self):
        for mutation in (lambda value:value["pages"][0].update(sources=[]),
                         lambda value:value.update(pages=["not a page"]),
                         lambda value:value.update(expected_revision=False),
                         lambda value:value.update(reason="x"*4097)):
            def worker(_payload):
                result=self.review(); mutation(result); return result
            runner=self.setup_runner(worker)
            with self.assertRaises(ValueError): runner.run_once()
            self.assertFalse(self.writes)

    def test_worker_input_budget_fails_before_delegation_without_truncation(self):
        runner=self.setup_runner(self.review)
        self.batch["messages"][0]["text"]="x"*256000
        with self.assertRaisesRegex(ValueError,"nothing omitted"): runner.run_once()
        self.assertFalse(self.inputs)
        self.assertFalse(self.writes)

    def test_worker_operational_keys_are_not_sent_to_publication(self):
        def worker(_payload):
            result=self.review()
            result.update(lease={"epoch":99},target="other room",command="deploy")
            result["pages"][0].update(command="execute",editor_principal="forged")
            return result
        runner=self.setup_runner(worker)
        runner.run_once()
        self.assertEqual(set(self.writes[0]),{"lease","review"})
        self.assertEqual(set(self.writes[0]["review"]["pages"][0]),{"slug","title","body","sources"})
        self.assertEqual(self.writes[0]["lease"]["epoch"],2)

    def test_invalid_publication_receipt_is_not_reported_as_accepted(self):
        for value in ({},{"revision":True},{"revision":"1"},{"revision":99}):
            runner=self.setup_runner(self.review)
            runner.finish=lambda _payload:value
            with self.assertRaisesRegex(ValueError,"unverified"): runner.run_once()


if __name__ == "__main__": unittest.main()
