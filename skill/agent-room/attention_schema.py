"""The attention store's schema and its upgrades: the tables a store opens
with, and the columns and tables later kit versions added. Called once by
``AttentionStore.__init__`` (``AttentionStore._initialize``); ``self`` is
that store. Split from attention_store.py: the function's body is the same,
one indent shallower, and its SQL strings are byte for byte the same.
"""

import os


def initialize(self) -> None:
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
                CREATE INDEX IF NOT EXISTS relay_items_turn_item
                    ON relay_items (turn_id, item_id);

                -- Building notices one Codex thread's model context holds.
                -- Written only with that input's accepted dispatch; the most
                -- recent are released by compaction, and a turn Codex lost
                -- releases its own. Never a server read receipt.
                CREATE TABLE IF NOT EXISTS notification_context (
                    identity TEXT NOT NULL,
                    thread_id TEXT NOT NULL,
                    notification_id INTEGER NOT NULL,
                    turn_id TEXT NOT NULL,
                    accepted_at_ms INTEGER NOT NULL,
                    PRIMARY KEY (identity, thread_id, notification_id)
                );

                -- The oldest notice id a thread's context is still owed.
                CREATE TABLE IF NOT EXISTS notification_threads (
                    identity TEXT NOT NULL,
                    thread_id TEXT NOT NULL,
                    floor_id INTEGER NOT NULL,
                    PRIMARY KEY (identity, thread_id)
                );
                """
        )
    os.chmod(self.path, 0o600)
