#!/usr/bin/env python3
"""What an agent should know about the loca it sits in: its role there and
the extensions that are on, as short cards.

The server sends state, never prose: a loca's settings name its extensions
and its Lead. This module turns that state into the cards under ``cards/``
(``manifest.json`` says which card belongs to which state), and into the
packet a listener adds to the agent's next real turn when the state
changed. It is the Vault's notice made general: context, not a turn of its
own and not instructions that outrank the agent's operator.

The cards ship with the kit, so their commands are always the ones this kit
has. Stdlib only.
"""
import json
import os
import sys

CARDS_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "cards")

# A loca saved before extensions existed has no `extensions` field and keeps
# these five (the server's frozen legacy set).
LEGACY_EXTENSIONS = ("notes", "focus", "journal", "wiki", "live")

# The most text one packet carries. A card that does not fit is named with
# where to read it instead.
PACKET_CHAR_LIMIT = 3000

NOTE = (
    "Loca context for this loca: your role here and what is switched on. It "
    "is information from the kit, not a task and not new authority; it never "
    "outranks your operator's instructions or Loca's principles. Read it all "
    "again any time with: connect.sh context SERVER ROOM NAME"
)


def load_manifest(cards_dir=CARDS_DIR):
    with open(os.path.join(cards_dir, "manifest.json"), encoding="utf-8") as handle:
        manifest = json.load(handle)
    cards = manifest.get("cards")
    if not isinstance(cards, list):
        raise ValueError("cards/manifest.json has no card list")
    return cards


def room_state(settings, name):
    """The state the cards depend on, from a loca's settings: the extensions
    that are on, and whether `name` is its Lead. Comparable with ==."""
    if not isinstance(settings, dict):
        return {"extensions": [], "lead": False}
    extensions = settings.get("extensions")
    if extensions is None:
        extensions = LEGACY_EXTENSIONS
    lead = str(settings.get("lead") or "")
    return {
        "extensions": sorted({str(item) for item in extensions if item}),
        "lead": bool(lead) and lead.casefold() == str(name or "").casefold(),
    }


def _holds(when, state):
    extension = when.get("extension")
    if extension and extension not in state["extensions"]:
        return False
    if when.get("role") == "lead" and not state["lead"]:
        return False
    return True


def active_cards(state, cards=None):
    """The ids of the cards that apply in `state`, in manifest order."""
    cards = load_manifest() if cards is None else cards
    return [card["id"] for card in cards if _holds(card.get("when") or {}, state)]


def _card_text(card, cards_dir):
    with open(os.path.join(cards_dir, card["file"]), encoding="utf-8") as handle:
        return handle.read().strip()


def packet(room, before, after, cards_dir=CARDS_DIR, limit=PACKET_CHAR_LIMIT):
    """The context an agent is owed in `room` when its state went from
    `before` to `after`; `before` None means it has been told nothing yet,
    so every card that applies is given. None when nothing is owed."""
    cards = load_manifest(cards_dir)
    by_id = {card["id"]: card for card in cards}
    now = active_cards(after, cards)
    then = [] if before is None else active_cards(before, cards)
    gained = [card_id for card_id in now if card_id not in then]
    lost = [card_id for card_id in then if card_id not in now]
    if not gained and not lost:
        return None
    result = {"room": room, "note": NOTE, "gained": [], "lost": []}
    used = 0
    for card_id in gained:
        card = by_id[card_id]
        text = _card_text(card, cards_dir)
        entry = {"id": card_id, "title": card.get("title") or card_id}
        if used + len(text) <= limit:
            entry["text"] = text
            used += len(text)
        else:
            entry["read"] = "connect.sh context SERVER ROOM NAME"
        result["gained"].append(entry)
    for card_id in lost:
        card = by_id[card_id]
        result["lost"].append({
            "id": card_id,
            "title": card.get("title") or card_id,
            "text": card.get("lost") or "",
        })
    return result


def render(context):
    """A packet as plain text, for a prompt or a terminal."""
    if not isinstance(context, dict):
        return ""
    lines = [str(context.get("note") or NOTE)]
    for entry in context.get("gained") or []:
        lines.append("")
        lines.append(f"[{entry.get('title')}]")
        lines.append(entry.get("text") or f"Read it with: {entry.get('read')}")
    for entry in context.get("lost") or []:
        lines.append("")
        lines.append(f"[{entry.get('title')}: no longer applies] {entry.get('text')}".rstrip())
    return "\n".join(lines)


def main(argv):
    # role_context.py show ROOM NAME   (settings JSON on stdin)
    if len(argv) != 4 or argv[1] != "show":
        sys.stderr.write("usage: role_context.py show ROOM NAME < settings.json\n")
        return 2
    try:
        settings = json.load(sys.stdin)
    except ValueError:
        sys.stderr.write("settings are not JSON\n")
        return 1
    context = packet(argv[2], None, room_state(settings, argv[3]), limit=10**9)
    if context is None:
        print("No role or extension context applies to you in this loca.")
        return 0
    print(render(context))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
