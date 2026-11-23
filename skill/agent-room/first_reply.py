#!/usr/bin/env python3
"""`connect.sh first-reply`: the end of connecting an agent is a reply.

A running listener and a green `doctor` say that delivery works; they do not
say that a call wakes this agent's worker and that the worker answers. The
only evidence of that is the thing itself: somebody names the agent in the
Lobby room and the agent's own message follows.

This helper judges the Lobby messages that came after the agent's hello
(`--after`, a message id), read by connect.sh as a JSON list. It prints one
line and exits:

- 0 PROVEN: someone other than the agent named it, and the agent posted after
  that call. The line gives who called, when, and how long the answer took.
- 3 CALLED: the call is there, no answer yet.
- 2 WAITING: nobody has named the agent yet.

Stdlib only; reads no credential and opens no connection.
"""
import json
import re
import sys

# The server's mention grammar (listen_addressing.py, docs/mention-cases.json).
_MENTION = re.compile(r"(?<![\w.])@([\w.-]+)")

PROVEN, WAITING, CALLED = 0, 2, 3


def _mentions(text):
    return {m.group(1).rstrip(".-").casefold() for m in _MENTION.finditer(str(text or ""))}


def calls(message, name, hello_id=None):
    """Does `message` call `name`: an exact target, an exact @name, or a
    reply to the agent (to its hello, or to any message the server says was
    the agent's)?"""
    expected = name.casefold()
    if str(message.get("target") or "").casefold() == expected:
        return True
    if expected in _mentions(message.get("text")):
        return True
    if str(message.get("reply_to_sender") or "").casefold() == expected:
        return True
    return hello_id is not None and message.get("reply_to") == hello_id


def judge(messages, name, hello_id=None):
    """(code, call, answer) for the Lobby messages after the hello."""
    expected = name.casefold()
    rows = sorted(
        (m for m in messages if isinstance(m, dict) and isinstance(m.get("id"), int)),
        key=lambda m: m["id"],
    )
    call = next(
        (
            m
            for m in rows
            if str(m.get("sender") or "").casefold() != expected
            and calls(m, name, hello_id)
        ),
        None,
    )
    if call is None:
        return WAITING, None, None
    answer = next(
        (
            m
            for m in rows
            if m["id"] > call["id"] and str(m.get("sender") or "").casefold() == expected
        ),
        None,
    )
    if answer is None:
        return CALLED, call, None
    return PROVEN, call, answer


def _seconds(call, answer):
    try:
        return max(0.0, (int(answer["ts"]) - int(call["ts"])) / 1000)
    except (KeyError, TypeError, ValueError):
        return None


def line(code, call, answer, name):
    if code == PROVEN:
        took = _seconds(call, answer)
        took_text = f" in {took:.0f} s" if took is not None else ""
        return (
            f"PROVEN: {call.get('sender')} called @{name} in the Lobby (message "
            f"{call['id']}) and {name} answered{took_text} (message {answer['id']}). "
            "Delivery, wake-up and reply work."
        )
    if code == CALLED:
        return (
            f"CALLED: {call.get('sender')} called @{name} (message {call['id']}); "
            "no answer from it yet."
        )
    return f"WAITING: nobody has written @{name} in the Lobby yet."


def main(argv):
    if len(argv) not in (2, 3):
        print("usage: first_reply.py NAME [HELLO_ID] < lobby-messages.json", file=sys.stderr)
        return 64
    name = argv[1]
    hello_id = int(argv[2]) if len(argv) == 3 and argv[2].isdigit() else None
    try:
        messages = json.load(sys.stdin)
    except ValueError:
        messages = []
    if not isinstance(messages, list):
        messages = []
    code, call, answer = judge(messages, name, hello_id)
    print(line(code, call, answer, name))
    return code


if __name__ == "__main__":
    sys.exit(main(sys.argv))
