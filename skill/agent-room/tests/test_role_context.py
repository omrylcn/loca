"""The kit's role and extension cards: which apply to a loca's state, what a
change owes the agent, and that every card the manifest names is shipped."""
import json
import os
import subprocess
import sys
import unittest
from pathlib import Path

SKILL_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SKILL_DIR))

import role_context  # noqa: E402


class RoleContextTests(unittest.TestCase):
    def test_every_card_in_the_manifest_is_shipped_and_short(self):
        cards = role_context.load_manifest()
        ids = [card["id"] for card in cards]
        self.assertEqual(len(ids), len(set(ids)))
        for card in cards:
            text = (Path(role_context.CARDS_DIR) / card["file"]).read_text(encoding="utf-8")
            self.assertTrue(text.strip(), card["id"])
            self.assertLess(len(text), 1200, f"{card['id']} is a card, not a manual")
            self.assertTrue(card.get("title") and card.get("lost"), card["id"])
        # Every extension of the server's catalog has a card.
        catalog = (SKILL_DIR.parents[1] / "crates/protocol/src/lib.rs").read_text(encoding="utf-8")
        for extension in ("notes", "focus", "board", "journal", "wiki", "live", "vault"):
            self.assertIn(f'id: "{extension}"', catalog)
            self.assertIn(extension, ids)

    def test_a_mentat_room_waiting_for_its_mentat_gives_everyone_the_askers_card(self):
        settings = {"extensions": [], "mentat_room": True}
        state = role_context.room_state(settings, "ada")
        self.assertEqual(state["mentat"], False)
        self.assertEqual(state["asker"], True)
        self.assertEqual(role_context.active_cards(state), ["mentat-room"])

    def test_a_mentat_room_gives_the_mentat_and_its_askers_their_cards(self):
        settings = {"extensions": [], "mentat": "Sage"}
        mentat = role_context.room_state(settings, "sage")
        asker = role_context.room_state(settings, "ada")
        self.assertEqual(mentat, {"extensions": [], "lead": False, "mentat": True, "asker": False})
        self.assertEqual(role_context.active_cards(mentat), ["mentat"])
        self.assertEqual(role_context.active_cards(asker), ["mentat-room"])
        # Appointed the mentat: the asker's card is taken back, with its line.
        now_mentat = role_context.room_state({"extensions": [], "mentat": "ada"}, "ada")
        gone = role_context.packet("r", asker, now_mentat)
        self.assertEqual([entry["id"] for entry in gone["lost"]], ["mentat-room"])
        self.assertIn("mentat now", gone["lost"][0]["text"])

    def test_state_reads_the_lead_by_name_and_the_legacy_extension_set(self):
        state = role_context.room_state({"lead": "Bob", "extensions": ["board", "notes"]}, "bob")
        self.assertEqual(state, {"extensions": ["board", "notes"], "lead": True})
        legacy = role_context.room_state({"lead": "ada"}, "bob")
        self.assertEqual(legacy["extensions"], sorted(role_context.LEGACY_EXTENSIONS))
        self.assertFalse(legacy["lead"])
        self.assertEqual(role_context.room_state(None, "bob"), {"extensions": [], "lead": False})

    def test_a_first_packet_gives_every_card_that_applies(self):
        state = role_context.room_state({"lead": "bob", "extensions": ["board", "notes"]}, "bob")
        packet = role_context.packet("proj", None, state)
        self.assertEqual([c["id"] for c in packet["gained"]], ["lead", "lead-board", "board", "notes"])
        self.assertEqual(packet["lost"], [])
        self.assertIn("task-new", packet["gained"][1]["text"])
        self.assertIn("not a task and not new authority", packet["note"])
        # Someone who is not the Lead gets no Lead card.
        other = role_context.packet("proj", None, role_context.room_state(
            {"lead": "bob", "extensions": ["board"]}, "carol"))
        self.assertEqual([c["id"] for c in other["gained"]], ["board"])

    def test_a_change_owes_only_what_changed(self):
        before = role_context.room_state({"lead": "ada", "extensions": ["notes"]}, "bob")
        became_lead = role_context.room_state({"lead": "bob", "extensions": ["notes"]}, "bob")
        packet = role_context.packet("proj", before, became_lead)
        self.assertEqual([c["id"] for c in packet["gained"]], ["lead"])
        board_on = role_context.room_state({"lead": "bob", "extensions": ["board", "notes"]}, "bob")
        packet = role_context.packet("proj", became_lead, board_on)
        self.assertEqual([c["id"] for c in packet["gained"]], ["lead-board", "board"])
        # Losing the title and the board: said once, with what stops working.
        packet = role_context.packet("proj", board_on, before)
        self.assertEqual(packet["gained"], [])
        self.assertEqual([c["id"] for c in packet["lost"]], ["lead", "lead-board", "board"])
        self.assertIn("403", packet["lost"][0]["text"])
        self.assertIsNone(role_context.packet("proj", before, dict(before)))

    def test_a_card_over_the_budget_is_named_with_where_to_read_it(self):
        state = role_context.room_state({"lead": "bob", "extensions": ["board"]}, "bob")
        packet = role_context.packet("proj", None, state, limit=500)
        self.assertIn("text", packet["gained"][0])
        self.assertNotIn("text", packet["gained"][1])
        self.assertIn("connect.sh context", packet["gained"][1]["read"])
        self.assertIn("Read it with", role_context.render(packet))

    def test_the_command_prints_the_cards_for_a_settings_document(self):
        result = subprocess.run(
            [sys.executable, str(SKILL_DIR / "role_context.py"), "show", "proj", "bob"],
            input=json.dumps({"lead": "bob", "extensions": ["board"]}),
            capture_output=True, text=True, timeout=10, env={**os.environ},
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("[You are this loca's Lead]", result.stdout)
        self.assertIn("task-assign", result.stdout)
        empty = subprocess.run(
            [sys.executable, str(SKILL_DIR / "role_context.py"), "show", "proj", "bob"],
            input=json.dumps({"extensions": []}), capture_output=True, text=True, timeout=10,
        )
        self.assertIn("No role or extension context", empty.stdout)


if __name__ == "__main__":
    unittest.main()
