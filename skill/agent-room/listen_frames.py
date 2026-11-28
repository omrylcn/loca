"""The server frames the listener knows, on a loca socket and on the Lobby
socket, and what it does with each (docs/protocol-frames.json).
"""

import re
import socket
from listen_ws import frames


# What this listener does with every server frame in docs/protocol-frames.json
# (docs/protocol.md), plus the "not_sent_yet" frames it already dispatches on.
# "handled" means watch() / lobby_watch() dispatch on it;
# any other value says why it is deliberately ignored. A frame type missing
# here fails tests/test_protocol_frames.py: none may fall through unexamined.
_FILTERED = "ignored: the server does not send it on the filter=mentions/msg socket the kit opens"


LOCA_FRAMES = {
    "msg": "handled",
    "turn": "handled",
    "control": "handled",
    "care": "handled",
    "reaction": "handled",
    "kicked": "handled",
    "evicted": "handled",
    "vault": "handled",
    "memory": "ignored: retired knowledge role; must neither wake a model nor be injected",
    "memoryversion": "ignored: retired knowledge role; must neither wake a model nor be injected",
    "caretaker": "ignored: never on the wire; the server delivers it as msg",
    "history": _FILTERED + "; missed messages come from REST ?since= backfill",
    "members": _FILTERED,
    "renamed": _FILTERED,
    "relabelled": _FILTERED,
    "settings": _FILTERED + "; the lead is re-read over REST on an announce",
    "note": _FILTERED,
    "notewarn": _FILTERED,
    "mode": _FILTERED,
    "typing": _FILTERED,
    "mod": _FILTERED,
    "journal": _FILTERED,
    "lesson": _FILTERED + "; an accepted lesson arrives as a vault revision",
    "mentat": _FILTERED + "; the line that calls an asker is a chat message naming it",
    "task": _FILTERED,
    "goal": _FILTERED,
    "attention": _FILTERED,
    "wait": _FILTERED,
}


LOBBY_FRAMES = {
    "lobby_ready": "handled",
    "called": "handled",
    "notification": "handled",
    "membership_revoked": "handled",
    "seats": "handled",
    "credential_revoked": "handled",
    "membership_suspended": "handled",
}
