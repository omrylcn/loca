"""Who a message calls: exact @names and targets, broadcasts, the Lobby's
narrower rule, and the delivery priority of an event.
"""

import re
from credentials import LOBBY_ROOM


def directly_names(message, name):
    """Return true only for an exact target or exact @name token."""
    expected = name.casefold()
    target = str(message.get("target") or "").casefold()
    if target == expected:
        return True
    # Agents often address the room (`target=all`) while explicitly naming
    # one or more people in the text.  The exact @name is still a direct call;
    # treating target=all as authoritative made those calls disappear for
    # --only-direct listeners.  Bare @all remains filtered because it does not
    # contain this identity's exact name.
    mentions = re.findall(r"(?<![A-Za-z0-9_-])@([A-Za-z0-9_-]+)", message.get("text") or "")
    return any(mention.casefold() == expected for mention in mentions)


def exact_mentions(message):
    return {
        mention.casefold()
        for mention in re.findall(
            r"(?<![A-Za-z0-9_-])@([A-Za-z0-9_-]+)",
            str(message.get("text") or ""),
        )
    }


def broadcasts(message):
    return (
        str(message.get("target") or "").casefold() == "all"
        or "all" in exact_mentions(message)
    )


def delivery_priority(event, identity, is_lead=False):
    """Classify one delivery according to Adapter Protocol v1."""
    if event.get("t") == "control":
        return "security_control"
    if event.get("t") == "care":
        return "care_signal"
    if event.get("t") == "task":
        return "explicit_task"
    if event.get("t") == "reaction":
        return "direct_user"
    messages = event.get("messages") if event.get("t") == "turn" else [event]
    messages = [message for message in messages if isinstance(message, dict)]
    user_messages = [
        message for message in messages if message.get("sender_type") == "user"
    ]
    if any(directly_names(message, identity) for message in user_messages):
        return "direct_user"
    if any(broadcasts(message) for message in messages):
        return "broadcast"
    if any(
        message.get("sender_type") == "agent"
        and directly_names(message, identity)
        for message in messages
    ):
        return "addressed_agent"
    if is_lead:
        # Only unaddressed traffic is context-only. A lead remains reachable
        # by other agents; the widened room stream must not swallow summons.
        return "lead_room"
    return "informational"


def lobby_addresses(m, name):
    """Does a Lobby room message call ``name``?

    In the Lobby only an exact ``target`` or ``@name``, a reply to this
    identity, or ``target: all`` calls an agent. The server keeps ``target:
    all`` for the ranks (a Master or Smaster ``@all`` is stored with it) and
    drops it from everyone else's message, so ``@all`` in the text alone is
    no call there: one member's greeting must not wake every agent of the
    Building. An announcement calls nobody.
    """
    if directly_names(m, name):
        return True
    if str(m.get("reply_to_sender") or "") == name:
        return True
    return (
        str(m.get("target") or "").casefold() == "all"
        and m.get("kind") != "announce"
    )


def addresses(m, name, room=None):
    """Client-side mirror of the server's msg_addresses (for REST backfill,
    which is unfiltered): target == name/all, or word-boundary @name/@all."""
    if room == LOBBY_ROOM:
        return lobby_addresses(m, name)
    if m.get("target") in (name, "all"):
        return True
    needle = "@" + name.lower()
    tok = ""
    for c in m.get("text", "").lower() + " ":
        if c.isalnum() or c in "@-_":
            tok += c
        else:
            if tok in (needle, "@all"):
                return True
            tok = ""
    return False


def should_deliver_message(message, skip_own, only_direct, current_lead):
    """Apply local runtime policy without hiding the room from its lead."""
    if skip_own and message.get("sender") == skip_own:
        return False
    if (
        only_direct
        and not current_lead
        and not directly_names(message, only_direct)
        and not broadcasts(message)
    ):
        return False
    return True
