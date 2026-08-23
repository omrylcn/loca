#!/usr/bin/env python3
"""Persistent, room-isolated Codex adapter for Adapter Protocol v2."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
import time
import uuid
from pathlib import Path
from typing import Any, Callable

from attention_store import AttentionStore, LeaseBusy, StaleLease, wall_time_ms
from nudge import AppServer, response_error


LEASE_TTL_MS = 45_000
LEASE_RENEW_MS = 5_000
ACTIVE_TURN_RECONCILE_MS = 30_000
MISSING_TURN_GRACE_MS = 60_000
MAX_CONTEXT_MESSAGES = 20
MAX_GOAL_CONTEXT_CHARS = 640
NO_REPLY_SENTINEL = "LOCA_NO_REPLY"


def bounded_value(value: Any, limit: int) -> str:
    text = str(value or "").strip()
    if len(text) <= limit:
        return text
    return text[: limit - 1].rstrip() + "…"


def missing_thread_error(error: str) -> bool:
    lowered = error.lower()
    return any(
        marker in lowered
        for marker in (
            "thread not found",
            "unknown thread",
            "does not exist",
            "no rollout found for thread id",
        )
    )


def inactive_turn_error(error: str) -> bool:
    lowered = error.lower()
    return any(
        marker in lowered
        for marker in (
            "no active turn",
            "turn is not active",
            "turn is not in progress",
        )
    )


def save_health(path: Path | None, values: dict[str, Any]) -> None:
    if path is None:
        return
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    os.chmod(path.parent, 0o700)
    payload = dict(values)
    payload["updated_at_ms"] = wall_time_ms()
    temp = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    fd = os.open(temp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            json.dump(payload, handle, sort_keys=True)
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


def delivery_messages(attention: dict[str, Any]) -> list[dict[str, Any]]:
    event = json.loads(str(attention["event_json"]))
    messages = event.get("messages") if event.get("t") == "turn" else [event]
    return [message for message in messages if isinstance(message, dict)]


def control_command(attention: dict[str, Any]) -> str:
    event = json.loads(str(attention["event_json"]))
    if event.get("t") != "control":
        return ""
    return str(event.get("cmd") or "")


def relay_target(attentions: list[dict[str, Any]]) -> str:
    fallback = "-"
    for attention in reversed(attentions):
        for message in reversed(delivery_messages(attention)):
            sender = str(message.get("sender") or "").strip()
            if sender:
                fallback = fallback if fallback != "-" else sender
                if message.get("sender_type") == "user":
                    return sender
    return fallback


def bounded_context_text(
    attention: dict[str, Any], context: list[dict[str, Any]]
) -> str:
    lines = []
    for message in context[-MAX_CONTEXT_MESSAGES:]:
        sender = str(message.get("sender") or "unknown")
        text = str(message.get("text") or "").strip()
        if text:
            lines.append(f"- {sender}: {text}")
    if not lines:
        for message in delivery_messages(attention):
            sender = str(message.get("sender") or "unknown")
            text = str(message.get("text") or "").strip()
            if text:
                lines.append(f"- {sender}: {text}")
    return "\n".join(lines) or "(no text context)"


def care_signal(attention: dict[str, Any]) -> dict[str, Any] | None:
    """Return the embedded care signal when this delivery is a care frame.

    A care delivery's event is ``{"t": "care", "signal": {...}}``: its content
    (subject/reason/source_room/context) lives inside ``signal`` and never on a
    top-level ``text`` field, so the normal message rendering path shows the
    model nothing.
    """
    event = json.loads(str(attention["event_json"]))
    if event.get("t") != "care":
        return None
    signal = event.get("signal")
    return signal if isinstance(signal, dict) else {}


def care_context_lines(
    signal: dict[str, Any], context: list[dict[str, Any]]
) -> list[str]:
    """Merge the signal's embedded context with id-range context, deduped.

    The care signal carries its own privacy-bounded context; the ContextProvider
    may independently return the delivery room's recent messages. They can
    overlap, so dedupe by message id (and by sender+text for id-less rows) and
    keep the whole result within MAX_CONTEXT_MESSAGES. The signal's own context
    is authoritative, so it is rendered first and survives the bound.
    """
    lines: list[str] = []
    seen_ids: set[int] = set()
    seen_pairs: set[tuple[str, str]] = set()

    def add(message: Any) -> None:
        if not isinstance(message, dict):
            return
        sender = str(message.get("sender") or "unknown")
        if sender == "loca-goal":
            return
        text = str(message.get("text") or "").strip()
        if not text:
            return
        message_id = message.get("id")
        if isinstance(message_id, int):
            if message_id in seen_ids:
                return
            seen_ids.add(message_id)
        pair = (sender, text)
        if pair in seen_pairs:
            return
        seen_pairs.add(pair)
        lines.append(f"- {sender}: {text}")

    for message in signal.get("context") or []:
        add(message)
    for message in context:
        add(message)
    return lines[:MAX_CONTEXT_MESSAGES]


def care_prompt_body(
    attention: dict[str, Any],
    signal: dict[str, Any],
    context: list[dict[str, Any]],
) -> str:
    reason = str(signal.get("reason") or "attention")
    subject = str(signal.get("subject") or "").strip()
    room = str(attention["room"])
    source_room = str(signal.get("source_room") or "").strip()
    stable_id = str(
        signal.get("attention_id")
        or signal.get("id")
        or attention["attention_id"]
    )
    header = f"Care signal: {reason}; {subject}" if subject else f"Care signal: {reason}"
    lines = [
        header,
        f"Care signal id: {stable_id}",
        f"Delivery room: {room}",
    ]
    if source_room:
        lines.append(f"Source room: {source_room}")
    goal_context = next(
        (
            str(message.get("text") or "").strip()
            for message in context
            if isinstance(message, dict) and message.get("sender") == "loca-goal"
        ),
        "",
    )
    if goal_context:
        lines.append(f"Goal context: {goal_context}")
    lines.append("Bounded context:")
    context_lines = care_context_lines(signal, context)
    lines.extend(context_lines or ["(no source-room context was shared)"])
    return "\n".join(lines)


def attention_prompt(
    identity: str,
    attention: dict[str, Any],
    context: list[dict[str, Any]],
) -> str:
    reply_rule = (
        "A user-visible reply is required."
        if attention["reply_required"]
        else "Reply only if the event requires a useful user-visible response."
    )
    recovery_rule = ""
    if int(attention.get("attempts") or 0) > 0:
        recovery_rule = (
            "\nRecovery: a previous runtime accepted this attention but its "
            "turn became unavailable. Inspect the current workspace and any "
            "external state before acting; do not repeat an effect that "
            "already happened."
        )
    signal = care_signal(attention)
    if signal is not None:
        body = care_prompt_body(attention, signal, context)
    else:
        body = (
            "Bounded room context:\n"
            f"{bounded_context_text(attention, context)}"
        )
    return (
        f"Loca attention for {identity} in private room {attention['room']}.\n"
        f"Attention id: {attention['attention_id']}\n"
        f"Priority: {attention['priority']}\n"
        f"{reply_rule}{recovery_rule}\n\n"
        f"{body}\n\n"
        "Respond as normal assistant text. Do not call connect.sh, do not send "
        "a Loca message with a shell command, and never request or reveal room "
        "credentials. The runtime adapter relays completed assistant messages. "
        f"If no useful room reply is warranted, respond exactly {NO_REPLY_SENTINEL}. "
        "Avoid acknowledgement-only chatter, never answer your own output, and "
        "after a useful reply wait for another participant before continuing."
    )


def stable_relay_op_id(
    bundle_ids: list[str],
    attention_ids: list[str],
    turn_id: str,
    item_id: str,
    relay_kind: str,
) -> str:
    source = "\n".join(
        bundle_ids + attention_ids + [turn_id, item_id, relay_kind]
    )
    digest = hashlib.sha256(source.encode()).hexdigest()
    return f"loca-v2-{digest}"


class ConnectShRelay:
    def __init__(self, connect_sh: Path):
        self.connect_sh = connect_sh

    def __call__(self, relay: dict[str, Any]) -> None:
        env = dict(os.environ)
        env["LOCA_OP_ID"] = str(relay["op_id"])
        result = subprocess.run(
            [
                str(self.connect_sh),
                "send",
                str(relay["server"]),
                str(relay["room"]),
                str(relay["identity"]),
                str(relay["target"]),
                str(relay["text"]),
            ],
            env=env,
            text=True,
            capture_output=True,
            timeout=20,
            check=False,
        )
        if result.returncode:
            detail = (result.stderr or result.stdout or "send failed").strip()
            raise RuntimeError(detail[-500:])


class ContextProvider:
    def __init__(self, connect_sh: Path):
        self.connect_sh = connect_sh

    def __call__(self, attention: dict[str, Any]) -> list[dict[str, Any]]:
        from_id = attention.get("context_from_id")
        since = max(0, int(from_id or 0) - MAX_CONTEXT_MESSAGES - 1)
        result = subprocess.run(
            [
                str(self.connect_sh),
                "since",
                str(attention["server"]),
                str(attention["room"]),
                str(since),
            ],
            text=True,
            capture_output=True,
            timeout=15,
            check=False,
        )
        if result.returncode:
            messages = []
        else:
            try:
                messages = json.loads(result.stdout)
            except json.JSONDecodeError:
                messages = []
        if not isinstance(messages, list):
            messages = []
        upper = attention.get("context_to_id")
        bounded = [
            message
            for message in messages
            if isinstance(message, dict)
            and (
                not isinstance(upper, int)
                or not isinstance(message.get("id"), int)
                or int(message["id"]) <= upper
            )
        ]
        bounded = bounded[-MAX_CONTEXT_MESSAGES:]

        # Goal is durable room context, not a task and not a second model call.
        # Inject it only into the Goal reminder turn: ordinary chat, waits,
        # tasks and silence checks keep their exact previous prompt shape.
        signal = care_signal(attention)
        if signal is None or signal.get("reason") != "goal_reminder":
            return bounded
        try:
            goal_result = subprocess.run(
                [
                    str(self.connect_sh),
                    "goals",
                    str(attention["server"]),
                    str(attention["room"]),
                ],
                text=True,
                capture_output=True,
                # Goal enriches an already accepted attention; a flaky local
                # resolver must never add connect.sh's full 10-second network
                # timeout to the reply SLA.
                timeout=3,
                check=False,
            )
        except subprocess.TimeoutExpired:
            goal_result = None
        if goal_result is not None and goal_result.returncode == 0:
            try:
                goals = json.loads(goal_result.stdout)
            except json.JSONDecodeError:
                goals = []
            if isinstance(goals, list):
                active = next(
                    (
                        goal
                        for goal in goals
                        if isinstance(goal, dict)
                        and goal.get("status") == "active"
                    ),
                    None,
                )
                if active:
                    outcome = bounded_value(active.get("outcome"), 280)
                    checkpoint = bounded_value(active.get("checkpoint"), 180)
                    completion = str(active.get("completion") or "manual")
                    task_ids = active.get("task_ids")
                    if completion == "all_tasks":
                        count = len(task_ids) if isinstance(task_ids, list) else 0
                        condition = f"all {count} linked tasks finished"
                    else:
                        condition = "operator verifies the outcome"
                    try:
                        settings_result = subprocess.run(
                            [
                                str(self.connect_sh),
                                "settings",
                                str(attention["server"]),
                                str(attention["room"]),
                            ],
                            text=True,
                            capture_output=True,
                            timeout=3,
                            check=False,
                        )
                    except subprocess.TimeoutExpired:
                        settings_result = None
                    settings: Any = {}
                    if settings_result is not None and settings_result.returncode == 0:
                        try:
                            settings = json.loads(settings_result.stdout)
                        except json.JSONDecodeError:
                            settings = {}
                    if not isinstance(settings, dict):
                        settings = {}
                    lead = bounded_value(settings.get("lead") or "unassigned", 80)
                    text = (
                        f"Active goal: {outcome}. Lead: @{lead}. "
                        f"Completion condition: {condition}"
                    )
                    if checkpoint:
                        text += f". Next observable proof: {checkpoint}"
                    if len(text) > MAX_GOAL_CONTEXT_CHARS:
                        text = text[: MAX_GOAL_CONTEXT_CHARS - 1].rstrip() + "…"
                    bounded.append(
                        {
                            "sender": "loca-goal",
                            "text": text,
                            "kind": "context",
                        }
                    )
        return bounded


class PersistentCodexAdapter:
    def __init__(
        self,
        *,
        store: AttentionStore,
        inbox: Path,
        identity: str,
        workdir: Path,
        codex_bin: str,
        relay_mode: str,
        relay: Callable[[dict[str, Any]], None],
        context_provider: Callable[[dict[str, Any]], list[dict[str, Any]]],
        health_file: Path | None = None,
        sandbox: str = "inherit",
        replay_existing: bool = False,
        app_server_factory: Callable[[str, float], AppServer] = AppServer,
    ):
        self.store = store
        self.inbox = inbox
        self.identity = identity
        self.workdir = workdir
        self.codex_bin = codex_bin
        self.relay_mode = relay_mode
        self.relay = relay
        self.context_provider = context_provider
        self.health_file = health_file
        self.sandbox = sandbox
        self.replay_existing = replay_existing
        self.owner = f"{os.getpid()}-{uuid.uuid4().hex}"
        self.epoch = 0
        self.client = app_server_factory(codex_bin, 10.0)
        self.active_turns: dict[str, str] = {}
        self.thread_scopes: dict[str, tuple[str, str]] = {}
        self.scope_threads: dict[tuple[str, str], str] = {}
        self.next_lease_renew_ms = 0
        self.next_turn_reconcile_ms = 0
        self.health: dict[str, Any] = {
            "protocol_version": "2",
            "adapter": "persistent-codex",
            "ingestion": "STARTING",
            "wake": "IDLE",
            "reply": "UNVERIFIED",
            "ack": "IDLE",
            "relay_mode": relay_mode,
            "stored": False,
            "accepted": False,
            "first_response": False,
            "final_response": False,
            "turn_completed": False,
        }

    def initialize(self) -> None:
        initial_offset = self.store.initialize_ingestion_source(
            self.inbox,
            self.identity,
            replay_existing=self.replay_existing,
        )
        self.epoch = self.store.claim_lease(
            self.identity, self.owner, LEASE_TTL_MS
        )
        self.next_lease_renew_ms = wall_time_ms() + LEASE_RENEW_MS
        initialized = self.client.request(
            "initialize",
            {
                "clientInfo": {
                    "name": "loca_runtime_v2",
                    "title": "Loca persistent runtime",
                    "version": "2.0.0",
                }
            },
        )
        if error := response_error(initialized):
            raise RuntimeError(f"Codex initialize failed: {error}")
        self.client.send_notification("initialized", {})
        self.recover_existing_threads()
        self.next_turn_reconcile_ms = (
            wall_time_ms() + ACTIVE_TURN_RECONCILE_MS
        )
        self.health.update(
            {
                "ingestion": "OK",
                "last_ingestion_offset": initial_offset,
                "lease_epoch": self.epoch,
                "wake": "IDLE",
            }
        )
        # The SQLite ledger, not the previous process's memory or health file,
        # is authoritative across adapter restarts.
        self.refresh_progress_health()
        save_health(self.health_file, self.health)

    def recover_existing_threads(self) -> None:
        for saved in self.store.list_threads(self.identity):
            thread_id = str(saved["thread_id"])
            server = str(saved["server"])
            room = str(saved["room"])
            response = self.client.request(
                "thread/resume", self.thread_resume_params(thread_id, room)
            )
            if error := response_error(response):
                if not missing_thread_error(error):
                    raise RuntimeError(
                        f"Codex thread resume failed for {thread_id}: {error}"
                    )
                for intent in self.store.unresolved_dispatches(
                    self.identity, thread_id
                ):
                    self.store.retry_unobserved_dispatch(
                        str(intent["attention_id"]),
                        "prepared dispatch absent because Codex thread is missing",
                        self.owner,
                        self.epoch,
                    )
                self.store.requeue_missing_thread(
                    self.identity,
                    thread_id,
                    f"Codex thread missing during recovery: {error}",
                    self.owner,
                    self.epoch,
                )
                continue
            self.thread_scopes[thread_id] = (server, room)
            self.scope_threads[(server, room)] = thread_id
            self.store.set_thread(
                self.identity,
                server,
                room,
                thread_id,
                self.owner,
                self.epoch,
            )
            result = response.get("result")
            thread = result.get("thread") if isinstance(result, dict) else None
            turns = self.authoritative_turns(thread_id, thread)
            self.reconcile_dispatches(thread_id, turns)
            self.reconcile_accepted_turns(thread_id, turns)
            if not isinstance(turns, list):
                continue
            for turn in turns:
                if not isinstance(turn, dict) or not turn.get("id"):
                    continue
                turn_id = str(turn["id"])
                if turn.get("status") == "inProgress":
                    self.active_turns[thread_id] = turn_id
                self.recover_turn(turn_id, thread_id, turn)

    def authoritative_turns(
        self, thread_id: str, fallback_thread: dict[str, Any] | None
    ) -> list[dict[str, Any]]:
        """Read durable rollout history before deciding a dispatch was lost."""
        response = self.client.request(
            "thread/read", {"threadId": thread_id, "includeTurns": True}
        )
        if error := response_error(response):
            raise RuntimeError(
                f"Codex thread/read failed during dispatch reconciliation: {error}"
            )
        result = response.get("result")
        thread = result.get("thread") if isinstance(result, dict) else None
        if not isinstance(thread, dict):
            thread = fallback_thread
        turns = thread.get("turns") if isinstance(thread, dict) else []
        return [turn for turn in turns if isinstance(turn, dict)]

    @staticmethod
    def turn_for_client_message(
        turns: list[dict[str, Any]], client_message_id: str
    ) -> str | None:
        for turn in turns:
            turn_id = str(turn.get("id") or "")
            for item in turn.get("items") or []:
                if (
                    isinstance(item, dict)
                    and item.get("type") == "userMessage"
                    and item.get("clientId") == client_message_id
                ):
                    return turn_id or None
        return None

    def reconcile_dispatches(
        self, thread_id: str, turns: list[dict[str, Any]]
    ) -> None:
        """Bind crash-window intents to Codex history or safely retry them."""
        for intent in self.store.unresolved_dispatches(self.identity, thread_id):
            attention_id = str(intent["attention_id"])
            turn_id = self.turn_for_client_message(
                turns, str(intent["client_message_id"])
            )
            if turn_id:
                self.store.finalize_dispatch(
                    attention_id,
                    thread_id,
                    turn_id,
                    self.owner,
                    self.epoch,
                    reconciled=True,
                )
                continue
            self.store.retry_unobserved_dispatch(
                attention_id,
                "prepared dispatch not present in authoritative Codex history; retrying",
                self.owner,
                self.epoch,
            )

    def reconcile_accepted_turns(
        self,
        thread_id: str,
        turns: list[dict[str, Any]],
        *,
        force: bool = False,
    ) -> int:
        """Requeue accepted output-free turns absent from Codex history."""
        durable_ids = {
            str(turn.get("id")) for turn in turns if turn.get("id")
        }
        requeued = 0
        older_than_ms = None if force else wall_time_ms() - MISSING_TURN_GRACE_MS
        for turn_id in self.store.incomplete_turn_ids(
            self.identity, thread_id, older_than_ms=older_than_ms
        ):
            if turn_id in durable_ids:
                continue
            reason = (
                "Accepted Codex turn is absent from durable thread history "
                f"after adapter recovery: {turn_id}"
            )
            requeued += self.store.requeue_missing_turn(
                self.identity,
                thread_id,
                turn_id,
                reason,
                self.owner,
                self.epoch,
            )
        return requeued

    def reconcile_active_turn(
        self,
        thread_id: str,
        turn_id: str,
        *,
        force_orphan: bool = False,
    ) -> bool:
        """Reconcile a turn whose completion event may have been lost.

        App-server can acknowledge ``turn/start`` before the turn is present in
        durable thread history. If that turn then disappears, no
        ``turn/completed`` notification can arrive. A later steer reports that
        there is no active turn, while the adapter's in-memory map otherwise
        remains stuck forever.
        """
        turns = self.authoritative_turns(thread_id, None)
        turn = next(
            (candidate for candidate in turns if candidate.get("id") == turn_id),
            None,
        )
        if turn is not None and turn.get("status") != "inProgress":
            self.recover_turn(turn_id, thread_id, turn)
            return True
        if turn is not None and not force_orphan:
            return False
        reason = (
            "Codex reported no active turn and durable thread history has no "
            f"terminal record for {turn_id}; replaying output-free attention"
        )
        requeued = self.store.requeue_missing_turn(
            self.identity,
            thread_id,
            turn_id,
            reason,
            self.owner,
            self.epoch,
        )
        if requeued:
            if self.active_turns.get(thread_id) == turn_id:
                self.active_turns.pop(thread_id, None)
            self.health.update(
                {
                    "wake": "RETRYING",
                    "ack": "PENDING",
                    "last_error": reason,
                }
            )
            save_health(self.health_file, self.health)
            return True
        return False

    def reconcile_active_turns_if_due(self) -> int:
        now = wall_time_ms()
        if now < self.next_turn_reconcile_ms:
            return 0
        self.next_turn_reconcile_ms = now + ACTIVE_TURN_RECONCILE_MS
        reconciled = 0
        for thread_id, turn_id in list(self.active_turns.items()):
            reconciled += int(self.reconcile_active_turn(thread_id, turn_id))
        return reconciled

    def renew_lease_if_needed(self) -> None:
        now = wall_time_ms()
        if now < self.next_lease_renew_ms:
            return
        self.store.renew_lease(
            self.identity, self.owner, self.epoch, LEASE_TTL_MS, now_ms=now
        )
        self.next_lease_renew_ms = now + LEASE_RENEW_MS

    def developer_instructions(self, room: str) -> str:
        return (
            "You are a dedicated headless Loca runtime thread for "
            f"identity {self.identity} in private room {room}. Process "
            "only attention supplied by the adapter. Never call Loca "
            "send commands for an incoming attention; normal assistant "
            "messages are relayed by the adapter. Never expose credentials."
        )

    def thread_resume_params(self, thread_id: str, room: str) -> dict[str, Any]:
        """Re-apply runtime policy when a durable Codex thread is resumed.

        Codex persists sandbox and cwd settings with a thread. Reusing a
        thread first opened with ``workspace-write`` otherwise makes a later
        ``--sandbox danger-full-access`` runtime report the new flag while
        turns still execute with the old policy.
        """
        params: dict[str, Any] = {
            "threadId": thread_id,
            "cwd": str(self.workdir),
            "approvalPolicy": "never",
            "developerInstructions": self.developer_instructions(room),
        }
        if self.sandbox != "inherit":
            params["sandbox"] = self.sandbox
        return params

    def turn_sandbox_policy(self) -> dict[str, Any] | None:
        """Return an explicit per-turn policy for resumed-thread safety."""
        if self.sandbox == "danger-full-access":
            return {"type": "dangerFullAccess"}
        if self.sandbox == "read-only":
            return {"type": "readOnly", "networkAccess": False}
        if self.sandbox == "workspace-write":
            return {
                "type": "workspaceWrite",
                "writableRoots": [str(self.workdir)],
                "networkAccess": False,
            }
        return None

    def create_thread(self, server: str, room: str) -> str:
        params: dict[str, Any] = {
            "cwd": str(self.workdir),
            "approvalPolicy": "never",
            "ephemeral": False,
            "developerInstructions": self.developer_instructions(room),
        }
        if self.sandbox != "inherit":
            params["sandbox"] = self.sandbox
        response = self.client.request("thread/start", params)
        if error := response_error(response):
            raise RuntimeError(f"Codex thread start failed: {error}")
        result = response.get("result")
        thread = result.get("thread") if isinstance(result, dict) else None
        thread_id = str(thread.get("id") or "") if isinstance(thread, dict) else ""
        if not thread_id:
            raise RuntimeError("Codex thread/start returned no thread id")
        self.store.set_thread(
            self.identity,
            server,
            room,
            thread_id,
            self.owner,
            self.epoch,
        )
        self.thread_scopes[thread_id] = (server, room)
        self.scope_threads[(server, room)] = thread_id
        return thread_id

    def ensure_thread(self, attention: dict[str, Any]) -> str:
        server = str(attention["server"])
        room = str(attention["room"])
        if cached := self.scope_threads.get((server, room)):
            return cached
        thread_id = self.store.get_thread(self.identity, server, room)
        if not thread_id:
            return self.create_thread(server, room)
        resumed = self.client.request(
            "thread/resume", self.thread_resume_params(thread_id, room)
        )
        if error := response_error(resumed):
            if not missing_thread_error(error):
                raise RuntimeError(
                    f"Codex thread resume failed for {thread_id}: {error}"
                )
            self.store.requeue_missing_thread(
                self.identity,
                thread_id,
                f"Codex thread missing during dispatch: {error}",
                self.owner,
                self.epoch,
            )
            return self.create_thread(server, room)
        self.thread_scopes[thread_id] = (server, room)
        self.scope_threads[(server, room)] = thread_id
        result = resumed.get("result")
        thread = result.get("thread") if isinstance(result, dict) else None
        turns = self.authoritative_turns(thread_id, thread)
        self.reconcile_dispatches(thread_id, turns)
        self.reconcile_accepted_turns(thread_id, turns)
        if isinstance(turns, list):
            for turn in turns:
                if not isinstance(turn, dict) or not turn.get("id"):
                    continue
                turn_id = str(turn["id"])
                if turn.get("status") == "inProgress":
                    self.active_turns[thread_id] = turn_id
                self.recover_turn(turn_id, thread_id, turn)
        return thread_id

    def recover_turn(
        self, turn_id: str, thread_id: str, turn: dict[str, Any]
    ) -> None:
        for item in turn.get("items") or []:
            if isinstance(item, dict):
                self.handle_agent_item(thread_id, turn_id, item)
        status = str(turn.get("status") or "")
        if status and status != "inProgress":
            self.handle_turn_completed(
                thread_id,
                {"id": turn_id, "status": status, "error": turn.get("error")},
            )

    def dispatch_one(self) -> bool:
        attention = self.store.next_pending(self.identity)
        if attention is None:
            return False
        command = control_command(attention)
        if command == "stop":
            return self.handle_stop_control(attention)
        if command == "room-closed":
            return self.handle_room_closed_control(attention)
        thread_id = self.ensure_thread(attention)
        context = self.context_provider(attention)
        inputs = [{"type": "text", "text": attention_prompt(
            self.identity, attention, context
        )}]
        client_message_id = f"loca:{attention['attention_id']}"
        active_turn = self.active_turns.get(thread_id)
        operation = "steer" if active_turn else "start"
        self.store.prepare_dispatch(
            str(attention["attention_id"]),
            thread_id,
            operation,
            client_message_id,
            self.owner,
            self.epoch,
        )
        if active_turn:
            response = self.client.request(
                "turn/steer",
                {
                    "threadId": thread_id,
                    "expectedTurnId": active_turn,
                    "input": inputs,
                    "clientUserMessageId": client_message_id,
                },
            )
            if error := response_error(response):
                recovered_inactive = False
                if inactive_turn_error(error):
                    recovered_inactive = self.reconcile_active_turn(
                        thread_id, active_turn, force_orphan=True
                    )
                self.store.retry_unobserved_dispatch(
                    str(attention["attention_id"]),
                    f"turn/steer rejected before acceptance: {error}",
                    self.owner,
                    self.epoch,
                )
                self.store.defer_attention(
                    str(attention["attention_id"]),
                    f"turn/steer rejected: {error}",
                    0 if recovered_inactive else 1_000,
                    self.owner,
                    self.epoch,
                )
                return False
            result = response.get("result")
            returned_turn = (
                str(result.get("turnId") or "")
                if isinstance(result, dict)
                else ""
            )
            turn_id = returned_turn or active_turn
            # AppServer.request preserves notifications read before the RPC
            # response in ``pending``.  Those items predate acceptance of this
            # steer and must not claim the new attention as covered output.
            while self.client.pending:
                self.handle_message(self.client.pending.pop(0))
        else:
            turn_params: dict[str, Any] = {
                "threadId": thread_id,
                "approvalPolicy": "never",
                "input": inputs,
                "clientUserMessageId": client_message_id,
            }
            if sandbox_policy := self.turn_sandbox_policy():
                turn_params["sandboxPolicy"] = sandbox_policy
            response = self.client.request("turn/start", turn_params)
            if error := response_error(response):
                self.store.retry_unobserved_dispatch(
                    str(attention["attention_id"]),
                    f"turn/start rejected before acceptance: {error}",
                    self.owner,
                    self.epoch,
                )
                self.store.defer_attention(
                    str(attention["attention_id"]),
                    f"turn/start rejected: {error}",
                    1_000,
                    self.owner,
                    self.epoch,
                )
                return False
            result = response.get("result")
            turn = result.get("turn") if isinstance(result, dict) else None
            turn_id = str(turn.get("id") or "") if isinstance(turn, dict) else ""
            if not turn_id:
                raise RuntimeError("Codex turn/start returned no turn id")
            self.active_turns[thread_id] = turn_id
        self.store.finalize_dispatch(
            str(attention["attention_id"]),
            thread_id,
            turn_id,
            self.owner,
            self.epoch,
        )
        self.health.update(
            {
                "wake": "OK",
                "ack": "OK",
                "last_attention_id": attention["attention_id"],
                "last_accepted_attention_id": attention["attention_id"],
                "last_turn_id": turn_id,
                "stored": True,
                "accepted": True,
                "first_response": False,
                "final_response": False,
                "turn_completed": False,
            }
        )
        save_health(self.health_file, self.health)
        return True

    def handle_stop_control(self, attention: dict[str, Any]) -> bool:
        server = str(attention["server"])
        room = str(attention["room"])
        thread_id = self.scope_threads.get((server, room)) or self.store.get_thread(
            self.identity, server, room
        )
        if thread_id:
            thread_id = self.ensure_thread(attention)
            active_turn = self.active_turns.get(thread_id)
            if active_turn:
                response = self.client.request(
                    "turn/interrupt",
                    {"threadId": thread_id, "turnId": active_turn},
                )
                if error := response_error(response):
                    self.store.defer_attention(
                        str(attention["attention_id"]),
                        f"turn/interrupt rejected: {error}",
                        1_000,
                        self.owner,
                        self.epoch,
                    )
                    return False
                # Interrupt acceptance is the durable boundary. Do not leave
                # the old work recoverable if the adapter dies before Codex
                # emits its eventual turn/completed notification.
                self.store.interrupt_turn_for_control(
                    active_turn,
                    "explicit Loca stop control",
                    self.owner,
                    self.epoch,
                )
                self.active_turns.pop(thread_id, None)
        self.store.cancel_pending_before_control(
            str(attention["attention_id"]),
            "explicit Loca stop control",
            self.owner,
            self.epoch,
        )
        self.health.update(
            {
                "wake": "OK",
                "ack": "OK",
                "last_attention_id": attention["attention_id"],
                "last_accepted_attention_id": attention["attention_id"],
                "control": "STOPPED",
            }
        )
        save_health(self.health_file, self.health)
        return True

    def handle_room_closed_control(self, attention: dict[str, Any]) -> bool:
        """Terminate room work permanently; a closed room cannot be retried."""
        server = str(attention["server"])
        room = str(attention["room"])
        thread_id = self.scope_threads.get((server, room)) or self.store.get_thread(
            self.identity, server, room
        )
        interrupt_error = ""
        if thread_id:
            active_turn = self.active_turns.get(thread_id)
            if active_turn:
                response = self.client.request(
                    "turn/interrupt",
                    {"threadId": thread_id, "turnId": active_turn},
                )
                interrupt_error = response_error(response)
                self.active_turns.pop(thread_id, None)
            self.scope_threads.pop((server, room), None)
            self.thread_scopes.pop(thread_id, None)
        self.store.close_room_for_control(
            str(attention["attention_id"]),
            "Loca room permanently closed",
            self.owner,
            self.epoch,
        )
        self.health.update(
            {
                "wake": "OK",
                "ack": "OK",
                "last_attention_id": attention["attention_id"],
                "last_accepted_attention_id": attention["attention_id"],
                "control": "ROOM_CLOSED",
            }
        )
        if interrupt_error:
            self.health["last_error"] = (
                "room closed; Codex interrupt was rejected but late output "
                f"is fenced: {interrupt_error}"
            )
        save_health(self.health_file, self.health)
        return True

    def refresh_progress_health(self) -> None:
        progress = self.store.progress_summary(self.identity)
        latest = self.store.latest_lifecycle(self.identity)
        pending = int(progress["reply_required_pending"])
        awaiting_first = int(progress["reply_awaiting_first"])
        if pending == 0:
            reply = (
                "FINAL"
                if latest is not None
                and latest["final_response_at_ms"] is not None
                else "IDLE"
            )
        elif awaiting_first:
            reply = "PENDING"
        else:
            reply = "FIRST"
        self.health.update(progress)
        self.health.update(
            {
                "reply": reply,
                "last_attention_id": (
                    latest["attention_id"] if latest is not None else None
                ),
                "last_accepted_attention_id": (
                    latest["attention_id"]
                    if latest is not None
                    and latest["accepted_at_ms"] is not None
                    else None
                ),
                "last_turn_id": (
                    latest["turn_id"] if latest is not None else None
                ),
                "stored": latest is not None,
                "accepted": (
                    latest is not None and latest["accepted_at_ms"] is not None
                ),
                "first_response": (
                    latest is not None
                    and latest["first_response_at_ms"] is not None
                ),
                "final_response": (
                    latest is not None
                    and latest["final_response_at_ms"] is not None
                ),
                "turn_completed": (
                    latest is not None
                    and latest["turn_completed_at_ms"] is not None
                ),
                "terminal_status": (
                    latest["terminal_status"] if latest is not None else None
                ),
            }
        )

    def relay_metadata(
        self, turn_id: str
    ) -> tuple[list[dict[str, Any]], list[str], str, str, str]:
        attentions = self.store.accepted_for_turn(turn_id)
        if not attentions:
            return [], [], "", "", "-"
        rooms = {str(attention["room"]) for attention in attentions}
        servers = {str(attention["server"]) for attention in attentions}
        if len(rooms) != 1 or len(servers) != 1:
            raise RuntimeError("cross-room attention detected in one Codex turn")
        ids = [str(attention["attention_id"]) for attention in attentions]
        return (
            attentions,
            ids,
            servers.pop(),
            rooms.pop(),
            relay_target(attentions),
        )

    def create_output(
        self,
        *,
        turn_id: str,
        item_id: str,
        phase: str | None,
        text: str,
        relay_kind: str,
        status: str | None = None,
    ) -> None:
        attentions, attention_ids, _, room, target = self.relay_metadata(turn_id)
        if not attention_ids:
            return
        bundle_ids = list(
            dict.fromkeys(
                str(attention["bundle_id"]) for attention in attentions
            )
        )
        op_id = stable_relay_op_id(
            bundle_ids, attention_ids, turn_id, item_id, relay_kind
        )
        default_status = "shadow" if self.relay_mode == "shadow" else "pending"
        self.store.create_relay_item(
            op_id=op_id,
            identity=self.identity,
            room=room,
            target=target,
            turn_id=turn_id,
            item_id=item_id,
            phase=phase,
            text=text,
            covered_attention_ids=attention_ids,
            relay_kind=relay_kind,
            owner=self.owner,
            epoch=self.epoch,
            status=status or default_status,
        )

    def handle_agent_item(
        self, thread_id: str, turn_id: str, item: dict[str, Any]
    ) -> None:
        if item.get("type") != "agentMessage":
            return
        text = str(item.get("text") or "").strip()
        item_id = str(item.get("id") or "")
        if not text or not item_id:
            return
        if text == NO_REPLY_SENTINEL:
            self.create_output(
                turn_id=turn_id,
                item_id=item_id,
                phase=item.get("phase"),
                text=text,
                relay_kind="no_reply",
                status="suppressed",
            )
            self.store.mark_turn_no_reply(
                turn_id,
                self.owner,
                self.epoch,
            )
            return
        phase = item.get("phase")
        if phase == "commentary":
            if self.store.turn_has_any_relay_kind(turn_id, "commentary"):
                self.create_output(
                    turn_id=turn_id,
                    item_id=item_id,
                    phase="commentary",
                    text=text,
                    relay_kind="commentary_suppressed",
                    status="suppressed",
                )
            else:
                self.create_output(
                    turn_id=turn_id,
                    item_id=item_id,
                    phase="commentary",
                    text=text,
                    relay_kind="commentary",
                )
            return
        if phase == "final_answer":
            if self.store.turn_has_any_relay_kind(turn_id, "final"):
                self.create_output(
                    turn_id=turn_id,
                    item_id=item_id,
                    phase="final_answer",
                    text=text,
                    relay_kind="final_suppressed",
                    status="suppressed",
                )
            else:
                self.create_output(
                    turn_id=turn_id,
                    item_id=item_id,
                    phase="final_answer",
                    text=text,
                    relay_kind="final",
                )
            return
        self.create_output(
            turn_id=turn_id,
            item_id=item_id,
            phase=None,
            text=text,
            relay_kind="candidate",
            status="held",
        )

    def handle_turn_completed(
        self, thread_id: str, turn: dict[str, Any]
    ) -> None:
        turn_id = str(turn.get("id") or "")
        if not turn_id:
            return
        status = str(turn.get("status") or "failed")
        error = turn.get("error")
        reason = json.dumps(error, ensure_ascii=False) if error else ""
        if (
            status == "completed"
            and not self.store.turn_has_any_relay_kind(turn_id, "final")
        ):
            candidate = self.store.latest_unknown_item(turn_id)
            if candidate is not None:
                self.create_output(
                    turn_id=turn_id,
                    item_id=f"{candidate['item_id']}:fallback",
                    phase=None,
                    text=str(candidate["text"]),
                    relay_kind="final",
                )
        mapped_status = {
            "completed": "completed",
            "interrupted": "interrupted",
            "failed": "failed",
            "cancelled": "cancelled",
            "expired": "expired",
        }.get(status, "failed")
        self.store.mark_turn_completed(
            turn_id,
            mapped_status,
            reason,
            self.owner,
            self.epoch,
        )
        if self.active_turns.get(thread_id) == turn_id:
            self.active_turns.pop(thread_id, None)
        self.health.update(
            {
                "turn": mapped_status.upper(),
                "last_completed_turn_id": turn_id,
                "turn_completed": True,
            }
        )
        save_health(self.health_file, self.health)

    def handle_message(self, message: dict[str, Any]) -> None:
        if "id" in message and message.get("method"):
            raise RuntimeError(
                "Codex turn blocked on unsupported headless client request: "
                f"{message['method']}"
            )
        method = message.get("method")
        params = message.get("params")
        if not isinstance(params, dict):
            return
        if method == "item/completed":
            item = params.get("item")
            if isinstance(item, dict):
                self.handle_agent_item(
                    str(params.get("threadId") or ""),
                    str(params.get("turnId") or ""),
                    item,
                )
        elif method == "turn/completed":
            turn = params.get("turn")
            if isinstance(turn, dict):
                self.handle_turn_completed(
                    str(params.get("threadId") or ""), turn
                )

    def drain_messages(self) -> int:
        count = 0
        while self.client.pending:
            self.handle_message(self.client.pending.pop(0))
            count += 1
        while True:
            message = self.client.read_message(time.monotonic() + 0.002)
            if message is None:
                break
            self.handle_message(message)
            count += 1
        return count

    def process_relays(self) -> int:
        if self.relay_mode != "live":
            return 0
        sent = 0
        for relay in self.store.pending_relays(self.identity):
            self.renew_lease_if_needed()
            payload = dict(relay)
            with self.store.connect() as connection:
                turn = connection.execute(
                    "SELECT server FROM turns WHERE turn_id = ?",
                    (relay["turn_id"],),
                ).fetchone()
            if turn is None:
                continue
            payload["server"] = str(turn["server"])
            try:
                self.store.assert_lease(
                    self.identity, self.owner, self.epoch
                )
                self.relay(payload)
                self.store.mark_relay_accepted(
                    str(relay["op_id"]), self.owner, self.epoch
                )
            except (OSError, RuntimeError, subprocess.SubprocessError) as error:
                self.store.mark_relay_failed(
                    str(relay["op_id"]), str(error), self.owner, self.epoch
                )
                continue
            kind = str(relay["relay_kind"])
            self.health.update(
                {
                    "reply": "FINAL" if kind == "final" else "FIRST",
                    "last_relay_op_id": relay["op_id"],
                    "first_response": True,
                    "final_response": kind == "final"
                    or bool(self.health.get("final_response")),
                }
            )
            save_health(self.health_file, self.health)
            sent += 1
        self.refresh_progress_health()
        return sent

    def cycle(self) -> dict[str, int]:
        self.renew_lease_if_needed()
        ingested = self.store.ingest_inbox(self.inbox, self.identity)
        self.health["ingestion"] = "OK"
        self.health["last_ingestion_offset"] = ingested["offset"]
        if int(ingested["inserted"]) > 0:
            self.health["stored"] = True
        events = self.drain_messages()
        reconciled = self.reconcile_active_turns_if_due()
        relayed = 0
        dispatched = 0
        # Dispatch the highest-priority pending attention before releasing an
        # older output. In particular, a durable room-closed control ingested
        # during restart must fence a recovered final relay before that relay
        # can escape to a room that no longer exists.
        # Bound one cycle so an unbounded producer cannot starve event/relay
        # handling. The next cycle resumes immediately.
        for _ in range(16):
            if not self.dispatch_one():
                break
            dispatched += 1
            events += self.drain_messages()
            relayed += self.process_relays()
        # A cycle with no dispatch still has to advance retryable output.
        relayed += self.process_relays()
        self.refresh_progress_health()
        save_health(self.health_file, self.health)
        return {
            "ingested": int(ingested["inserted"]),
            "events": events,
            "reconciled": reconciled,
            "relayed": relayed,
            "dispatched": dispatched,
        }

    def close(self) -> None:
        try:
            if self.epoch:
                self.store.release_lease(
                    self.identity, self.owner, self.epoch
                )
        finally:
            self.client.close()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--inbox", type=Path, required=True)
    parser.add_argument("--state-db", type=Path, required=True)
    parser.add_argument("--identity", required=True)
    parser.add_argument("--workdir", type=Path, required=True)
    parser.add_argument("--codex-bin", default=os.environ.get("CODEX_BIN", "codex"))
    parser.add_argument(
        "--connect-sh",
        type=Path,
        default=Path(__file__).resolve().with_name("connect.sh"),
    )
    parser.add_argument("--health-file", type=Path)
    parser.add_argument("--relay-mode", choices=("shadow", "live"), default="shadow")
    parser.add_argument(
        "--sandbox",
        choices=("inherit", "workspace-write", "danger-full-access"),
        default="inherit",
    )
    parser.add_argument("--poll-seconds", type=float, default=0.1)
    parser.add_argument("--once", action="store_true")
    parser.add_argument(
        "--replay-existing",
        action="store_true",
        help="ingest a pre-existing inbox on first start (recovery/testing only)",
    )
    args = parser.parse_args()

    store = AttentionStore(args.state_db)
    adapter = PersistentCodexAdapter(
        store=store,
        inbox=args.inbox,
        identity=args.identity,
        workdir=args.workdir,
        codex_bin=args.codex_bin,
        relay_mode=args.relay_mode,
        relay=ConnectShRelay(args.connect_sh),
        context_provider=ContextProvider(args.connect_sh),
        health_file=args.health_file,
        sandbox=args.sandbox,
        replay_existing=args.replay_existing,
    )
    try:
        adapter.initialize()
        while True:
            adapter.cycle()
            if args.once:
                return 0
            time.sleep(max(0.02, args.poll_seconds))
    except LeaseBusy as error:
        print(f"persistent adapter lease busy: {error}", file=sys.stderr)
        return 4
    except (OSError, RuntimeError, StaleLease, ValueError) as error:
        print(f"persistent adapter failed: {error}", file=sys.stderr)
        save_health(
            args.health_file,
            {
                "protocol_version": "2",
                "adapter": "persistent-codex",
                "wake": "FAILED",
                "ack": "PENDING",
                "last_error": str(error),
                "relay_mode": args.relay_mode,
            },
        )
        return 1
    finally:
        adapter.close()


if __name__ == "__main__":
    raise SystemExit(main())
