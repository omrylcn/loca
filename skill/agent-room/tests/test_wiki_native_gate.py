"""Non-inference safety tests for the opt-in native conformance harness."""
import importlib.util
import os
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location("wiki_native_gate",ROOT / "scripts/wiki-native-e2e.py")
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)
sys.path.insert(0,str(ROOT / "skill/agent-room"))
from wiki_context import WikiPageContext


class WikiNativeGateTests(unittest.TestCase):
    def packet(self, context):
        snapshot = {"room":"general","revision":1,"pages":[{
            "slug":"overview","title":"Overview","revision":1,"sources":[1],"body":"old rule"}]}
        return context.prepare(snapshot,["overview"])

    def test_native_environment_has_no_loca_credentials_or_hooks(self):
        with patch.dict(os.environ,{"LOCA_MEMBERSHIP":"fixture", "DAVET_general":"fixture",
                "ANTHROPIC_API_KEY":"unselected", "CODEX_HOME":"existing",
                "CLAUDE_CONFIG_DIR":"existing", "CLAUDECODE":"parent", "AWS_SECRET_ACCESS_KEY":"fixture"}):
            result = gate.environment(Path("/tmp/disposable"))
        for key in ("LOCA_MEMBERSHIP","DAVET_general","ANTHROPIC_API_KEY","CODEX_HOME",
                    "CLAUDE_CONFIG_DIR","CLAUDECODE","AWS_SECRET_ACCESS_KEY"):
            self.assertNotIn(key,result)
        self.assertEqual(result["HOME"],"/tmp/disposable")

    def test_native_worker_uses_new_thread_and_only_data(self):
        adapter = gate.Codex.__new__(gate.Codex)
        adapter.context,adapter.worker_threads = "reader",set()
        adapter.thread = lambda _: "worker"
        seen = []
        adapter.turn = lambda thread,prompt,schema: seen.append((thread,prompt,schema)) or {
            "pages":[{"slug":"overview","body":"## Policy\nold rule\n## Unchanged\nkeep this\n"}]}
        adapter.review({"instructions":"role","room":"general","messages":[],
                        "wiki":{"revision":0,"pages":[]},"reviewed_through":3})
        self.assertEqual(seen[0][0],"worker")
        self.assertNotIn('"instructions"',seen[0][1])
        with self.assertRaisesRegex(RuntimeError,"reused"):
            adapter.review({"instructions":"role"})

    def test_empty_default_pages_are_not_a_previously_published_wiki(self):
        payload = {"wiki":{"revision":0,"pages":[{"slug":"overview","body":""}]}}
        gate.verify_oracle_review({"pages":[{"slug":"overview",
            "body":"## Policy\nold rule\n## Unchanged\nkeep this\n"}]},payload)
        with self.assertRaisesRegex(RuntimeError,"oracle"):
            gate.verify_oracle_review({"pages":[{"slug":"overview",
                "body":"## Policy\nnew rule\n## Unchanged\nkeep this\n"}]},payload)

    def test_failed_native_start_does_not_ack_context(self):
        context = WikiPageContext("general","reader")
        packet = self.packet(context)
        adapter = gate.Codex.__new__(gate.Codex)
        adapter.context = "reader"
        def failed(*_args):
            raise RuntimeError("rejected")
        adapter.turn = failed
        with self.assertRaisesRegex(RuntimeError,"rejected"):
            adapter.inject(packet,context,"old rule")
        self.assertFalse(context.injected_pages)
        self.assertIn(packet["packet_id"],context.pending)

    def test_wrong_native_context_is_rejected_before_call(self):
        context = WikiPageContext("general","reader")
        packet = self.packet(context)
        adapter = gate.Codex.__new__(gate.Codex)
        adapter.context = "other"
        adapter.turn = lambda *_: self.fail("foreign thread was called")
        with self.assertRaisesRegex(RuntimeError,"wrong native"):
            adapter.inject(packet,context,"old rule")
        self.assertFalse(context.injected_pages)

    def test_model_result_without_acceptance_cannot_be_certified(self):
        context = WikiPageContext("general","reader")
        packet = self.packet(context)
        adapter = gate.Codex.__new__(gate.Codex)
        adapter.context = "reader"
        # Acceptance callback must execute; returning the expected JSON alone
        # is not proof that the packet was actually accepted by a thread.
        adapter.turn = lambda *_: {"policy":"old rule","unchanged":"keep this"}
        with self.assertRaisesRegex(RuntimeError,"acceptance"):
            adapter.inject(packet,context,"old rule")

    def test_claude_wrong_or_failed_result_does_not_ack(self):
        context = WikiPageContext("general","reader")
        packet = self.packet(context)
        adapter = gate.Claude.__new__(gate.Claude)
        adapter.context = "reader"
        adapter.call = lambda *_: {"policy":"wrong","unchanged":"keep this"}
        with self.assertRaisesRegex(RuntimeError,"did not retain"):
            adapter.inject(packet,context,"old rule")
        self.assertFalse(context.injected_pages)


if __name__ == "__main__":
    unittest.main()
