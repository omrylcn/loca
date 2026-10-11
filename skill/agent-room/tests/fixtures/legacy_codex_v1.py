#!/usr/bin/env python3
"""Turn one server-filtered Loca message into a runtime-specific nudge."""

from __future__ import annotations

import argparse
import json
import os
import queue
import shutil
import subprocess
import sys
import threading
import time
from collections import deque
from pathlib import Path
from typing import Any

# Historical comparison only. This file is excluded from installed skills.
sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from wiki_state import agent_event


def read_event() -> dict[str, Any] | None:
    raw = os.environ.get("LOCA_MSG", "")
    if not raw and not sys.stdin.isatty():
        raw = sys.stdin.read()
    try:
        event = json.loads(raw)
    except (TypeError, json.JSONDecodeError) as exc:
        raise ValueError("nudge input must be one Loca JSON message") from exc
    if not isinstance(event, dict):
        raise ValueError("nudge input must be a JSON object")
    if isinstance(event.get("event"), dict) and "delivery_id" in event:
        event = event["event"]
    return agent_event(event)


def nudge_text(event: dict[str, Any]) -> str:
    if event.get("t") == "control":
        room = str(event.get("room") or "unknown")
        return (
            f"$loca immediate control in {room}: /{event.get('cmd') or 'unknown'}\n"
            "Open the Loca skill and obey this control before processing queued chat."
        )
    if event.get("t") == "care":
        signal = event.get("signal") or {}
        room = str(signal.get("room") or "unknown")
        reason = str(signal.get("reason") or "attention")
        subject = str(signal.get("subject") or "").strip()
        target = str(signal.get("target") or "").strip()
        attention_id = str(signal.get("attention_id") or "").strip()
        context = signal.get("context") or []
        lines = [
            f"- {message.get('sender') or 'unknown'}: {str(message.get('text') or '').strip()}"
            for message in context
            if isinstance(message, dict)
        ]
        context_text = "\n".join(lines) if lines else "(no source-room context was shared)"
        return (
            f"$loca care signal in {room}: {reason}; {subject}\n"
            f"Attention id: {attention_id or 'unknown'}\n"
            "Owner resolve command: SKILL_DIR/connect.sh attention-resolve "
            f'"$SERVER" {room} "$NAME" {attention_id or "<attention-id>"}\n'
            f"Target: {target or 'operator escalation'}\n"
            f"Bounded context:\n{context_text}\n"
            "You are the single attention owner for this signal. Read the Loca "
            "skill. If a nudge is useful, send exactly one direct message; "
            "otherwise stay quiet or escalate to the operator. If you own this "
            "attention and no action is needed, resolve the displayed attention "
            "id without posting filler chat. Do not create "
            "tasks or infer new work."
        )
    if event.get("t") == "reaction":
        reaction = event.get("reaction") or {}
        room = str(event.get("room") or "unknown")
        reactor = str(reaction.get("reactor") or "someone")
        emoji = str(reaction.get("emoji") or "")
        action = "added" if reaction.get("active") else "removed"
        return (
            f"$loca reaction in {room}: {reactor} {action} {emoji} on your "
            f"message #{reaction.get('message_id')}.\n"
            "This is a low-noise acknowledgement, not a request for a reply. "
            "Do not post acknowledgement-only chatter."
        )
    messages = event.get("messages") if event.get("t") == "turn" else [event]
    messages = [m for m in messages if isinstance(m, dict)]
    first = messages[0] if messages else event
    room = str(event.get("room") or first.get("room") or "unknown")
    if len(messages) > 1:
        lines = [
            f"{index}. {m.get('sender') or 'unknown'}: "
            f"{str(m.get('text') or '').strip()}"
            for index, m in enumerate(messages, start=1)
        ]
        notice = (
            f"$loca Loca queued {len(messages)} messages into one turn in "
            f"{room}:\n" + "\n".join(lines)
        )
    else:
        sender = str(first.get("sender") or "unknown")
        text = str(first.get("text") or "").strip()
        target = first.get("target")
        addressed = f" addressed to {target}" if target else ""
        notice = f"$loca Loca nudge from {sender} in {room}{addressed}: {text}"
    return (
        notice
        + "\n"
        "Open the Loca skill, pull any missing room context, and respond only "
        "when the room rules invite a reply."
    )


from codex_rpc import AppServer, response_error, classify_sandbox_failure


def active_turn_id(response: dict[str, Any]) -> str:
    """Return the currently running turn exposed by thread/resume, if any."""
    result = response.get("result")
    thread = result.get("thread") if isinstance(result, dict) else None
    if not isinstance(thread, dict):
        return ""
    turns = thread.get("turns")
    if not isinstance(turns, list):
        return ""
    for turn in reversed(turns):
        if (
            isinstance(turn, dict)
            and turn.get("status") == "inProgress"
            and turn.get("id")
        ):
            return str(turn["id"])
    return ""


def steer_active_turn(
    client: AppServer,
    thread_id: str,
    turn_id: str,
    inputs: list[dict[str, Any]],
) -> bool:
    """Deliver a Loca message into work already running in this Codex thread."""
    params: dict[str, Any] = {
        "threadId": thread_id,
        "expectedTurnId": turn_id,
        "input": inputs,
    }
    delivery_id = os.environ.get("LOCA_DELIVERY_ID", "").strip()
    if delivery_id:
        params["clientUserMessageId"] = f"loca:{delivery_id}"
    response = client.request("turn/steer", params)
    if response_error(response):
        return False
    print("Codex active turn steered")
    return True


def should_preempt_active_turn() -> bool:
    """Reserve hard preemption for a human operator's direct call.

    Agent-to-agent mentions may safely join an active turn through
    ``turn/steer``. A direct operator call is the runtime's emergency lane:
    leaving it inside a long build/test turn made an online agent appear deaf
    for minutes. The listener classifies that lane server-side and the
    consumer passes both fields through the delivery environment.
    """
    return (
        os.environ.get("LOCA_PRIORITY", "").strip() == "direct_user"
        and os.environ.get("LOCA_FROM", "").strip() == "operator"
    )


def interrupt_active_turn(
    client: AppServer,
    thread_id: str,
    turn_id: str,
) -> None:
    response = client.request(
        "turn/interrupt",
        {"threadId": thread_id, "turnId": turn_id},
    )
    if error := response_error(response):
        raise RuntimeError(f"Codex active turn interrupt failed: {error}")
    print("Codex active turn interrupted for direct operator call")


def mark_wake_accepted() -> None:
    """Publish the adapter boundary without claiming model completion."""
    raw = os.environ.get("LOCA_WAKE_RECEIPT", "").strip()
    if not raw:
        return
    path = Path(raw)
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    temp = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    temp.write_text(str(int(time.time() * 1000)) + "\n", encoding="utf-8")
    os.chmod(temp, 0o600)
    os.replace(temp, path)


def resolve_codex_bin(requested: str) -> str:
    """Resolve Codex without relying on a systemd user's minimal PATH."""
    candidate = os.path.expanduser(requested.strip())
    if os.path.sep in candidate:
        path = Path(candidate)
        if path.is_file() and os.access(path, os.X_OK):
            return str(path.resolve())
        raise FileNotFoundError(f"Codex executable is not usable: {candidate}")

    if resolved := shutil.which(candidate):
        return str(Path(resolved).resolve())

    home = Path.home()
    common = [
        home / ".bun/bin/codex",
        home / ".local/bin/codex",
    ]
    common.extend(
        sorted(
            home.glob(".vscode/extensions/openai.chatgpt-*/bin/*/codex"),
            reverse=True,
        )
    )
    for path in common:
        if path.is_file() and os.access(path, os.X_OK):
            return str(path.resolve())

    raise FileNotFoundError(
        "Codex executable not found; install Codex or set CODEX_BIN to its absolute path"
    )


def nudge_codex(
    event: dict[str, Any],
    thread_id: str,
    skill_path: Path,
    codex_bin: str,
    timeout: float,
    turn_timeout: float,
    sandbox_policy: str,
) -> int:
    if not thread_id:
        print(
            "Codex nudge skipped: no thread id; invoke $loca once in that Codex thread",
            file=sys.stderr,
        )
        return 2

    codex_bin = resolve_codex_bin(codex_bin)
    client = AppServer(codex_bin, timeout)
    try:
        initialized = client.request(
            "initialize",
            {
                "clientInfo": {
                    "name": "loca_nudge",
                    "title": "Loca nudge",
                    "version": "1.0.0",
                }
            },
        )
        if error := response_error(initialized):
            raise RuntimeError(f"Codex initialize failed: {error}")
        client.send_notification("initialized", {})

        inputs = [
            {"type": "text", "text": nudge_text(event)},
            {"type": "skill", "name": "loca", "path": str(skill_path)},
        ]
        turn_params = {
            "threadId": thread_id,
            # There is no interactive approval UI behind a systemd nudge.
            # Keep the resumed thread's existing sandbox/permission profile;
            # forcing workspaceWrite here can fail on hosts without usable
            # user namespaces (bwrap uid-map). With "never", an operation
            # outside that existing profile fails instead of waiting forever.
            "approvalPolicy": "never",
            "input": inputs,
        }
        if sandbox_policy == "danger-full-access":
            # Explicit opt-in for hosts where Codex's bwrap network namespace
            # cannot initialize. Headless Loca agents need network access to
            # read and reply to the room; never enable this implicitly.
            turn_params["sandboxPolicy"] = {"type": "dangerFullAccess"}
        # A Loca call arriving while Codex is already working is a follow-up,
        # not a second competing turn.  Waiting for the active turn to finish
        # made direct operator calls sit invisible for up to five minutes.
        # turn/steer is Codex's native same-turn delivery mechanism.
        for attempt in range(3):
            resumed = client.request("thread/resume", {"threadId": thread_id})
            if error := response_error(resumed):
                raise RuntimeError(f"Codex thread resume failed: {error}")
            if current_turn := active_turn_id(resumed):
                if should_preempt_active_turn():
                    # Human operator calls are deliberately preemptive. They
                    # must not sit behind an unrelated build or tool call for
                    # minutes. Interrupt through the app-server to keep the
                    # thread consistent; the next loop starts the Loca turn.
                    # ACK still waits for that new turn to complete.
                    interrupt_active_turn(client, thread_id, current_turn)
                    continue
                if steer_active_turn(client, thread_id, current_turn, inputs):
                    # A successful steer proves delivery into Codex, not that
                    # the agent acted.  Keep the queue item unacknowledged
                    # until that exact turn completes.
                    mark_wake_accepted()
                    completed = client.wait_for_turn(
                        thread_id, current_turn, turn_timeout
                    )
                    status = completed.get("status")
                    if status != "completed":
                        detail = completed.get("error") or status or "unknown"
                        raise RuntimeError(
                            f"Codex steered turn ended without completion: {detail}"
                        )
                    print("Codex steered turn completed")
                    return 0
                # The active turn may have completed between resume and steer.
                # Re-read state and either steer its successor or start a turn.
                continue

            started = client.request("turn/start", turn_params)
            if not (error := response_error(started)):
                break
            lowered = error.lower()
            if "active" in lowered or "in progress" in lowered:
                # A race started work after thread/resume. Loop once more and
                # steer that exact turn instead of waiting behind it.
                continue
            raise RuntimeError(f"Codex turn start failed: {error}")
        else:
            raise RuntimeError("Codex turn did not start")

        turn = started.get("result", {}).get("turn", {})
        turn_id = turn.get("id") if isinstance(turn, dict) else None
        if not turn_id:
            raise RuntimeError("Codex turn start returned no turn id")
        mark_wake_accepted()
        completed = client.wait_for_turn(thread_id, str(turn_id), turn_timeout)
        status = completed.get("status")
        if status != "completed":
            detail = completed.get("error") or status or "unknown"
            raise RuntimeError(f"Codex turn ended without completion: {detail}")
        print("Codex turn completed")
        return 0
    finally:
        client.close()


def thread_id_from(args: argparse.Namespace) -> str:
    if args.thread_id:
        return args.thread_id.strip()
    if args.thread_file:
        try:
            return Path(args.thread_file).read_text(encoding="utf-8").strip()
        except OSError:
            return ""
    return os.environ.get("CODEX_THREAD_ID", "").strip()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="runtime", required=True)
    codex = sub.add_parser("codex", help="nudge an existing Codex thread")
    codex.add_argument("--thread-id")
    codex.add_argument("--thread-file")
    codex.add_argument("--timeout", type=float, default=8.0)
    codex.add_argument("--turn-timeout", type=float, default=300.0)
    codex.add_argument("--codex-bin", default=os.environ.get("CODEX_BIN", "codex"))
    codex.add_argument(
        "--sandbox-policy",
        choices=("inherit", "danger-full-access"),
        default="inherit",
    )
    args = parser.parse_args()

    try:
        event = read_event()
        if event is None:
            return 0
        if args.runtime == "codex":
            skill_path = Path(__file__).resolve().with_name("SKILL.md")
            return nudge_codex(
                event,
                thread_id_from(args),
                skill_path,
                args.codex_bin,
                args.timeout,
                args.turn_timeout,
                args.sandbox_policy,
            )
    except (OSError, RuntimeError, TimeoutError, ValueError) as exc:
        print(f"Loca nudge failed: {exc}", file=sys.stderr)
        return 1
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
