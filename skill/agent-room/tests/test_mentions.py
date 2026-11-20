"""The kit reads mentions with the server's grammar: docs/mention-cases.json
pins it for both (crates/server/src/hub.rs `mentioned_names`)."""
import json
import sys
import unittest
from pathlib import Path

KIT = Path(__file__).resolve().parents[1]
ROOT = KIT.parents[1]
sys.path.insert(0, str(KIT))

import listen  # noqa: E402
import listen_addressing  # noqa: E402

CASES = json.loads((ROOT / "docs" / "mention-cases.json").read_text(encoding="utf-8"))["cases"]


class MentionGrammarTests(unittest.TestCase):
    def test_the_shared_cases(self):
        for case in CASES:
            with self.subTest(text=case["text"]):
                self.assertEqual(listen_addressing.mentions(case["text"]), case["names"])

    def test_a_dotted_name_is_called(self):
        message = {"text": "@ops.bot review this", "target": None}
        self.assertTrue(listen.directly_names(message, "ops.bot"))
        self.assertTrue(listen.addresses(message, "ops.bot"))
        self.assertFalse(listen.addresses(message, "ops"))
        self.assertFalse(listen.broadcasts(message))

    def test_all_is_everyone_but_all_dot_bot_is_a_name(self):
        self.assertTrue(listen.broadcasts({"text": "@all please look"}))
        self.assertFalse(listen.broadcasts({"text": "@all.bot is a name"}))


if __name__ == "__main__":
    unittest.main()
