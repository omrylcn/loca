#!/usr/bin/env python3
"""Consume Loca delivery envelopes one at a time and ACK only on success.

The WebSocket listener owns durable delivery into an append-only inbox. This
process owns runtime wake-up. Keeping those concerns separate means a failed
Codex/hook invocation is retried after restart instead of being mistaken for a
delivered model turn.
"""

from __future__ import annotations

import argparse
import fcntl
import json
import os
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path
from typing import Any

import orchestrator_queue


class DeliveryYielded(RuntimeError):
    """The active adapter yielded so a newer urgent delivery can run."""

    def __init__(self, delivery_id: str):
        super().__init__(f"yielded to urgent delivery {delivery_id}")
        self.delivery_id = delivery_id


class UrgentInboxProbe:
    """Read only records appended after the currently running delivery."""

    def __init__(self, inbox: Path, record: dict[str, Any]):
        self.inbox = inbox
        self.current_delivery_id = str(record["delivery_id"])
        self.offset = int(
            record.get("_queue_record_end")
            or record.get("_queue_next_offset")
            or 0
        )

    def next_delivery_id(self) -> str:
        try:
            size = self.inbox.stat().st_size
        except FileNotFoundError:
            return ""
        if size < self.offset:
            # Inbox rotation during a turn is rare but valid. The rotated file
            # contains only newer durable records, so resume at its beginning.
            self.offset = 0
        if size <= self.offset:
            return ""

        urgent: list[dict[str, Any]] = []
        with self.inbox.open("rb") as handle:
            handle.seek(self.offset)
            while raw := handle.readline():
                # The listener appends one newline-terminated JSON record. If
                # we caught the writer between bytes, retry this tail later.
                if not raw.endswith(b"\n"):
                    break
                offset = self.offset
                next_offset = handle.tell()
                record = orchestrator_queue.parse_record(raw, offset, next_offset)
                self.offset = next_offset
                if str(record.get("delivery_id") or "") == self.current_delivery_id:
                    continue
                priority = orchestrator_queue.record_priority(record)
                if priority == orchestrator_queue.PRIORITY_RANKS["direct_user"]:
                    urgent.append(record)
        if not urgent:
            return ""
        return str(orchestrator_queue.choose_record(urgent)["delivery_id"])


def acquire_consumer_lock(cursor: Path):
    """Hold a non-blocking per-identity lease for this consumer's lifetime."""
    lock_path = cursor.with_name(cursor.name + ".lock")
    lock_path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    os.chmod(lock_path.parent, 0o700)
    fd = os.open(lock_path, os.O_RDWR | os.O_CREAT, 0o600)
    handle = os.fdopen(fd, "w", encoding="utf-8")
    try:
        fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        handle.close()
        raise RuntimeError(
            f"another consumer already owns {cursor}"
        ) from None
    handle.seek(0)
    handle.truncate()
    handle.write(f"{os.getpid()}\n")
    handle.flush()
    os.fsync(handle.fileno())
    return handle


def save_health(path: Path | None, health: dict[str, Any]) -> None:
    if path is None:
        return
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    os.chmod(path.parent, 0o700)
    temp = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    fd = os.open(temp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            json.dump(health, handle, ensure_ascii=False, sort_keys=True)
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


def event_messages(event: dict[str, Any]) -> list[dict[str, Any]]:
    messages = event.get("messages") if event.get("t") == "turn" else [event]
    return [message for message in messages if isinstance(message, dict)]


def delivery_environment(
    record: dict[str, Any], attempt: int
) -> dict[str, str]:
    event = record.get("event")
    if not isinstance(event, dict):
        raise ValueError("delivery event is not an object")
    messages = event_messages(event)
    first = messages[0] if messages else event
    senders = list(
        dict.fromkeys(str(message.get("sender") or "") for message in messages)
    )
    delivery_id = str(record["delivery_id"])
    env = dict(os.environ)
    envelope = json.dumps(record, ensure_ascii=False)
    env.update(
        {
            "LOCA_MSG": json.dumps(event, ensure_ascii=False),
            "LOCA_DELIVERY": envelope,
            "LOCA_FROM": ",".join(sender for sender in senders if sender),
            # Backward-compatible alias used by older generic adapters.
            "LOCA_SENDER": ",".join(sender for sender in senders if sender),
            "LOCA_TEXT": "\n".join(
                str(message.get("text") or "") for message in messages
            ),
            "LOCA_TARGET": str(first.get("target") or ""),
            "LOCA_ROOM": str(record.get("room") or first.get("room") or ""),
            "LOCA_ID": str(
                messages[-1].get("id", "") if messages else record.get("last_id") or ""
            ),
            "LOCA_DELIVERY_ID": delivery_id,
            "LOCA_OP_ID": f"loca-{delivery_id}",
            "LOCA_ATTEMPT": str(attempt),
            "LOCA_PRIORITY": str(record.get("priority") or "informational"),
            "LOCA_PROTOCOL_VERSION": str(record.get("protocol_version") or 0),
        }
    )
    return env


def run_delivery(
    command: str,
    record: dict[str, Any],
    attempt: int,
    timeout_seconds: float,
    on_wake=None,
    on_reply=None,
    urgent_delivery=None,
) -> None:
    env = delivery_environment(record, attempt)
    receipt_dir = Path(tempfile.mkdtemp(prefix="loca-wake-"))
    wake_receipt = receipt_dir / "accepted"
    reply_receipt = receipt_dir / "replied"
    env["LOCA_WAKE_RECEIPT"] = str(wake_receipt)
    env["LOCA_REPLY_RECEIPT"] = str(reply_receipt)
    process = subprocess.Popen(
        command,
        shell=True,
        text=True,
        env=env,
        stdin=subprocess.PIPE,
        start_new_session=True,
    )
    assert process.stdin is not None
    process.stdin.write(env["LOCA_DELIVERY"])
    process.stdin.close()
    deadline = time.monotonic() + timeout_seconds
    wake_reported = False
    reply_reported = False
    background_reaper = False

    def cleanup_receipts() -> None:
        for path in (wake_receipt, reply_receipt):
            try:
                path.unlink()
            except FileNotFoundError:
                pass
        try:
            receipt_dir.rmdir()
        except OSError:
            pass

    def reap_detached_process() -> None:
        try:
            process.wait()
        finally:
            cleanup_receipts()

    def detach_after_reply() -> None:
        nonlocal background_reaper, reply_reported
        reply_reported = True
        if on_reply is not None:
            on_reply()
        # A server-accepted room reply is the durable completion of this
        # delivery even if the Codex turn keeps running tests or deployment
        # work. Keep that productive turn alive and release the queue.
        threading.Thread(
            target=reap_detached_process,
            name=f"loca-reaper-{process.pid}",
            daemon=True,
        ).start()
        background_reaper = True

    try:
        while process.poll() is None:
            if wake_receipt.is_file() and not wake_reported:
                wake_reported = True
                if on_wake is not None:
                    on_wake()
            if reply_receipt.is_file():
                detach_after_reply()
                return
            if urgent_delivery is not None:
                urgent_id = urgent_delivery()
                if urgent_id:
                    # The inbox probe does I/O. Recheck the receipt after it so
                    # an accepted reply racing with the urgent append wins and
                    # the completed delivery is ACKed normally.
                    if reply_receipt.is_file():
                        detach_after_reply()
                        return
                    # Do not kill this adapter here.  The urgent delivery's
                    # nudge must reach Codex while the old turn is still
                    # visible so it can use the native turn/interrupt path.
                    # The interrupted adapter then exits and this reaper
                    # cleans its private receipt directory.  Its delivery is
                    # deliberately left unacknowledged: compactable chat is
                    # superseded by the newer room watermark, while protected
                    # task/control records are replayed after the direct call.
                    threading.Thread(
                        target=reap_detached_process,
                        name=f"loca-yield-reaper-{process.pid}",
                        daemon=True,
                    ).start()
                    background_reaper = True
                    raise DeliveryYielded(urgent_id)
            if time.monotonic() >= deadline:
                os.killpg(process.pid, 15)
                try:
                    process.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, 9)
                    process.wait(timeout=2)
                raise subprocess.TimeoutExpired(command, timeout_seconds)
            time.sleep(0.1)
        if wake_receipt.is_file() and not wake_reported and on_wake is not None:
            on_wake()
        if reply_receipt.is_file() and not reply_reported and on_reply is not None:
            on_reply()
        if process.returncode:
            raise subprocess.CalledProcessError(process.returncode, command)
    finally:
        if not background_reaper:
            cleanup_receipts()


def process_one(
    inbox: Path,
    cursor: Path,
    command: str,
    *,
    wait_seconds: float,
    poll_seconds: float,
    timeout_seconds: float,
    attempt: int = 1,
) -> dict[str, Any] | None:
    """Run at most one delivery; failure leaves it unacknowledged."""
    record = orchestrator_queue.next_record(
        inbox,
        cursor,
        wait_seconds,
        poll_seconds,
    )
    if record is None:
        return None
    run_delivery(command, record, attempt, timeout_seconds)
    orchestrator_queue.ack_record(
        cursor,
        int(record["_queue_next_offset"]),
        str(record.get("room") or ""),
        record.get("last_id") if isinstance(record.get("last_id"), int) else None,
        str(record["delivery_id"]),
    )
    return record


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--inbox", type=Path, required=True)
    parser.add_argument("--cursor", type=Path, required=True)
    parser.add_argument("--exec", dest="command", required=True)
    parser.add_argument("--wait-seconds", type=float, default=30.0)
    parser.add_argument("--poll-seconds", type=float, default=0.2)
    parser.add_argument("--timeout-seconds", type=float, default=330.0)
    parser.add_argument("--retry-seconds", type=float, default=2.0)
    parser.add_argument("--health-file", type=Path)
    parser.add_argument(
        "--preempt-direct-user",
        action="store_true",
        help=(
            "allow a newer direct-user delivery to interrupt an "
            "in-flight adapter that supports native turn interruption"
        ),
    )
    parser.add_argument("--once", action="store_true")
    args = parser.parse_args()

    try:
        consumer_lease = acquire_consumer_lock(args.cursor)
    except RuntimeError as exc:
        print(f"consumer lease failed: {exc}", file=sys.stderr)
        return 4

    attempts: dict[str, int] = {}
    health = {
        "protocol_version": "1",
        "adapter": "single-flight-command",
        "delivery": "OK",
        "wake": "IDLE",
        "reply": "UNVERIFIED",
        "ack": "IDLE",
        "updated_at_ms": int(time.time() * 1000),
    }
    save_health(args.health_file, health)
    try:
        while True:
            try:
                pending = orchestrator_queue.next_record(
                    args.inbox,
                    args.cursor,
                    args.wait_seconds,
                    args.poll_seconds,
                )
                if pending is None:
                    if args.once:
                        return 3
                    continue
                delivery_id = str(pending["delivery_id"])
                attempt = attempts.get(delivery_id, 0) + 1
                attempts[delivery_id] = attempt
                health.update(
                    {
                        "delivery": "OK",
                        "delivery_id": delivery_id,
                        "attempt": attempt,
                        "wake": "RUNNING",
                        "reply": "PENDING",
                        "ack": "PENDING",
                        "started_at_ms": int(time.time() * 1000),
                        "updated_at_ms": int(time.time() * 1000),
                    }
                )
                health.pop("yielded_to_delivery_id", None)
                save_health(args.health_file, health)
                urgent_probe = (
                    UrgentInboxProbe(args.inbox, pending)
                    if args.preempt_direct_user
                    else None
                )

                def wake_accepted() -> None:
                    health.update(
                        {
                            "wake": "OK",
                            "wake_accepted_at_ms": int(time.time() * 1000),
                            "updated_at_ms": int(time.time() * 1000),
                        }
                    )
                    health.pop("last_error", None)
                    save_health(args.health_file, health)

                def reply_accepted() -> None:
                    health.update(
                        {
                            "reply": "OK",
                            "reply_accepted_at_ms": int(time.time() * 1000),
                            "updated_at_ms": int(time.time() * 1000),
                        }
                    )
                    health.pop("last_error", None)
                    save_health(args.health_file, health)

                def urgent_delivery() -> str:
                    return (
                        urgent_probe.next_delivery_id()
                        if urgent_probe is not None
                        else ""
                    )

                run_delivery(
                    args.command,
                    pending,
                    attempt,
                    args.timeout_seconds,
                    wake_accepted,
                    reply_accepted,
                    urgent_delivery if urgent_probe is not None else None,
                )
                orchestrator_queue.ack_record(
                    args.cursor,
                    int(pending["_queue_next_offset"]),
                    str(pending.get("room") or ""),
                    (
                        pending.get("last_id")
                        if isinstance(pending.get("last_id"), int)
                        else None
                    ),
                    delivery_id,
                )
                attempts.pop(delivery_id, None)
                health.update(
                    {
                        "wake": "OK",
                        "ack": "OK",
                        "last_acked_delivery_id": delivery_id,
                        "updated_at_ms": int(time.time() * 1000),
                    }
                )
                save_health(args.health_file, health)
                print(
                    f"delivery acked: {delivery_id} attempt={attempt}",
                    file=sys.stderr,
                    flush=True,
                )
                if args.once:
                    return 0
            except DeliveryYielded as exc:
                attempts.pop(delivery_id, None)
                health.update(
                    {
                        "wake": "YIELDED",
                        "reply": "PENDING",
                        "ack": "PENDING",
                        "yielded_to_delivery_id": exc.delivery_id,
                        "updated_at_ms": int(time.time() * 1000),
                    }
                )
                save_health(args.health_file, health)
                print(
                    f"delivery yielded (not acked): {delivery_id} -> {exc.delivery_id}",
                    file=sys.stderr,
                    flush=True,
                )
                # No retry delay: the urgent record is already durable and
                # must immediately launch nudge.py, whose native interrupt
                # stops/steers the old Codex turn.
                continue
            except (OSError, ValueError, subprocess.SubprocessError) as exc:
                delivery_id = (
                    str(pending.get("delivery_id") or "unknown")
                    if "pending" in locals() and isinstance(pending, dict)
                    else "unknown"
                )
                print(
                    f"delivery failed (not acked): {delivery_id}: {exc}",
                    file=sys.stderr,
                    flush=True,
                )
                health.update(
                    {
                        "delivery_id": delivery_id,
                        "wake": "FAILED",
                        "ack": "PENDING",
                        "last_error": str(exc),
                        "updated_at_ms": int(time.time() * 1000),
                    }
                )
                save_health(args.health_file, health)
                if args.once:
                    return 1
                delay = min(60.0, max(0.1, args.retry_seconds) * (2 ** min(attempt - 1, 5)))
                health["retry_at_ms"] = int((time.time() + delay) * 1000)
                save_health(args.health_file, health)
                time.sleep(delay)
    finally:
        consumer_lease.close()


if __name__ == "__main__":
    raise SystemExit(main())
