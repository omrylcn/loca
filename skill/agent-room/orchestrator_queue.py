#!/usr/bin/env python3
"""Durable turn inbox reader for session-scoped Loca orchestrators.

The listener appends one JSON envelope per runtime turn.  A monitor calls
``next`` to obtain the oldest unacknowledged envelope and calls ``ack`` only
after the worker turn finishes.  The cursor is separate from the listener's
WebSocket cursor on purpose: written-to-disk is not the same as processed.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import time
from pathlib import Path
from typing import Any

ACK_HISTORY_LIMIT = 1024
PRIORITY_RANKS = {
    "security_control": 0,
    "direct_user": 1,
    "care_signal": 2,
    "explicit_task": 3,
    "lead_room": 4,
    "broadcast": 5,
    "addressed_agent": 6,
    "informational": 7,
}
PROTECTED_PRIORITIES = {
    PRIORITY_RANKS["security_control"],
    PRIORITY_RANKS["care_signal"],
    PRIORITY_RANKS["explicit_task"],
}


def empty_state() -> dict[str, Any]:
    """Return a fresh cursor state (the rooms map must never be shared)."""
    return {
        "offset": 0,
        "rooms": {},
        "delivery_id": None,
        "acked_delivery_ids": [],
    }


def load_state(path: Path) -> dict[str, Any]:
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return empty_state()
    if not isinstance(raw, dict):
        return empty_state()
    rooms = raw.get("rooms")
    acked = raw.get("acked_delivery_ids")
    if not isinstance(acked, list):
        acked = []
    return {
        "offset": max(0, int(raw.get("offset") or 0)),
        "rooms": {
            str(room): max(0, int(last_id))
            for room, last_id in (rooms.items() if isinstance(rooms, dict) else [])
        },
        "delivery_id": raw.get("delivery_id"),
        "acked_delivery_ids": [
            str(delivery_id)
            for delivery_id in acked[-ACK_HISTORY_LIMIT:]
            if isinstance(delivery_id, str) and delivery_id
        ],
    }


def save_state(path: Path, state: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temp = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    fd = os.open(temp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            json.dump(state, handle, ensure_ascii=False, sort_keys=True)
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temp, path)
        os.chmod(path, 0o600)
    finally:
        try:
            temp.unlink()
        except FileNotFoundError:
            pass


def parse_record(raw: bytes, offset: int, next_offset: int) -> dict[str, Any]:
    try:
        record = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ValueError(f"invalid inbox JSON at byte {offset}") from exc
    if not isinstance(record, dict) or not record.get("delivery_id"):
        raise ValueError(f"invalid inbox record at byte {offset}")
    record["_queue_offset"] = offset
    record["_queue_next_offset"] = next_offset
    return record


def is_already_acked(record: dict[str, Any], state: dict[str, Any]) -> bool:
    if record.get("delivery_id") in set(state["acked_delivery_ids"]):
        return True
    # A direct call may bypass an older task/control record. The newer call's
    # room watermark must not erase that protected work on the next scan.
    if record_priority(record) in PROTECTED_PRIORITIES:
        return False
    room = str(record.get("room") or "")
    last_id = record.get("last_id")
    if not room or not isinstance(last_id, int):
        return False
    return last_id <= int(state["rooms"].get(room, 0))


def event_messages(record: dict[str, Any]) -> list[dict[str, Any]]:
    event = record.get("event")
    if not isinstance(event, dict):
        return []
    messages = event.get("messages") if event.get("t") == "turn" else [event]
    return [message for message in messages if isinstance(message, dict)]


def exact_mentions(message: dict[str, Any]) -> set[str]:
    text = str(message.get("text") or "")
    return {
        mention.casefold()
        for mention in re.findall(r"(?<![A-Za-z0-9_-])@([A-Za-z0-9_-]+)", text)
    }


def message_names(message: dict[str, Any], identity: str) -> bool:
    if not identity:
        return False
    expected = identity.casefold()
    return (
        str(message.get("target") or "").casefold() == expected
        or expected in exact_mentions(message)
    )


def message_broadcasts(message: dict[str, Any]) -> bool:
    return (
        str(message.get("target") or "").casefold() == "all"
        or "all" in exact_mentions(message)
    )


def record_priority(record: dict[str, Any]) -> int:
    """Return Adapter Protocol v1 priority, with a legacy-envelope fallback."""
    declared = record.get("priority")
    if isinstance(declared, str) and declared in PRIORITY_RANKS:
        return PRIORITY_RANKS[declared]

    messages = event_messages(record)
    event = record.get("event")
    if isinstance(event, dict) and event.get("t") == "control":
        return PRIORITY_RANKS["security_control"]
    if isinstance(event, dict) and event.get("t") == "care":
        return PRIORITY_RANKS["care_signal"]
    if isinstance(event, dict) and event.get("t") == "task":
        return PRIORITY_RANKS["explicit_task"]

    identity = str(record.get("identity") or "")
    user_messages = [
        message for message in messages if message.get("sender_type") == "user"
    ]
    if user_messages and (
        not identity
        or any(message_names(message, identity) for message in user_messages)
    ):
        # Envelopes predating protocol v1 did not carry identity. Preserve
        # their former operator-first behavior rather than starving them.
        return PRIORITY_RANKS["direct_user"]
    if any(message_broadcasts(message) for message in messages):
        return PRIORITY_RANKS["broadcast"]
    if user_messages:
        return PRIORITY_RANKS["lead_room"]
    if messages and any(
        message_names(message, identity)
        and message.get("sender_type") == "agent"
        for message in messages
    ):
        return PRIORITY_RANKS["addressed_agent"]
    return PRIORITY_RANKS["informational"]


def choose_record(
    records: list[dict[str, Any]], base_offset: int = 0
) -> dict[str, Any]:
    """Choose urgent work and compact superseded chat backlog."""
    best_priority = min(record_priority(record) for record in records)
    candidates = [
        record for record in records if record_priority(record) == best_priority
    ]
    compactable = {
        PRIORITY_RANKS["direct_user"],
        PRIORITY_RANKS["lead_room"],
        PRIORITY_RANKS["broadcast"],
        PRIORITY_RANKS["addressed_agent"],
    }
    # The newest addressed chat represents current room state. Controls,
    # tasks, and legacy/informational records stay FIFO.
    chosen = (
        candidates[-1] if best_priority in compactable else candidates[0]
    )
    record_end = chosen["_queue_next_offset"]
    prior_records = [
        record
        for record in records
        if record["_queue_offset"] < chosen["_queue_offset"]
    ]
    protected_prior = [
        record
        for record in prior_records
        if record_priority(record) in PROTECTED_PRIORITIES
    ]
    # `_queue_next_offset` is the highest *contiguous safe* ACK point, not
    # necessarily the chosen record's byte end. If an urgent direct mention
    # jumped over an older task, keep the cursor parked at that task and record
    # this delivery id separately. The task is then offered on the next pull.
    chosen["_queue_record_end"] = record_end
    chosen["_queue_next_offset"] = (
        min(record["_queue_offset"] for record in protected_prior)
        if protected_prior
        else record_end
    )
    chosen["_queue_compacted"] = sum(
        1
        for record in prior_records
        if record_priority(record) not in PROTECTED_PRIORITIES
        and record["_queue_next_offset"] > base_offset
    )
    return chosen


def next_record(
    inbox: Path,
    cursor: Path,
    wait_seconds: float,
    poll_seconds: float,
) -> dict[str, Any] | None:
    deadline = time.monotonic() + max(0.0, wait_seconds)
    while True:
        state = load_state(cursor)
        try:
            size = inbox.stat().st_size
        except FileNotFoundError:
            size = 0
        if state["offset"] > size:
            # The inbox was deliberately rotated/truncated. Start from its
            # beginning; per-room watermarks still suppress replayed messages.
            state["offset"] = 0
            save_state(cursor, state)
        if size > state["offset"]:
            records = []
            scan_offset = state["offset"]
            found_unacked = False
            with inbox.open("rb") as handle:
                handle.seek(state["offset"])
                while raw := handle.readline():
                    offset = scan_offset
                    next_offset = handle.tell()
                    scan_offset = next_offset
                    record = parse_record(raw, offset, next_offset)
                    if is_already_acked(record, state):
                        if not found_unacked:
                            state["offset"] = next_offset
                            save_state(cursor, state)
                        continue
                    found_unacked = True
                    records.append(record)
            if records:
                return choose_record(records, state["offset"])
        if time.monotonic() >= deadline:
            return None
        time.sleep(max(0.05, poll_seconds))


def ack_record(
    cursor: Path,
    offset: int,
    room: str,
    last_id: int | None,
    delivery_id: str,
) -> dict[str, Any]:
    state = load_state(cursor)
    state["offset"] = max(state["offset"], offset)
    if room and last_id is not None:
        state["rooms"][room] = max(int(state["rooms"].get(room, 0)), last_id)
    state["delivery_id"] = delivery_id
    acked = [
        existing
        for existing in state["acked_delivery_ids"]
        if existing != delivery_id
    ]
    acked.append(delivery_id)
    state["acked_delivery_ids"] = acked[-ACK_HISTORY_LIMIT:]
    save_state(cursor, state)
    return state


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    nxt = sub.add_parser("next", help="wait for and print one unacked turn")
    nxt.add_argument("--inbox", type=Path, required=True)
    nxt.add_argument("--cursor", type=Path, required=True)
    nxt.add_argument("--wait-seconds", type=float, default=30.0)
    nxt.add_argument("--poll-seconds", type=float, default=0.2)

    ack = sub.add_parser("ack", help="acknowledge one completed worker turn")
    ack.add_argument("--cursor", type=Path, required=True)
    ack.add_argument("--offset", type=int, required=True)
    ack.add_argument("--room", default="")
    ack.add_argument("--last-id", type=int)
    ack.add_argument("--delivery-id", required=True)

    status = sub.add_parser("status", help="print the current worker cursor")
    status.add_argument("--cursor", type=Path, required=True)

    args = parser.parse_args()
    try:
        if args.command == "next":
            record = next_record(
                args.inbox,
                args.cursor,
                args.wait_seconds,
                args.poll_seconds,
            )
            if record is None:
                return 3
            print(json.dumps(record, ensure_ascii=False))
            return 0
        if args.command == "ack":
            state = ack_record(
                args.cursor,
                args.offset,
                args.room,
                args.last_id,
                args.delivery_id,
            )
            print(json.dumps(state, ensure_ascii=False, sort_keys=True))
            return 0
        print(json.dumps(load_state(args.cursor), ensure_ascii=False, sort_keys=True))
        return 0
    except (OSError, ValueError) as exc:
        print(f"orchestrator queue failed: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
