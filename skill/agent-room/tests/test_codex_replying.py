"""A live Codex v2 adapter tells the room it is replying while a model turn
runs: "on" when the turn starts, repeated while it runs (the room forgets a
typing signal after a few seconds), and "off" once it completes. A shadow
adapter never speaks to the room, so it says nothing either."""

import sys
import tempfile
import unittest
from pathlib import Path

SKILL_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SKILL_DIR))
sys.path.insert(0, str(Path(__file__).resolve().parent))

from test_codex_adapter_v2 import AdapterFixture, delivery  # noqa: E402


class ReplyingRecorder:
    def __init__(self):
        self.calls = []

    def __call__(self, server, room, identity, on):
        self.calls.append((server, room, identity, on))


def completed(turn_id="turn-1", thread_id="thread-1"):
    return {
        "method": "turn/completed",
        "params": {"threadId": thread_id, "turn": {"id": turn_id, "status": "completed"}},
    }


class CodexReplyingTests(unittest.TestCase):
    def fixture(self, tmp, relay_mode="live"):
        fixture = AdapterFixture(Path(tmp), relay_mode=relay_mode)
        recorder = ReplyingRecorder()
        fixture.adapter.replying = recorder
        return fixture, recorder

    def test_a_running_turn_says_replying_and_its_end_says_done(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture, recorder = self.fixture(tmp)
            fixture.ingest(delivery(500))
            fixture.adapter.cycle()
            on = ("https://loca.example", "sb-dev", "reviewer", True)
            self.assertEqual(recorder.calls, [on])

            # Within the refresh window nothing is repeated.
            fixture.adapter.cycle()
            self.assertEqual(recorder.calls, [on])

            # After it, the still-running turn says it again.
            fixture.adapter.replying_refresh_at = {
                scope: 0 for scope in fixture.adapter.replying_refresh_at
            }
            fixture.adapter.cycle()
            self.assertEqual(recorder.calls, [on, on])

            fixture.fake.pending.append(completed())
            fixture.adapter.cycle()
            self.assertEqual(
                recorder.calls[-1], ("https://loca.example", "sb-dev", "reviewer", False)
            )
            calls = len(recorder.calls)
            fixture.adapter.cycle()
            self.assertEqual(len(recorder.calls), calls, "an idle adapter says nothing")
            fixture.adapter.close()

    def test_a_shadow_adapter_never_says_replying(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture, recorder = self.fixture(tmp, relay_mode="shadow")
            fixture.ingest(delivery(501))
            fixture.adapter.cycle()
            fixture.fake.pending.append(completed())
            fixture.adapter.cycle()
            self.assertEqual(recorder.calls, [])
            fixture.adapter.close()

    def test_a_failing_signal_never_stops_the_turn(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture, _ = self.fixture(tmp)

            def broken(*_args):
                raise OSError("network down")

            fixture.adapter.replying = broken
            fixture.ingest(delivery(502))
            fixture.adapter.cycle()
            methods = [method for method, _ in fixture.fake.requests]
            self.assertEqual(methods.count("turn/start"), 1)
            fixture.adapter.close()


if __name__ == "__main__":
    unittest.main()
