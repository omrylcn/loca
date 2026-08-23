#!/usr/bin/env python3
"""Durable Adapter Protocol v2 attention, lease, and relay ledger."""

from __future__ import annotations

import hashlib
import json
import os
import sqlite3
import time
from pathlib import Path
from typing import Any, Iterable

import orchestrator_queue


TERMINAL_STATUSES = {"failed", "interrupted", "cancelled", "expired"}
CONTEXT_ONLY_PRIORITIES = {"lead_room", "informational"}
DEFAULT_REPLY_REQUIRED = {"direct_user"}
PROTECTED_PRIORITIES = {"security_control", "care_signal", "explicit_task"}

# A pre-acceptance RPC rejection (turn/start, turn/steer, turn/interrupt) used
# to re-drive the same attention at ~1/sec forever. Cap the retries with
# exponential backoff and give up at the ceiling so a wedged adapter/sandbox
# degrades instead of churning.
DEFER_MAX_ATTEMPTS = 8
DEFER_MAX_DELAY_MS = 30_000


class LeaseBusy(RuntimeError):
    """Another non-expired adapter currently owns this identity."""


class StaleLease(RuntimeError):
    """A state mutation was attempted with an obsolete fencing token."""


def wall_time_ms() -> int:
    return int(time.time() * 1000)


def attention_id_for(identity: str, server: str, delivery_id: str) -> str:
    origin = hashlib.sha256(server.encode("utf-8")).hexdigest()[:12]
    return f"attention:{identity}:{origin}:{delivery_id}"


def lease_owner_process_alive(owner: str) -> bool:
    """Return false only when a local PID-bearing owner is provably dead."""
    prefix = owner.split("-", 1)[0]
    try:
        pid = int(prefix)
    except ValueError:
        return True
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def event_messages(event: dict[str, Any]) -> list[dict[str, Any]]:
    messages = event.get("messages") if event.get("t") == "turn" else [event]
    return [message for message in messages if isinstance(message, dict)]


def message_range(event: dict[str, Any]) -> tuple[int | None, int | None]:
    ids = [
        int(message["id"])
        for message in event_messages(event)
        if isinstance(message.get("id"), int)
    ]
    return (min(ids), max(ids)) if ids else (None, None)


class AttentionStore:
    def __init__(self, path: Path):
        self.path = path
        self.path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        os.chmod(self.path.parent, 0o700)
        self._initialize()

    def connect(self) -> sqlite3.Connection:
        connection = sqlite3.connect(self.path, timeout=10)
        connection.row_factory = sqlite3.Row
        connection.execute("PRAGMA foreign_keys = ON")
        connection.execute("PRAGMA busy_timeout = 10000")
        return connection

    def _initialize(self) -> None:
        with self.connect() as connection:
            connection.executescript(
                """
                PRAGMA journal_mode = WAL;

                CREATE TABLE IF NOT EXISTS ingestion_sources (
                    source_id TEXT PRIMARY KEY,
                    path TEXT NOT NULL,
                    offset INTEGER NOT NULL DEFAULT 0,
                    updated_at_ms INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS context_watermarks (
                    server TEXT NOT NULL,
                    room TEXT NOT NULL,
                    last_id INTEGER NOT NULL,
                    observed_at_ms INTEGER NOT NULL,
                    PRIMARY KEY (server, room)
                );

                CREATE TABLE IF NOT EXISTS bundles (
                    bundle_id TEXT PRIMARY KEY,
                    identity TEXT NOT NULL,
                    priority TEXT NOT NULL,
                    created_at_ms INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS attentions (
                    attention_id TEXT PRIMARY KEY,
                    delivery_id TEXT NOT NULL,
                    source_id TEXT NOT NULL,
                    source_offset INTEGER NOT NULL,
                    server TEXT NOT NULL,
                    room TEXT NOT NULL,
                    identity TEXT NOT NULL,
                    priority TEXT NOT NULL,
                    priority_rank INTEGER NOT NULL,
                    reply_required INTEGER NOT NULL,
                    context_from_id INTEGER,
                    context_to_id INTEGER,
                    event_json TEXT NOT NULL,
                    bundle_id TEXT NOT NULL REFERENCES bundles(bundle_id),
                    stored_at_ms INTEGER NOT NULL,
                    accepted_at_ms INTEGER,
                    first_response_at_ms INTEGER,
                    final_response_at_ms INTEGER,
                    turn_completed_at_ms INTEGER,
                    terminal_status TEXT CHECK (
                        terminal_status IS NULL OR terminal_status IN (
                            'failed', 'interrupted', 'cancelled', 'expired'
                        )
                    ),
                    terminal_reason TEXT,
                    turn_id TEXT,
                    lease_epoch INTEGER,
                    attempts INTEGER NOT NULL DEFAULT 0,
                    next_attempt_at_ms INTEGER NOT NULL DEFAULT 0,
                    UNIQUE (server, room, identity, delivery_id)
                );

                CREATE INDEX IF NOT EXISTS attentions_pending
                    ON attentions (
                        identity, terminal_status, accepted_at_ms,
                        next_attempt_at_ms, priority_rank, stored_at_ms
                    );

                CREATE TABLE IF NOT EXISTS bundle_attentions (
                    bundle_id TEXT NOT NULL REFERENCES bundles(bundle_id),
                    attention_id TEXT NOT NULL REFERENCES attentions(attention_id),
                    position INTEGER NOT NULL,
                    PRIMARY KEY (bundle_id, attention_id),
                    UNIQUE (bundle_id, position)
                );

                CREATE TABLE IF NOT EXISTS leases (
                    identity TEXT PRIMARY KEY,
                    lease_owner TEXT NOT NULL,
                    lease_epoch INTEGER NOT NULL,
                    lease_expires_at_ms INTEGER NOT NULL,
                    updated_at_ms INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS threads (
                    server TEXT NOT NULL,
                    room TEXT NOT NULL,
                    identity TEXT NOT NULL,
                    thread_id TEXT NOT NULL,
                    lease_epoch INTEGER NOT NULL,
                    created_at_ms INTEGER NOT NULL,
                    updated_at_ms INTEGER NOT NULL,
                    PRIMARY KEY (server, room, identity),
                    UNIQUE (thread_id)
                );

                CREATE TABLE IF NOT EXISTS turns (
                    turn_id TEXT PRIMARY KEY,
                    identity TEXT NOT NULL,
                    server TEXT NOT NULL,
                    room TEXT NOT NULL,
                    thread_id TEXT NOT NULL,
                    lease_epoch INTEGER NOT NULL,
                    started_at_ms INTEGER NOT NULL,
                    completed_at_ms INTEGER,
                    terminal_status TEXT,
                    terminal_reason TEXT
                );

                CREATE TABLE IF NOT EXISTS turn_attentions (
                    turn_id TEXT NOT NULL REFERENCES turns(turn_id),
                    attention_id TEXT NOT NULL REFERENCES attentions(attention_id),
                    position INTEGER NOT NULL,
                    PRIMARY KEY (turn_id, attention_id),
                    UNIQUE (turn_id, position)
                );

                CREATE TABLE IF NOT EXISTS dispatch_intents (
                    attention_id TEXT PRIMARY KEY REFERENCES attentions(attention_id),
                    identity TEXT NOT NULL,
                    server TEXT NOT NULL,
                    room TEXT NOT NULL,
                    thread_id TEXT NOT NULL,
                    operation TEXT NOT NULL CHECK (operation IN ('start', 'steer')),
                    client_message_id TEXT NOT NULL UNIQUE,
                    status TEXT NOT NULL CHECK (
                        status IN ('prepared', 'accepted', 'reconciled', 'failed')
                    ),
                    turn_id TEXT,
                    prepared_at_ms INTEGER NOT NULL,
                    resolved_at_ms INTEGER,
                    last_error TEXT,
                    lease_epoch INTEGER NOT NULL
                );

                CREATE INDEX IF NOT EXISTS dispatch_intents_unresolved
                    ON dispatch_intents (identity, status, prepared_at_ms);

                CREATE TABLE IF NOT EXISTS relay_items (
                    op_id TEXT PRIMARY KEY,
                    identity TEXT NOT NULL,
                    room TEXT NOT NULL,
                    target TEXT NOT NULL,
                    turn_id TEXT NOT NULL,
                    item_id TEXT NOT NULL,
                    phase TEXT,
                    text TEXT NOT NULL,
                    covered_attention_ids_json TEXT NOT NULL,
                    relay_kind TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'pending',
                    attempts INTEGER NOT NULL DEFAULT 0,
                    last_error TEXT,
                    next_attempt_at_ms INTEGER NOT NULL DEFAULT 0,
                    created_at_ms INTEGER NOT NULL,
                    accepted_at_ms INTEGER,
                    lease_epoch INTEGER NOT NULL,
                    UNIQUE (turn_id, item_id, relay_kind)
                );

                CREATE INDEX IF NOT EXISTS relay_items_pending
                    ON relay_items (identity, status, created_at_ms);
                """
            )
        os.chmod(self.path, 0o600)

    @staticmethod
    def _source_id(inbox: Path, identity: str) -> str:
        return f"{identity}:{inbox.resolve()}"

    @staticmethod
    def _reply_required(record: dict[str, Any], priority: str) -> bool:
        explicit = record.get("reply_required")
        if isinstance(explicit, bool):
            return explicit
        return priority in DEFAULT_REPLY_REQUIRED

    def initialize_ingestion_source(
        self,
        inbox: Path,
        identity: str,
        *,
        replay_existing: bool = False,
        now_ms: int | None = None,
    ) -> int:
        """Create a first-run cursor without replaying a historical inbox.

        Existing cursor state always wins.  Operators can explicitly request
        a replay for conformance/recovery, but normal runtime cutover starts at
        the current complete-file boundary so stale direct calls are not
        turned into fresh model work.
        """
        source_id = self._source_id(inbox, identity)
        now = wall_time_ms() if now_ms is None else now_ms
        with self.connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            row = connection.execute(
                "SELECT offset FROM ingestion_sources WHERE source_id = ?",
                (source_id,),
            ).fetchone()
            if row is not None:
                return int(row["offset"])
            try:
                size = inbox.stat().st_size
            except FileNotFoundError:
                size = 0
            offset = 0 if replay_existing else size
            connection.execute(
                """
                INSERT INTO ingestion_sources (
                    source_id, path, offset, updated_at_ms
                ) VALUES (?, ?, ?, ?)
                """,
                (source_id, str(inbox.resolve()), offset, now),
            )
            return offset

    def ingest_inbox(self, inbox: Path, identity: str) -> dict[str, int]:
        """Materialize complete JSONL records and advance offset atomically."""
        source_id = self._source_id(inbox, identity)
        now = wall_time_ms()
        inserted = context_only = duplicate = 0
        with self.connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            row = connection.execute(
                "SELECT offset FROM ingestion_sources WHERE source_id = ?",
                (source_id,),
            ).fetchone()
            offset = int(row["offset"]) if row else 0
            try:
                size = inbox.stat().st_size
            except FileNotFoundError:
                size = 0
            if size < offset:
                offset = 0
            next_offset = offset
            if size > offset:
                with inbox.open("rb") as handle:
                    handle.seek(offset)
                    while raw := handle.readline():
                        if not raw.endswith(b"\n"):
                            break
                        record_end = handle.tell()
                        record = orchestrator_queue.parse_record(
                            raw, next_offset, record_end
                        )
                        next_offset = record_end
                        record_identity = str(record.get("identity") or identity)
                        if record_identity != identity:
                            raise ValueError(
                                "inbox identity mismatch: "
                                f"expected {identity}, got {record_identity}"
                            )
                        event = record.get("event")
                        if not isinstance(event, dict):
                            raise ValueError("delivery event is not an object")
                        server = str(record.get("server") or "")
                        room = str(record.get("room") or "")
                        first_id, last_id = message_range(event)
                        if room and last_id is not None:
                            connection.execute(
                                """
                                INSERT INTO context_watermarks (
                                    server, room, last_id, observed_at_ms
                                ) VALUES (?, ?, ?, ?)
                                ON CONFLICT(server, room) DO UPDATE SET
                                    last_id = MAX(last_id, excluded.last_id),
                                    observed_at_ms = excluded.observed_at_ms
                                """,
                                (server, room, last_id, now),
                            )

                        priority = str(record.get("priority") or "informational")
                        if priority in CONTEXT_ONLY_PRIORITIES:
                            context_only += 1
                            continue
                        delivery_id = str(record["delivery_id"])
                        attention_id = attention_id_for(
                            identity, server, delivery_id
                        )
                        bundle_id = f"bundle:{attention_id}"
                        connection.execute(
                            """
                            INSERT OR IGNORE INTO bundles (
                                bundle_id, identity, priority, created_at_ms
                            ) VALUES (?, ?, ?, ?)
                            """,
                            (bundle_id, identity, priority, now),
                        )
                        cursor = connection.execute(
                            """
                            INSERT OR IGNORE INTO attentions (
                                attention_id, delivery_id, source_id,
                                source_offset, server,
                                room, identity, priority, priority_rank,
                                reply_required, context_from_id, context_to_id,
                                event_json, bundle_id, stored_at_ms
                            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                            """,
                            (
                                attention_id,
                                delivery_id,
                                source_id,
                                int(record["_queue_offset"]),
                                server,
                                room,
                                identity,
                                priority,
                                orchestrator_queue.record_priority(record),
                                int(self._reply_required(record, priority)),
                                first_id,
                                last_id,
                                json.dumps(event, ensure_ascii=False),
                                bundle_id,
                                int(record.get("received_at_ms") or now),
                            ),
                        )
                        if cursor.rowcount:
                            connection.execute(
                                """
                                INSERT INTO bundle_attentions (
                                    bundle_id, attention_id, position
                                ) VALUES (?, ?, 0)
                                """,
                                (bundle_id, attention_id),
                            )
                            inserted += 1
                        else:
                            duplicate += 1

            connection.execute(
                """
                INSERT INTO ingestion_sources (
                    source_id, path, offset, updated_at_ms
                ) VALUES (?, ?, ?, ?)
                ON CONFLICT(source_id) DO UPDATE SET
                    path = excluded.path,
                    offset = excluded.offset,
                    updated_at_ms = excluded.updated_at_ms
                """,
                (source_id, str(inbox.resolve()), next_offset, now),
            )
        return {
            "inserted": inserted,
            "context_only": context_only,
            "duplicate": duplicate,
            "offset": next_offset,
        }

    def claim_lease(
        self,
        identity: str,
        owner: str,
        ttl_ms: int,
        *,
        now_ms: int | None = None,
    ) -> int:
        now = wall_time_ms() if now_ms is None else now_ms
        with self.connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            row = connection.execute(
                "SELECT * FROM leases WHERE identity = ?", (identity,)
            ).fetchone()
            if row is None:
                epoch = 1
                connection.execute(
                    "INSERT INTO leases VALUES (?, ?, ?, ?, ?)",
                    (identity, owner, epoch, now + ttl_ms, now),
                )
                return epoch
            if row["lease_owner"] == owner:
                epoch = int(row["lease_epoch"])
            elif (
                int(row["lease_expires_at_ms"]) <= now
                or not lease_owner_process_alive(str(row["lease_owner"]))
            ):
                epoch = int(row["lease_epoch"]) + 1
            else:
                raise LeaseBusy(
                    f"identity {identity} owned by {row['lease_owner']} "
                    f"until {row['lease_expires_at_ms']}"
                )
            connection.execute(
                """
                UPDATE leases SET lease_owner = ?, lease_epoch = ?,
                    lease_expires_at_ms = ?, updated_at_ms = ?
                WHERE identity = ?
                """,
                (owner, epoch, now + ttl_ms, now, identity),
            )
            return epoch

    def renew_lease(
        self,
        identity: str,
        owner: str,
        epoch: int,
        ttl_ms: int,
        *,
        now_ms: int | None = None,
    ) -> None:
        now = wall_time_ms() if now_ms is None else now_ms
        with self.connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            self._assert_fence(connection, identity, owner, epoch, now)
            connection.execute(
                """
                UPDATE leases SET lease_expires_at_ms = ?, updated_at_ms = ?
                WHERE identity = ? AND lease_owner = ? AND lease_epoch = ?
                """,
                (now + ttl_ms, now, identity, owner, epoch),
            )

    def release_lease(
        self,
        identity: str,
        owner: str,
        epoch: int,
        *,
        now_ms: int | None = None,
    ) -> None:
        now = wall_time_ms() if now_ms is None else now_ms
        with self.connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            connection.execute(
                """
                UPDATE leases SET lease_expires_at_ms = ?, updated_at_ms = ?
                WHERE identity = ? AND lease_owner = ? AND lease_epoch = ?
                """,
                (now, now, identity, owner, epoch),
            )

    @staticmethod
    def _assert_fence(
        connection: sqlite3.Connection,
        identity: str,
        owner: str,
        epoch: int,
        now_ms: int,
    ) -> None:
        row = connection.execute(
            "SELECT * FROM leases WHERE identity = ?", (identity,)
        ).fetchone()
        if (
            row is None
            or row["lease_owner"] != owner
            or int(row["lease_epoch"]) != epoch
            or int(row["lease_expires_at_ms"]) <= now_ms
        ):
            raise StaleLease(
                f"stale lease for {identity}: owner={owner} epoch={epoch}"
            )

    def set_thread(
        self,
        identity: str,
        server: str,
        room: str,
        thread_id: str,
        owner: str,
        epoch: int,
        *,
        now_ms: int | None = None,
    ) -> None:
        now = wall_time_ms() if now_ms is None else now_ms
        with self.connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            self._assert_fence(connection, identity, owner, epoch, now)
            connection.execute(
                """
                INSERT INTO threads (
                    server, room, identity, thread_id, lease_epoch,
                    created_at_ms, updated_at_ms
                ) VALUES (?, ?, ?, ?, ?, ?, ?)
                ON CONFLICT(server, room, identity) DO UPDATE SET
                    thread_id = excluded.thread_id,
                    lease_epoch = excluded.lease_epoch,
                    updated_at_ms = excluded.updated_at_ms
                """,
                (server, room, identity, thread_id, epoch, now, now),
            )

    def get_thread(self, identity: str, server: str, room: str) -> str:
        with self.connect() as connection:
            row = connection.execute(
                """
                SELECT thread_id FROM threads
                WHERE identity = ? AND server = ? AND room = ?
                """,
                (identity, server, room),
            ).fetchone()
        return str(row["thread_id"]) if row else ""

    def list_threads(self, identity: str) -> list[dict[str, Any]]:
        with self.connect() as connection:
            rows = connection.execute(
                """
                SELECT * FROM threads WHERE identity = ?
                ORDER BY server, room
                """,
                (identity,),
            ).fetchall()
        return [dict(row) for row in rows]

    def assert_lease(
        self,
        identity: str,
        owner: str,
        epoch: int,
        *,
        now_ms: int | None = None,
    ) -> None:
        """Expose the fencing check for side effects performed by an adapter."""
        now = wall_time_ms() if now_ms is None else now_ms
        with self.connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            self._assert_fence(connection, identity, owner, epoch, now)

    def requeue_missing_thread(
        self,
        identity: str,
        thread_id: str,
        reason: str,
        owner: str,
        epoch: int,
        *,
        now_ms: int | None = None,
    ) -> int:
        """Recover accepted work when Codex proves its thread no longer exists.

        A turn that already produced any durable output is not replayed: its
        relay evidence must be reconciled instead.  Only output-free,
        incomplete turns are returned to the pending lane.
        """
        now = wall_time_ms() if now_ms is None else now_ms
        with self.connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            self._assert_fence(connection, identity, owner, epoch, now)
            turns = connection.execute(
                """
                SELECT turn_id FROM turns
                WHERE identity = ? AND thread_id = ? AND completed_at_ms IS NULL
                """,
                (identity, thread_id),
            ).fetchall()
            requeued = 0
            for turn in turns:
                turn_id = str(turn["turn_id"])
                output = connection.execute(
                    "SELECT 1 FROM relay_items WHERE turn_id = ? LIMIT 1",
                    (turn_id,),
                ).fetchone()
                if output is not None:
                    continue
                attention_ids = [
                    str(row["attention_id"])
                    for row in connection.execute(
                        """
                        SELECT attention_id FROM turn_attentions
                        WHERE turn_id = ? ORDER BY position
                        """,
                        (turn_id,),
                    ).fetchall()
                ]
                connection.execute(
                    """
                    UPDATE turns SET completed_at_ms = ?, terminal_status = 'failed',
                        terminal_reason = ? WHERE turn_id = ?
                    """,
                    (now, reason, turn_id),
                )
                connection.execute(
                    "DELETE FROM turn_attentions WHERE turn_id = ?", (turn_id,)
                )
                for attention_id in attention_ids:
                    cursor = connection.execute(
                        """
                        UPDATE attentions SET accepted_at_ms = NULL,
                            turn_id = NULL, lease_epoch = NULL,
                            turn_completed_at_ms = NULL,
                            terminal_status = NULL, terminal_reason = ?,
                            next_attempt_at_ms = ?
                        WHERE attention_id = ? AND final_response_at_ms IS NULL
                        """,
                        (reason, now, attention_id),
                    )
                    requeued += int(cursor.rowcount)
            connection.execute(
                """
                DELETE FROM threads
                WHERE identity = ? AND thread_id = ?
                """,
                (identity, thread_id),
            )
            return requeued

    def requeue_missing_turn(
        self,
        identity: str,
        thread_id: str,
        turn_id: str,
        reason: str,
        owner: str,
        epoch: int,
        *,
        now_ms: int | None = None,
    ) -> int:
        """Replay an accepted turn that Codex no longer knows about.

        ``turn/start`` can be accepted by app-server and still disappear before
        it reaches durable thread history.  In that crash window no completion
        notification can arrive.  Requeue only output-free work; durable output
        is evidence that the turn did run and must be reconciled instead of
        repeated.
        """
        now = wall_time_ms() if now_ms is None else now_ms
        with self.connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            self._assert_fence(connection, identity, owner, epoch, now)
            turn = connection.execute(
                """
                SELECT 1 FROM turns
                WHERE identity = ? AND thread_id = ? AND turn_id = ?
                  AND completed_at_ms IS NULL
                """,
                (identity, thread_id, turn_id),
            ).fetchone()
            if turn is None:
                return 0
            output = connection.execute(
                "SELECT 1 FROM relay_items WHERE turn_id = ? LIMIT 1",
                (turn_id,),
            ).fetchone()
            if output is not None:
                return 0
            attention_ids = [
                str(row["attention_id"])
                for row in connection.execute(
                    """
                    SELECT attention_id FROM turn_attentions
                    WHERE turn_id = ? ORDER BY position
                    """,
                    (turn_id,),
                ).fetchall()
            ]
            connection.execute(
                """
                UPDATE turns SET completed_at_ms = ?, terminal_status = 'failed',
                    terminal_reason = ? WHERE turn_id = ?
                """,
                (now, reason, turn_id),
            )
            connection.execute(
                "DELETE FROM turn_attentions WHERE turn_id = ?", (turn_id,)
            )
            requeued = 0
            for attention_id in attention_ids:
                cursor = connection.execute(
                    """
                    UPDATE attentions SET accepted_at_ms = NULL,
                        turn_id = NULL, lease_epoch = NULL,
                        first_response_at_ms = NULL,
                        turn_completed_at_ms = NULL,
                        terminal_status = NULL, terminal_reason = ?,
                        next_attempt_at_ms = ?
                    WHERE attention_id = ? AND final_response_at_ms IS NULL
                    """,
                    (reason, now, attention_id),
                )
                requeued += int(cursor.rowcount)
            return requeued

    def incomplete_turn_ids(
        self,
        identity: str,
        thread_id: str,
        *,
        older_than_ms: int | None = None,
    ) -> list[str]:
        with self.connect() as connection:
            rows = connection.execute(
                """
                SELECT turn_id FROM turns
                WHERE identity = ? AND thread_id = ? AND completed_at_ms IS NULL
                  AND (? IS NULL OR started_at_ms <= ?)
                ORDER BY started_at_ms, turn_id
                """,
                (identity, thread_id, older_than_ms, older_than_ms),
            ).fetchall()
        return [str(row["turn_id"]) for row in rows]

    def next_pending(
        self, identity: str, *, now_ms: int | None = None
    ) -> dict[str, Any] | None:
        now = wall_time_ms() if now_ms is None else now_ms
        with self.connect() as connection:
            row = connection.execute(
                """
                SELECT * FROM attentions
                WHERE identity = ?
                  AND accepted_at_ms IS NULL
                  AND terminal_status IS NULL
                  AND next_attempt_at_ms <= ?
                ORDER BY priority_rank, stored_at_ms, source_offset, attention_id
                LIMIT 1
                """,
                (identity, now),
            ).fetchone()
        return dict(row) if row else None

    def accepted_for_turn(self, turn_id: str) -> list[dict[str, Any]]:
        with self.connect() as connection:
            rows = connection.execute(
                """
                SELECT a.* FROM turn_attentions ta
                JOIN attentions a ON a.attention_id = ta.attention_id
                WHERE ta.turn_id = ? AND a.terminal_status IS NULL
                ORDER BY ta.position
                """,
                (turn_id,),
            ).fetchall()
        return [dict(row) for row in rows]

    def mark_accepted(
        self,
        attention_id: str,
        thread_id: str,
        turn_id: str,
        owner: str,
        epoch: int,
        *,
        now_ms: int | None = None,
    ) -> None:
        now = wall_time_ms() if now_ms is None else now_ms
        with self.connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            self._mark_accepted_in_transaction(
                connection,
                attention_id,
                thread_id,
                turn_id,
                owner,
                epoch,
                now,
            )

    def _mark_accepted_in_transaction(
        self,
        connection: sqlite3.Connection,
        attention_id: str,
        thread_id: str,
        turn_id: str,
        owner: str,
        epoch: int,
        now: int,
    ) -> None:
        row = connection.execute(
            "SELECT identity FROM attentions WHERE attention_id = ?",
            (attention_id,),
        ).fetchone()
        if row is None:
            raise ValueError(f"unknown attention {attention_id}")
        identity = str(row["identity"])
        attention = connection.execute(
            "SELECT server, room FROM attentions WHERE attention_id = ?",
            (attention_id,),
        ).fetchone()
        self._assert_fence(connection, identity, owner, epoch, now)
        connection.execute(
            """
            INSERT OR IGNORE INTO turns (
                turn_id, identity, server, room, thread_id, lease_epoch,
                started_at_ms
            ) VALUES (?, ?, ?, ?, ?, ?, ?)
            """,
            (
                turn_id,
                identity,
                str(attention["server"]),
                str(attention["room"]),
                thread_id,
                epoch,
                now,
            ),
        )
        position = int(
            connection.execute(
                "SELECT COUNT(*) AS count FROM turn_attentions WHERE turn_id = ?",
                (turn_id,),
            ).fetchone()["count"]
        )
        connection.execute(
            """
            INSERT OR IGNORE INTO turn_attentions (
                turn_id, attention_id, position
            ) VALUES (?, ?, ?)
            """,
            (turn_id, attention_id, position),
        )
        connection.execute(
            """
            UPDATE attentions SET
                accepted_at_ms = COALESCE(accepted_at_ms, ?),
                turn_id = ?, lease_epoch = ?, attempts = attempts + 1
            WHERE attention_id = ?
            """,
            (now, turn_id, epoch, attention_id),
        )

    def prepare_dispatch(
        self,
        attention_id: str,
        thread_id: str,
        operation: str,
        client_message_id: str,
        owner: str,
        epoch: int,
        *,
        now_ms: int | None = None,
    ) -> None:
        """Durably record intent before issuing a non-idempotent Codex RPC."""
        if operation not in {"start", "steer"}:
            raise ValueError(f"invalid dispatch operation {operation}")
        now = wall_time_ms() if now_ms is None else now_ms
        with self.connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            attention = connection.execute(
                "SELECT * FROM attentions WHERE attention_id = ?",
                (attention_id,),
            ).fetchone()
            if attention is None:
                raise ValueError(f"unknown attention {attention_id}")
            identity = str(attention["identity"])
            self._assert_fence(connection, identity, owner, epoch, now)
            connection.execute(
                """
                INSERT INTO dispatch_intents (
                    attention_id, identity, server, room, thread_id,
                    operation, client_message_id, status, prepared_at_ms,
                    lease_epoch
                ) VALUES (?, ?, ?, ?, ?, ?, ?, 'prepared', ?, ?)
                ON CONFLICT(attention_id) DO UPDATE SET
                    thread_id = excluded.thread_id,
                    operation = excluded.operation,
                    client_message_id = excluded.client_message_id,
                    status = 'prepared', turn_id = NULL,
                    prepared_at_ms = excluded.prepared_at_ms,
                    resolved_at_ms = NULL, last_error = NULL,
                    lease_epoch = excluded.lease_epoch
                """,
                (
                    attention_id,
                    identity,
                    str(attention["server"]),
                    str(attention["room"]),
                    thread_id,
                    operation,
                    client_message_id,
                    now,
                    epoch,
                ),
            )

    def finalize_dispatch(
        self,
        attention_id: str,
        thread_id: str,
        turn_id: str,
        owner: str,
        epoch: int,
        *,
        reconciled: bool = False,
        now_ms: int | None = None,
    ) -> None:
        """Atomically bind an accepted Codex RPC to its attention and turn."""
        now = wall_time_ms() if now_ms is None else now_ms
        with self.connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            row = connection.execute(
                "SELECT status FROM dispatch_intents WHERE attention_id = ?",
                (attention_id,),
            ).fetchone()
            if row is None:
                raise ValueError(f"missing dispatch intent for {attention_id}")
            self._mark_accepted_in_transaction(
                connection,
                attention_id,
                thread_id,
                turn_id,
                owner,
                epoch,
                now,
            )
            connection.execute(
                """
                UPDATE dispatch_intents SET status = ?, turn_id = ?,
                    resolved_at_ms = ?, last_error = NULL,
                    lease_epoch = ?
                WHERE attention_id = ?
                """,
                (
                    "reconciled" if reconciled else "accepted",
                    turn_id,
                    now,
                    epoch,
                    attention_id,
                ),
            )

    def unresolved_dispatches(
        self, identity: str, thread_id: str | None = None
    ) -> list[dict[str, Any]]:
        query = (
            "SELECT * FROM dispatch_intents "
            "WHERE identity = ? AND status = 'prepared'"
        )
        params: list[Any] = [identity]
        if thread_id is not None:
            query += " AND thread_id = ?"
            params.append(thread_id)
        query += " ORDER BY prepared_at_ms, attention_id"
        with self.connect() as connection:
            rows = connection.execute(query, params).fetchall()
        return [dict(row) for row in rows]

    def retry_unobserved_dispatch(
        self,
        attention_id: str,
        reason: str,
        owner: str,
        epoch: int,
        *,
        now_ms: int | None = None,
    ) -> None:
        """Resolve a prepared intent absent from authoritative Codex history."""
        now = wall_time_ms() if now_ms is None else now_ms
        with self.connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            row = connection.execute(
                "SELECT identity FROM dispatch_intents WHERE attention_id = ?",
                (attention_id,),
            ).fetchone()
            if row is None:
                return
            self._assert_fence(
                connection, str(row["identity"]), owner, epoch, now
            )
            connection.execute(
                """
                UPDATE dispatch_intents SET status = 'failed',
                    resolved_at_ms = ?, last_error = ?, lease_epoch = ?
                WHERE attention_id = ?
                """,
                (now, reason, epoch, attention_id),
            )
            connection.execute(
                """
                UPDATE attentions SET next_attempt_at_ms = ?,
                    terminal_reason = ?
                WHERE attention_id = ? AND accepted_at_ms IS NULL
                  AND terminal_status IS NULL
                """,
                (now, reason, attention_id),
            )

    def defer_attention(
        self,
        attention_id: str,
        reason: str,
        delay_ms: int,
        owner: str,
        epoch: int,
        *,
        now_ms: int | None = None,
    ) -> None:
        now = wall_time_ms() if now_ms is None else now_ms
        with self.connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            row = connection.execute(
                "SELECT identity, attempts FROM attentions WHERE attention_id = ?",
                (attention_id,),
            ).fetchone()
            if row is None:
                return
            identity = str(row["identity"])
            self._assert_fence(connection, identity, owner, epoch, now)
            if delay_ms <= 0:
                # Deliberate immediate re-dispatch (e.g. a steer recovered from
                # an inactive turn): not churn, so keep the fast path and do
                # not consume the pre-acceptance retry budget.
                connection.execute(
                    """
                    UPDATE attentions SET next_attempt_at_ms = ?,
                        terminal_reason = ? WHERE attention_id = ?
                    """,
                    (now + delay_ms, reason, attention_id),
                )
                return
            attempt = int(row["attempts"]) + 1
            if attempt >= DEFER_MAX_ATTEMPTS:
                # Ceiling reached: stop the infinite retry and mark the
                # attention degraded with guidance rather than churning.
                guidance = (
                    f"{reason}; gave up after {attempt} pre-acceptance "
                    "retries — attention marked degraded (check "
                    "adapter/sandbox health)"
                )
                connection.execute(
                    """
                    UPDATE attentions SET attempts = ?,
                        terminal_status = 'failed', terminal_reason = ?
                    WHERE attention_id = ?
                    """,
                    (attempt, guidance, attention_id),
                )
                return
            backoff = min(DEFER_MAX_DELAY_MS, delay_ms * (2 ** (attempt - 1)))
            connection.execute(
                """
                UPDATE attentions SET attempts = ?, next_attempt_at_ms = ?,
                    terminal_reason = ? WHERE attention_id = ?
                """,
                (attempt, now + backoff, reason, attention_id),
            )

    def cancel_pending_before_control(
        self,
        control_attention_id: str,
        reason: str,
        owner: str,
        epoch: int,
        *,
        now_ms: int | None = None,
    ) -> int:
        """Cancel older disposable chat while preserving protected work."""
        now = wall_time_ms() if now_ms is None else now_ms
        with self.connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            control = connection.execute(
                "SELECT * FROM attentions WHERE attention_id = ?",
                (control_attention_id,),
            ).fetchone()
            if control is None:
                raise ValueError(f"unknown attention {control_attention_id}")
            identity = str(control["identity"])
            self._assert_fence(connection, identity, owner, epoch, now)
            cursor = connection.execute(
                """
                UPDATE attentions SET terminal_status = 'cancelled',
                    terminal_reason = ?
                WHERE identity = ? AND server = ? AND room = ?
                  AND accepted_at_ms IS NULL AND terminal_status IS NULL
                  AND source_offset < ? AND attention_id != ?
                  AND priority NOT IN (
                      'security_control', 'care_signal', 'explicit_task'
                  )
                """,
                (
                    reason,
                    identity,
                    str(control["server"]),
                    str(control["room"]),
                    int(control["source_offset"]),
                    control_attention_id,
                ),
            )
            connection.execute(
                """
                UPDATE attentions SET
                    accepted_at_ms = COALESCE(accepted_at_ms, ?),
                    turn_completed_at_ms = COALESCE(turn_completed_at_ms, ?),
                    terminal_status = 'cancelled', terminal_reason = ?,
                    lease_epoch = ?, attempts = attempts + 1
                WHERE attention_id = ?
                """,
                (now, now, reason, epoch, control_attention_id),
            )
            return int(cursor.rowcount)

    def interrupt_turn_for_control(
        self,
        turn_id: str,
        reason: str,
        owner: str,
        epoch: int,
        *,
        now_ms: int | None = None,
    ) -> int:
        """Interrupt a turn without erasing protected task/care/control work.

        Disposable chat attentions become terminal. Protected attentions that
        have not already produced a final room response are detached from the
        interrupted turn and returned to the durable pending lane.
        """
        now = wall_time_ms() if now_ms is None else now_ms
        with self.connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            turn = connection.execute(
                "SELECT identity FROM turns WHERE turn_id = ?", (turn_id,)
            ).fetchone()
            if turn is None:
                return 0
            identity = str(turn["identity"])
            self._assert_fence(connection, identity, owner, epoch, now)
            connection.execute(
                """
                UPDATE turns SET completed_at_ms = ?,
                    terminal_status = 'interrupted', terminal_reason = ?
                WHERE turn_id = ?
                """,
                (now, reason, turn_id),
            )
            rows = connection.execute(
                """
                SELECT a.attention_id, a.priority, a.final_response_at_ms
                FROM turn_attentions ta
                JOIN attentions a ON a.attention_id = ta.attention_id
                WHERE ta.turn_id = ? ORDER BY ta.position
                """,
                (turn_id,),
            ).fetchall()
            requeued = 0
            for row in rows:
                attention_id = str(row["attention_id"])
                if (
                    str(row["priority"]) in PROTECTED_PRIORITIES
                    and row["final_response_at_ms"] is None
                ):
                    connection.execute(
                        "DELETE FROM turn_attentions WHERE turn_id = ? "
                        "AND attention_id = ?",
                        (turn_id, attention_id),
                    )
                    connection.execute(
                        """
                        UPDATE attentions SET accepted_at_ms = NULL,
                            turn_id = NULL, lease_epoch = NULL,
                            turn_completed_at_ms = NULL,
                            terminal_status = NULL, terminal_reason = ?,
                            next_attempt_at_ms = ?
                        WHERE attention_id = ?
                        """,
                        (reason, now, attention_id),
                    )
                    requeued += 1
                else:
                    connection.execute(
                        """
                        UPDATE attentions SET turn_completed_at_ms = ?,
                            terminal_status = 'interrupted',
                            terminal_reason = ? WHERE attention_id = ?
                        """,
                        (now, reason, attention_id),
                    )
            connection.execute(
                """
                UPDATE relay_items SET status = 'suppressed',
                    last_error = COALESCE(last_error, ?),
                    next_attempt_at_ms = 0
                WHERE turn_id = ? AND status IN ('pending', 'held')
                """,
                (reason, turn_id),
            )
            return requeued

    def close_room_for_control(
        self,
        control_attention_id: str,
        reason: str,
        owner: str,
        epoch: int,
        *,
        now_ms: int | None = None,
    ) -> int:
        """Terminally cancel every unfinished record for a closed room."""
        now = wall_time_ms() if now_ms is None else now_ms
        with self.connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            control = connection.execute(
                "SELECT * FROM attentions WHERE attention_id = ?",
                (control_attention_id,),
            ).fetchone()
            if control is None:
                raise ValueError(f"unknown attention {control_attention_id}")
            identity = str(control["identity"])
            server = str(control["server"])
            room = str(control["room"])
            self._assert_fence(connection, identity, owner, epoch, now)
            connection.execute(
                """
                UPDATE turns SET completed_at_ms = ?,
                    terminal_status = 'cancelled', terminal_reason = ?
                WHERE identity = ? AND server = ? AND room = ?
                  AND completed_at_ms IS NULL
                """,
                (now, reason, identity, server, room),
            )
            cursor = connection.execute(
                """
                UPDATE attentions SET
                    turn_completed_at_ms = COALESCE(turn_completed_at_ms, ?),
                    terminal_status = 'cancelled', terminal_reason = ?
                WHERE identity = ? AND server = ? AND room = ?
                  AND terminal_status IS NULL
                  AND (
                      accepted_at_ms IS NULL
                      OR turn_completed_at_ms IS NULL
                      OR (reply_required = 1 AND final_response_at_ms IS NULL)
                  )
                """,
                (now, reason, identity, server, room),
            )
            connection.execute(
                """
                UPDATE attentions SET
                    accepted_at_ms = COALESCE(accepted_at_ms, ?),
                    lease_epoch = ?, attempts = attempts + 1
                WHERE attention_id = ?
                """,
                (now, epoch, control_attention_id),
            )
            connection.execute(
                """
                UPDATE relay_items SET status = 'suppressed',
                    last_error = COALESCE(last_error, ?),
                    next_attempt_at_ms = 0
                WHERE status IN ('pending', 'held') AND turn_id IN (
                    SELECT turn_id FROM turns
                    WHERE identity = ? AND server = ? AND room = ?
                )
                """,
                (reason, identity, server, room),
            )
            connection.execute(
                """
                DELETE FROM threads
                WHERE identity = ? AND server = ? AND room = ?
                """,
                (identity, server, room),
            )
            return int(cursor.rowcount)

    def create_relay_item(
        self,
        *,
        op_id: str,
        identity: str,
        room: str,
        target: str,
        turn_id: str,
        item_id: str,
        phase: str | None,
        text: str,
        covered_attention_ids: Iterable[str],
        relay_kind: str,
        owner: str,
        epoch: int,
        status: str = "pending",
        now_ms: int | None = None,
    ) -> bool:
        now = wall_time_ms() if now_ms is None else now_ms
        covered = list(dict.fromkeys(covered_attention_ids))
        with self.connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            self._assert_fence(connection, identity, owner, epoch, now)
            cursor = connection.execute(
                """
                INSERT OR IGNORE INTO relay_items (
                    op_id, identity, room, target, turn_id, item_id, phase,
                    text, covered_attention_ids_json, relay_kind,
                    status, created_at_ms, lease_epoch
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                (
                    op_id,
                    identity,
                    room,
                    target,
                    turn_id,
                    item_id,
                    phase,
                    text,
                    json.dumps(covered),
                    relay_kind,
                    status,
                    now,
                    epoch,
                ),
            )
            return bool(cursor.rowcount)

    def pending_relays(
        self, identity: str, *, now_ms: int | None = None
    ) -> list[dict[str, Any]]:
        now = wall_time_ms() if now_ms is None else now_ms
        with self.connect() as connection:
            rows = connection.execute(
                """
                SELECT * FROM relay_items
                WHERE identity = ? AND status = 'pending'
                  AND next_attempt_at_ms <= ?
                ORDER BY created_at_ms, op_id
                """,
                (identity, now),
            ).fetchall()
        return [dict(row) for row in rows]

    def mark_relay_failed(
        self,
        op_id: str,
        error: str,
        owner: str,
        epoch: int,
        retry_delay_ms: int = 1_000,
        *,
        now_ms: int | None = None,
    ) -> None:
        now = wall_time_ms() if now_ms is None else now_ms
        with self.connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            row = connection.execute(
                "SELECT identity FROM relay_items WHERE op_id = ?", (op_id,)
            ).fetchone()
            if row is None:
                return
            self._assert_fence(
                connection, str(row["identity"]), owner, epoch, now
            )
            attempts = int(
                connection.execute(
                    "SELECT attempts FROM relay_items WHERE op_id = ?",
                    (op_id,),
                ).fetchone()["attempts"]
            )
            delay = min(60_000, retry_delay_ms * (2 ** min(attempts, 6)))
            connection.execute(
                """
                UPDATE relay_items SET attempts = attempts + 1,
                    last_error = ?, next_attempt_at_ms = ? WHERE op_id = ?
                """,
                (error, now + delay, op_id),
            )

    def mark_relay_accepted(
        self,
        op_id: str,
        owner: str,
        epoch: int,
        *,
        now_ms: int | None = None,
    ) -> None:
        now = wall_time_ms() if now_ms is None else now_ms
        with self.connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            relay = connection.execute(
                "SELECT * FROM relay_items WHERE op_id = ?", (op_id,)
            ).fetchone()
            if relay is None:
                raise ValueError(f"unknown relay {op_id}")
            identity = str(relay["identity"])
            self._assert_fence(connection, identity, owner, epoch, now)
            connection.execute(
                """
                UPDATE relay_items SET status = 'accepted',
                    attempts = attempts + 1, accepted_at_ms = ?,
                    last_error = NULL, next_attempt_at_ms = 0 WHERE op_id = ?
                """,
                (now, op_id),
            )
            attention_ids = json.loads(relay["covered_attention_ids_json"])
            for attention_id in attention_ids:
                if relay["relay_kind"] == "final":
                    connection.execute(
                        """
                        UPDATE attentions SET
                            first_response_at_ms = COALESCE(first_response_at_ms, ?),
                            final_response_at_ms = COALESCE(final_response_at_ms, ?)
                        WHERE attention_id = ?
                        """,
                        (now, now, str(attention_id)),
                    )
                else:
                    connection.execute(
                        """
                        UPDATE attentions SET
                            first_response_at_ms = COALESCE(first_response_at_ms, ?)
                        WHERE attention_id = ?
                        """,
                        (now, str(attention_id)),
                    )

    def turn_has_any_relay_kind(self, turn_id: str, relay_kind: str) -> bool:
        with self.connect() as connection:
            row = connection.execute(
                """
                SELECT 1 FROM relay_items
                WHERE turn_id = ? AND relay_kind = ? LIMIT 1
                """,
                (turn_id, relay_kind),
            ).fetchone()
        return row is not None

    def mark_turn_no_reply(
        self,
        turn_id: str,
        owner: str,
        epoch: int,
        *,
        now_ms: int | None = None,
    ) -> int:
        """Close attentions when the model explicitly elects not to reply.

        A suppressed ``LOCA_NO_REPLY`` item is a terminal outcome, not a
        response milestone.  Leaving its reply-required attentions open makes
        health remain DEGRADED forever even though no relay is pending.
        """
        now = wall_time_ms() if now_ms is None else now_ms
        with self.connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            turn = connection.execute(
                "SELECT identity FROM turns WHERE turn_id = ?", (turn_id,)
            ).fetchone()
            if turn is None:
                return 0
            identity = str(turn["identity"])
            self._assert_fence(connection, identity, owner, epoch, now)
            cursor = connection.execute(
                """
                UPDATE attentions SET terminal_status = 'cancelled',
                    terminal_reason = 'model explicitly chose no reply'
                WHERE turn_id = ? AND final_response_at_ms IS NULL
                  AND terminal_status IS NULL
                """,
                (turn_id,),
            )
            return int(cursor.rowcount)

    def latest_unknown_item(self, turn_id: str) -> dict[str, Any] | None:
        with self.connect() as connection:
            row = connection.execute(
                """
                SELECT * FROM relay_items
                WHERE turn_id = ? AND relay_kind = 'candidate'
                ORDER BY created_at_ms DESC, op_id DESC LIMIT 1
                """,
                (turn_id,),
            ).fetchone()
        return dict(row) if row else None

    def mark_turn_completed(
        self,
        turn_id: str,
        status: str,
        reason: str,
        owner: str,
        epoch: int,
        *,
        now_ms: int | None = None,
    ) -> None:
        now = wall_time_ms() if now_ms is None else now_ms
        terminal = status if status in TERMINAL_STATUSES else None
        with self.connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            turn = connection.execute(
                "SELECT identity FROM turns WHERE turn_id = ?", (turn_id,)
            ).fetchone()
            if turn is None:
                return
            identity = str(turn["identity"])
            self._assert_fence(connection, identity, owner, epoch, now)
            connection.execute(
                """
                UPDATE turns SET completed_at_ms = ?, terminal_status = ?,
                    terminal_reason = ? WHERE turn_id = ?
                """,
                (now, terminal, reason or None, turn_id),
            )
            connection.execute(
                """
                UPDATE attentions SET turn_completed_at_ms = ?,
                    terminal_status = COALESCE(terminal_status, ?),
                    terminal_reason = CASE
                        WHEN ? IS NULL THEN terminal_reason ELSE ? END
                WHERE turn_id = ?
                """,
                (now, terminal, terminal, reason or None, turn_id),
            )
            if terminal is not None:
                connection.execute(
                    """
                    UPDATE relay_items SET status = 'suppressed',
                        last_error = COALESCE(last_error, ?),
                        next_attempt_at_ms = 0
                    WHERE turn_id = ? AND status IN ('pending', 'held')
                    """,
                    (reason or f"turn {terminal}", turn_id),
                )

    def progress_summary(self, identity: str) -> dict[str, Any]:
        """Return reply-aware progress without conflating it with presence."""
        with self.connect() as connection:
            row = connection.execute(
                """
                SELECT
                    COUNT(*) AS pending_count,
                    MIN(stored_at_ms) AS oldest_stored_at_ms,
                    SUM(CASE WHEN first_response_at_ms IS NULL THEN 1 ELSE 0 END)
                        AS awaiting_first_count
                FROM attentions
                WHERE identity = ? AND reply_required = 1
                  AND final_response_at_ms IS NULL
                  AND terminal_status IS NULL
                """,
                (identity,),
            ).fetchone()
            accepted = connection.execute(
                """
                SELECT COUNT(*) AS count FROM attentions
                WHERE identity = ? AND accepted_at_ms IS NOT NULL
                  AND turn_completed_at_ms IS NULL
                  AND terminal_status IS NULL
                """,
                (identity,),
            ).fetchone()
        return {
            "reply_required_pending": int(row["pending_count"] or 0),
            "reply_awaiting_first": int(row["awaiting_first_count"] or 0),
            "oldest_reply_stored_at_ms": (
                int(row["oldest_stored_at_ms"])
                if row["oldest_stored_at_ms"] is not None
                else None
            ),
            "turns_in_progress": int(accepted["count"] or 0),
        }

    def latest_lifecycle(self, identity: str) -> dict[str, Any] | None:
        """Return the newest durable attention and its independent milestones."""
        with self.connect() as connection:
            row = connection.execute(
                """
                SELECT attention_id, turn_id, stored_at_ms, accepted_at_ms,
                    first_response_at_ms, final_response_at_ms,
                    turn_completed_at_ms, terminal_status
                FROM attentions
                WHERE identity = ?
                -- Producer timestamps can move backwards or be replayed.
                -- SQLite insertion order is the durable ingestion order.
                ORDER BY rowid DESC
                LIMIT 1
                """,
                (identity,),
            ).fetchone()
        return dict(row) if row is not None else None

    def snapshot(self, identity: str) -> dict[str, Any]:
        with self.connect() as connection:
            attention_rows = connection.execute(
                "SELECT * FROM attentions WHERE identity = ? ORDER BY stored_at_ms",
                (identity,),
            ).fetchall()
            lease = connection.execute(
                "SELECT * FROM leases WHERE identity = ?", (identity,)
            ).fetchone()
        return {
            "attentions": [dict(row) for row in attention_rows],
            "lease": dict(lease) if lease else None,
        }
