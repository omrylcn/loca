//! SQLite persistence for rooms, messages, notes, mode, and settings.
//!
//! The [`Hub`](crate::hub::Hub) keeps everything in memory for speed and
//! *writes through* to this store on every mutation; on boot the hub loads a
//! snapshot back. A single connection behind a `Mutex` is plenty for this
//! app's write volume, and it keeps ordering trivially correct.
//!
//! When no DB path is configured the store runs in "memory-only" mode: every
//! method is a no-op and `load()` returns empty, so the server behaves exactly
//! like the pre-persistence version (used by tests that want a clean slate).

mod attention;
mod content;
mod identity;
mod messages;
mod operators;
mod rooms;
mod work;

use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

use protocol::{
    Attention, AttentionStatus, ChatMode, Goal, GoalCompletion, GoalStatus, Invite, Message,
    MessageReaction, Note, RoomSettings, SenderType, Task, TaskStatus, WaitState,
};

use crate::sync::RecoverMutex;

pub struct Store {
    conn: Option<Mutex<Connection>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildingRole {
    Master,
    Smaster,
    Member,
}

/// Credential-free public view of a room's explicit Loca Operator assignment.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LocaOperatorAssignment {
    pub room: String,
    pub principal_id: String,
    pub display_name: String,
    pub kind: SenderType,
    pub appointed_by_principal_id: String,
    pub appointed_by_role: BuildingRole,
    pub appointed_at: u64,
    pub revoked_at: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocaOperatorError {
    PrincipalNotFound,
    PrincipalMustBeHuman,
    AppointerNotFound,
    AppointerNotAuthorized,
    EmptySeatRequired,
    NotFound,
    Conflict,
    Storage,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PrincipalIdentity {
    pub id: String,
    pub display_name: String,
    pub kind: SenderType,
    pub role: BuildingRole,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CredentialSummary {
    pub id: String,
    pub label: String,
    pub created_at: u64,
    pub last_used_at: Option<u64>,
    pub revoked_at: Option<u64>,
    pub root_recovery: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialError {
    NotFound,
    RootRecovery,
    Storage,
}

/// Everything needed to rebuild the in-memory hub for one room.
pub struct RoomSnapshot {
    pub room: String,
    pub messages: Vec<Message>,
    pub notes: Vec<Note>,
    pub mode: ChatMode,
    pub settings: RoomSettings,
    /// Highest message id seen, so the hub's id counter resumes past it.
    pub max_msg_id: u64,
    /// Highest note rev seen, so per-room rev counters resume past it.
    pub max_rev: u64,
}

impl Store {
    /// Open (and migrate) the DB at `path`. `path == None` → memory-only no-op.
    pub fn open(path: Option<&str>) -> rusqlite::Result<Self> {
        let Some(path) = path else {
            return Ok(Store { conn: None });
        };
        let mut conn = Connection::open(path)?;
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;
            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY,
                room TEXT NOT NULL,
                sender TEXT NOT NULL,
                sender_type TEXT NOT NULL,
                target TEXT,
                text TEXT NOT NULL,
                reply_to INTEGER,
                ts INTEGER NOT NULL,
                kind TEXT NOT NULL DEFAULT 'say'
            );
            CREATE INDEX IF NOT EXISTS idx_messages_room ON messages(room, id);
            CREATE TABLE IF NOT EXISTS message_reactions (
                room TEXT NOT NULL,
                message_id INTEGER NOT NULL,
                principal TEXT NOT NULL,
                reactor TEXT NOT NULL,
                emoji TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                PRIMARY KEY (room, message_id, principal, emoji),
                FOREIGN KEY (message_id) REFERENCES messages(id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS message_reactions_room
                ON message_reactions(room, message_id);
            CREATE TABLE IF NOT EXISTS notes (
                room TEXT NOT NULL,
                key TEXT NOT NULL,
                title TEXT NOT NULL,
                body TEXT NOT NULL,
                can_write TEXT NOT NULL,   -- JSON array
                updated_by TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                rev INTEGER NOT NULL,
                PRIMARY KEY (room, key)
            );
            CREATE TABLE IF NOT EXISTS note_revisions (
                room TEXT NOT NULL,
                key TEXT NOT NULL,
                rev INTEGER NOT NULL,
                title TEXT NOT NULL,
                body TEXT NOT NULL,
                updated_by TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (room, key, rev)
            );
            CREATE TABLE IF NOT EXISTS tasks (
                id INTEGER NOT NULL,
                room TEXT NOT NULL,
                title TEXT NOT NULL,
                created_by TEXT NOT NULL,
                from_message INTEGER,
                assigned_to TEXT,
                status TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                progress_at INTEGER NOT NULL,
                closed_at INTEGER,
                PRIMARY KEY (room, id)
            );
            CREATE TABLE IF NOT EXISTS goals (
                id INTEGER NOT NULL,
                room TEXT NOT NULL,
                outcome TEXT NOT NULL,
                checkpoint TEXT,
                stale_after_secs INTEGER,
                created_by TEXT NOT NULL,
                completion TEXT NOT NULL,
                task_ids TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                progress_at INTEGER NOT NULL,
                closed_at INTEGER,
                PRIMARY KEY (room, id)
            );
            CREATE UNIQUE INDEX IF NOT EXISTS goals_one_active
                ON goals(room) WHERE status = 'active';
            CREATE TABLE IF NOT EXISTS waits (
                room TEXT NOT NULL,
                waiter TEXT NOT NULL,
                waiting_for TEXT NOT NULL,
                reason TEXT NOT NULL,
                since INTEGER NOT NULL,
                last_signal_at INTEGER,
                signal_count INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (room, waiter)
            );
            CREATE TABLE IF NOT EXISTS care_marks (
                room TEXT NOT NULL,
                signal_key TEXT NOT NULL,
                last_signal_at INTEGER NOT NULL,
                signal_count INTEGER NOT NULL,
                PRIMARY KEY (room, signal_key)
            );
            CREATE TABLE IF NOT EXISTS care_outbox (
                id TEXT PRIMARY KEY,
                attention_id TEXT NOT NULL DEFAULT '',
                delivery_room TEXT NOT NULL,
                owner TEXT NOT NULL,
                signal TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                acked_at INTEGER
            );
            CREATE INDEX IF NOT EXISTS care_outbox_delivery
                ON care_outbox(delivery_room, owner, acked_at);
            CREATE TABLE IF NOT EXISTS attentions (
                id TEXT PRIMARY KEY,
                room TEXT NOT NULL,
                signal TEXT NOT NULL,
                owner TEXT,
                created_at INTEGER NOT NULL,
                delivered_at INTEGER,
                claimed_by TEXT,
                claimed_at INTEGER,
                resolved_at INTEGER
            );
            CREATE INDEX IF NOT EXISTS attentions_room
                ON attentions(room, created_at, id);
            CREATE TABLE IF NOT EXISTS rooms (
                room TEXT PRIMARY KEY,
                mode TEXT NOT NULL,        -- JSON ChatMode
                settings TEXT NOT NULL     -- JSON RoomSettings
            );
            -- Davetler. Bir davet = bir locaya giriş hakkı; master üretir,
            -- master iptal eder. Kalıcı, çünkü restart'ta kimsenin daveti
            -- düşmemeli (session'lar bellekte, davetler diskte).
            CREATE TABLE IF NOT EXISTS invites (
                token TEXT PRIMARY KEY,
                room TEXT NOT NULL,        -- hangi loca; başka locayı açmaz
                name TEXT NOT NULL,        -- kime verildi
                kind TEXT NOT NULL,        -- 'agent' | 'user'
                issued_at INTEGER NOT NULL,
                issued_by TEXT NOT NULL,   -- 'master'
                revoked_at INTEGER         -- NULL = geçerli
            );
            CREATE INDEX IF NOT EXISTS invites_room ON invites(room);
            -- The journal: what was already done, in the doer's words. Written
            -- once and never updated -- there is no UPDATE or DELETE against
            -- this table anywhere in the code, which is what makes it a record
            -- rather than a status board.
            CREATE TABLE IF NOT EXISTS journal (
                id INTEGER NOT NULL,
                room TEXT NOT NULL,
                by TEXT NOT NULL,
                by_type TEXT NOT NULL,
                text TEXT NOT NULL,
                at INTEGER NOT NULL,
                PRIMARY KEY (room, id)
            );
            CREATE INDEX IF NOT EXISTS journal_room ON journal(room, id);
            -- Smasters: second masters. They do everything a master does --
            -- issue davets, revoke them, run any loca -- but the master has the
            -- last word, so a smaster cannot undo what the master decided.
            -- Only the master mints one, and only the master takes it back.
            -- Membership: who belongs to the building. Distinct from invites
            -- on purpose -- a davet seats a member in one loca and comes and
            -- goes, while membership is the identity itself and outlives every
            -- room. Leaving a loca must never cost you the building.
            CREATE TABLE IF NOT EXISTS members (
                token TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                kind TEXT NOT NULL,
                joined_at INTEGER NOT NULL,
                admitted_by TEXT NOT NULL,
                revoked_at INTEGER
            );
            CREATE TABLE IF NOT EXISTS smasters (
                token TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                issued_at INTEGER NOT NULL,
                revoked_at INTEGER
            );
            -- Bans and mutes: per-loca door state that must survive a restart.
            -- Without this a restart empties the sets while davets reload from
            -- disk, so a banned name walks right back in. `kind` is 'ban' or
            -- 'mute'. Keyed by (room, name, kind); clearing = DELETE the row.
            CREATE TABLE IF NOT EXISTS bans (
                room TEXT NOT NULL,
                name TEXT NOT NULL,
                kind TEXT NOT NULL,        -- 'ban' | 'mute'
                at INTEGER NOT NULL,
                PRIMARY KEY (room, name, kind)
            );
            -- Browser admin sessions are short-lived bearer credentials, but
            -- a server deploy must not turn the master into a stranger. Only
            -- admin sessions are persisted; davet sessions remain ephemeral.
            CREATE TABLE IF NOT EXISTS admin_sessions (
                token TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                kind TEXT NOT NULL,
                expires_at INTEGER NOT NULL
            );
            -- Identity v2 separates who someone is from how they authenticate.
            -- Roles belong to principals; credentials merely prove a principal.
            CREATE TABLE IF NOT EXISTS principals (
                id TEXT PRIMARY KEY,
                display_name TEXT NOT NULL,
                kind TEXT NOT NULL CHECK (kind IN ('human', 'agent')),
                building_role TEXT NOT NULL CHECK (building_role IN ('master', 'smaster', 'member')),
                created_at INTEGER NOT NULL,
                disabled_at INTEGER
            );
            CREATE UNIQUE INDEX IF NOT EXISTS principals_one_master
                ON principals(building_role) WHERE building_role = 'master' AND disabled_at IS NULL;
            CREATE TABLE IF NOT EXISTS credentials (
                id TEXT PRIMARY KEY,
                principal_id TEXT NOT NULL REFERENCES principals(id),
                label TEXT NOT NULL,
                secret_hash TEXT NOT NULL UNIQUE,
                created_at INTEGER NOT NULL,
                last_used_at INTEGER,
                revoked_at INTEGER,
                legacy_source TEXT UNIQUE
            );
            CREATE INDEX IF NOT EXISTS credentials_principal
                ON credentials(principal_id, revoked_at);
            CREATE UNIQUE INDEX IF NOT EXISTS credentials_owner
                ON credentials(id, principal_id);
            CREATE TABLE IF NOT EXISTS principal_sessions (
                id TEXT PRIMARY KEY,
                principal_id TEXT NOT NULL REFERENCES principals(id),
                credential_id TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                revoked_at INTEGER,
                FOREIGN KEY (credential_id, principal_id)
                    REFERENCES credentials(id, principal_id)
            );
            CREATE INDEX IF NOT EXISTS principal_sessions_principal
                ON principal_sessions(principal_id, revoked_at, expires_at);
            -- A Loca Operator is an explicit, principal-bound room authority.
            -- Rows are never overwritten: replacement/revocation closes the
            -- prior row, preserving who appointed whom and when.
            CREATE TABLE IF NOT EXISTS room_operator_assignments (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                room TEXT NOT NULL,
                principal_id TEXT NOT NULL REFERENCES principals(id),
                appointed_by_principal_id TEXT NOT NULL REFERENCES principals(id),
                appointed_by_role TEXT NOT NULL
                    CHECK (appointed_by_role IN ('master', 'smaster', 'member')),
                appointed_at INTEGER NOT NULL,
                revoked_at INTEGER
            );
            CREATE UNIQUE INDEX IF NOT EXISTS room_operator_one_active
                ON room_operator_assignments(room) WHERE revoked_at IS NULL;
            CREATE INDEX IF NOT EXISTS room_operator_history
                ON room_operator_assignments(room, appointed_at, id);
            -- Admission stock: a Master pre-mints N single-use, time-limited
            -- Lobby-admission rights. loca-care distributes them; the join-request
            -- approve step consumes exactly one per admitted agent. Each row is one
            -- right; consumed_at / consumed_by_name mark its single, final use.
            CREATE TABLE IF NOT EXISTS admission_stock (
                id TEXT PRIMARY KEY,
                minted_by TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                consumed_at INTEGER,
                consumed_by_name TEXT
            );
            CREATE INDEX IF NOT EXISTS admission_stock_available
                ON admission_stock(consumed_at, expires_at);
            -- Join requests: an outside agent asks to join the Building and picks
            -- its own name. The request grants NOTHING until a Master/Smaster
            -- approves it, which consumes one admission-stock right and issues a
            -- Lobby membership (mb_). The mb_ is delivered exactly once via the
            -- bootstrap endpoint, never in the pollable status. `secret_hash` is
            -- the hash of the caller's request-secret (the plaintext never lands
            -- here), so only the requester can poll or bootstrap their request.
            CREATE TABLE IF NOT EXISTS join_requests (
                id TEXT PRIMARY KEY,
                secret_hash TEXT NOT NULL,
                name TEXT NOT NULL,
                kind TEXT NOT NULL,
                status TEXT NOT NULL
                    CHECK (status IN ('pending', 'approving', 'approved', 'denied')),
                created_at INTEGER NOT NULL,
                decided_at INTEGER,
                decided_by TEXT,
                mb_token TEXT,
                bootstrap_delivered_at INTEGER
            );
            CREATE INDEX IF NOT EXISTS join_requests_pending
                ON join_requests(status, created_at);
            "#,
        )?;
        // Older databases predate message kinds; adding the column is a no-op
        // once it exists, and existing rows are plain speech by definition.
        let _ = conn.execute(
            "ALTER TABLE messages ADD COLUMN kind TEXT NOT NULL DEFAULT 'say'",
            [],
        );
        let _ = conn.execute("ALTER TABLE messages ADD COLUMN principal TEXT", []);
        let _ = conn.execute("ALTER TABLE messages ADD COLUMN op_id TEXT", []);
        conn.execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_messages_operation
             ON messages(room, principal, op_id)
             WHERE principal IS NOT NULL AND op_id IS NOT NULL",
            [],
        )?;
        // A davet now seats a MEMBER (mb_ token) — identity comes from the
        // membership record, the invite's name/kind are an audit snapshot.
        // Legacy rows get an empty member and are bound at load (migration).
        let _ = conn.execute(
            "ALTER TABLE invites ADD COLUMN member TEXT NOT NULL DEFAULT ''",
            [],
        );
        // A deleted loca is SEALED, not destroyed (PRINCIPLES: "seal not
        // destroy" — the record survives so "what happened here" stays
        // answerable). `sealed_at` marks it; the row and its history stay on
        // disk, and boot re-tombstones it so it never silently reopens.
        let _ = conn.execute("ALTER TABLE rooms ADD COLUMN sealed_at INTEGER", []);
        // Goal/task care is based on explicit progress, never on unrelated
        // chat. Existing rows start at creation time so upgrades do not
        // manufacture artificial recent progress.
        let _ = conn.execute(
            "ALTER TABLE tasks ADD COLUMN progress_at INTEGER NOT NULL DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "UPDATE tasks SET progress_at = created_at WHERE progress_at = 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE goals ADD COLUMN progress_at INTEGER NOT NULL DEFAULT 0",
            [],
        );
        let _ = conn.execute("ALTER TABLE goals ADD COLUMN checkpoint TEXT", []);
        let _ = conn.execute("ALTER TABLE goals ADD COLUMN stale_after_secs INTEGER", []);
        let _ = conn.execute(
            "UPDATE goals SET progress_at = created_at WHERE progress_at = 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE care_outbox ADD COLUMN attention_id TEXT NOT NULL DEFAULT ''",
            [],
        );
        let _ = conn.execute(
            "UPDATE care_outbox SET attention_id = id WHERE attention_id = ''",
            [],
        );
        // v0.6.9 makes Attention a first-class product ledger. Older DBs may
        // already contain durable Care deliveries; promote each one instead
        // of replaying an event that cannot be claimed or resolved.
        let tx = conn.transaction()?;
        let legacy_rows = {
            let mut stmt = tx.prepare(
                "SELECT id, attention_id, signal, owner, created_at, acked_at FROM care_outbox",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, u64>(4)?,
                    row.get::<_, Option<u64>>(5)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (delivery_id, attention_id, old_json, owner, created_at, acked_at) in legacy_rows {
            let mut signal: protocol::CareSignal =
                serde_json::from_str(&old_json).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
            if signal.attention_id.is_empty() {
                signal.attention_id = attention_id.clone();
            }
            let signal_json = serde_json::to_string(&signal)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
            tx.execute(
                "UPDATE care_outbox SET signal = ?2 WHERE id = ?1",
                params![delivery_id, signal_json],
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO attentions
                 (id, room, signal, owner, created_at, delivered_at, claimed_by, claimed_at, resolved_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL, NULL)",
                params![
                    attention_id,
                    signal.room,
                    signal_json,
                    owner,
                    created_at,
                    acked_at
                ],
            )?;
        }
        tx.commit()?;
        migrate_legacy_principals(&mut conn)?;
        Ok(Store {
            conn: Some(Mutex::new(conn)),
        })
    }

    /// Insert one admission-stock row per pre-generated right id (a Master
    /// pre-minting a batch of single-use Lobby-admission rights). All-or-nothing
    /// in one transaction, and returns the number of rows actually inserted so
    /// the caller reports a truthful `minted` count (never the merely-requested
    /// one) even if the batch is rolled back.
    pub fn mint_admission_rights(
        &self,
        ids: &[String],
        minted_by: &str,
        created_at: u64,
        expires_at: u64,
    ) -> rusqlite::Result<usize> {
        let Some(mut c) = self.conn() else { return Ok(0) };
        let tx = c.transaction()?;
        let mut inserted = 0usize;
        for id in ids {
            inserted += tx.execute(
                "INSERT INTO admission_stock (id, minted_by, created_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![id, minted_by, created_at, expires_at],
            )?;
        }
        tx.commit()?;
        Ok(inserted)
    }

    /// `(total_ever, available_now)` where available = unconsumed AND unexpired.
    pub fn admission_stock_counts(&self, now: u64) -> (u64, u64) {
        let Some(c) = self.conn() else { return (0, 0) };
        let total: u64 = c
            .query_row("SELECT count(*) FROM admission_stock", [], |r| r.get(0))
            .unwrap_or(0);
        let available: u64 = c
            .query_row(
                "SELECT count(*) FROM admission_stock \
                 WHERE consumed_at IS NULL AND expires_at > ?1",
                params![now],
                |r| r.get(0),
            )
            .unwrap_or(0);
        (total, available)
    }

    /// Atomically claim exactly one available right (oldest first), marking it
    /// consumed by `name`. Returns its id, or None if the stock is empty/expired.
    /// The single `Mutex<Connection>` serialises access, so the SELECT + UPDATE
    /// pair cannot interleave with another consume; the `consumed_at IS NULL`
    /// guard on the write keeps it safe even so.
    pub fn consume_admission_right(&self, name: &str, now: u64) -> Option<String> {
        let c = self.conn()?;
        let id: String = c
            .query_row(
                "SELECT id FROM admission_stock \
                 WHERE consumed_at IS NULL AND expires_at > ?1 \
                 ORDER BY created_at, id LIMIT 1",
                params![now],
                |r| r.get(0),
            )
            .ok()?;
        let changed = c
            .execute(
                "UPDATE admission_stock SET consumed_at = ?1, consumed_by_name = ?2 \
                 WHERE id = ?3 AND consumed_at IS NULL",
                params![now, name, id],
            )
            .ok()?;
        (changed == 1).then_some(id)
    }

    /// Record a new pending join request. The caller's request-secret is stored
    /// only as a hash, so only the requester (who holds the plaintext) can later
    /// poll or bootstrap it.
    pub fn create_join_request(
        &self,
        id: &str,
        secret: &str,
        name: &str,
        kind: &str,
        created_at: u64,
    ) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else { return Ok(()) };
        c.execute(
            "INSERT INTO join_requests (id, secret_hash, name, kind, status, created_at) \
             VALUES (?1, ?2, ?3, ?4, 'pending', ?5)",
            params![id, secret_hash(secret), name, kind, created_at],
        )
        .map(|_| ())
    }

    /// `(status, name, bootstrap_ready)` if `id` + `secret` match; otherwise None
    /// (a caller learns nothing about a request that is not theirs).
    pub fn join_request_view(&self, id: &str, secret: &str) -> Option<(String, String, bool)> {
        let c = self.conn()?;
        c.query_row(
            "SELECT status, name, \
                    (mb_token IS NOT NULL AND bootstrap_delivered_at IS NULL) \
             FROM join_requests WHERE id = ?1 AND secret_hash = ?2",
            params![id, secret_hash(secret)],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, bool>(2)?,
                ))
            },
        )
        .ok()
    }

    /// Pending requests for the Master review list (id, name, kind, created_at);
    /// never any secret or token.
    pub fn list_pending_join_requests(&self) -> Vec<(String, String, String, u64)> {
        let Some(c) = self.conn() else { return Vec::new() };
        let Ok(mut stmt) = c.prepare(
            "SELECT id, name, kind, created_at FROM join_requests \
             WHERE status = 'pending' ORDER BY created_at, id",
        ) else {
            return Vec::new();
        };
        let Ok(rows) = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, u64>(3)?,
            ))
        }) else {
            return Vec::new();
        };
        rows.flatten().collect()
    }

    /// Atomically claim a pending request for approval (pending -> approving),
    /// returning its `name`. Only the FIRST caller wins — the `WHERE
    /// status='pending'` gate means a repeated/racing approve gets None, so
    /// admission stock is consumed at most once per request.
    pub fn claim_join_request_for_approval(&self, id: &str) -> Option<String> {
        let c = self.conn()?;
        let changed = c
            .execute(
                "UPDATE join_requests SET status = 'approving' \
                 WHERE id = ?1 AND status = 'pending'",
                params![id],
            )
            .ok()?;
        if changed != 1 {
            return None;
        }
        c.query_row(
            "SELECT name FROM join_requests WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .ok()
    }

    /// Finalise a claimed approval: attach the issued mb_ and mark approved.
    pub fn finalize_join_request_approval(
        &self,
        id: &str,
        mb_token: &str,
        decided_by: &str,
        decided_at: u64,
    ) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else { return Ok(()) };
        c.execute(
            "UPDATE join_requests \
             SET status = 'approved', mb_token = ?2, decided_by = ?3, decided_at = ?4 \
             WHERE id = ?1 AND status = 'approving'",
            params![id, mb_token, decided_by, decided_at],
        )
        .map(|_| ())
    }

    /// Release a claimed request back to pending (e.g. no stock was available),
    /// so the Master can retry after replenishing.
    pub fn release_join_request_claim(&self, id: &str) {
        if let Some(c) = self.conn() {
            let _ = c.execute(
                "UPDATE join_requests SET status = 'pending' \
                 WHERE id = ?1 AND status = 'approving'",
                params![id],
            );
        }
    }

    /// Deny a pending request. Returns true iff it was pending (now denied).
    pub fn deny_join_request(&self, id: &str, decided_by: &str, decided_at: u64) -> bool {
        let Some(c) = self.conn() else { return false };
        c.execute(
            "UPDATE join_requests SET status = 'denied', decided_by = ?2, decided_at = ?3 \
             WHERE id = ?1 AND status = 'pending'",
            params![id, decided_by, decided_at],
        )
        .map(|n| n == 1)
        .unwrap_or(false)
    }

    /// Deliver the issued mb_ EXACTLY ONCE: only if approved, not-yet-delivered,
    /// and the secret matches. Marks it delivered so a second call returns None —
    /// the credential never sits in a repeatable/pollable response.
    pub fn claim_join_request_bootstrap(&self, id: &str, secret: &str, now: u64) -> Option<String> {
        let c = self.conn()?;
        let mb: String = c
            .query_row(
                "SELECT mb_token FROM join_requests \
                 WHERE id = ?1 AND secret_hash = ?2 AND status = 'approved' \
                   AND mb_token IS NOT NULL AND bootstrap_delivered_at IS NULL",
                params![id, secret_hash(secret)],
                |r| r.get(0),
            )
            .ok()?;
        let changed = c
            .execute(
                "UPDATE join_requests SET bootstrap_delivered_at = ?2 \
                 WHERE id = ?1 AND bootstrap_delivered_at IS NULL",
                params![id, now],
            )
            .ok()?;
        (changed == 1).then_some(mb)
    }

    pub fn is_persistent(&self) -> bool {
        self.conn.is_some()
    }

    fn conn(&self) -> Option<std::sync::MutexGuard<'_, Connection>> {
        self.conn.as_ref().map(|m| m.lock_or_recover())
    }
}

pub(crate) fn hashed_id(prefix: &str, value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    format!("{prefix}{}", hex_bytes(&digest[..16]))
}

fn secret_hash(secret: &str) -> String {
    hex_bytes(&Sha256::digest(secret.as_bytes()))
}

fn hex_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

/// Backfill legacy token-shaped identities without copying their raw secrets
/// into the new model. Deterministic hashed ids make the migration idempotent.
fn migrate_legacy_principals(conn: &mut Connection) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    let members = {
        let mut stmt =
            tx.prepare("SELECT token, name, kind, joined_at, revoked_at FROM members")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, u64>(3)?,
                row.get::<_, Option<u64>>(4)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (token, name, kind, joined_at, revoked_at) in members {
        let principal_id = hashed_id("pr_", &format!("member:{token}"));
        let credential_id = hashed_id("cr_", &format!("member:{token}"));
        let principal_kind = if kind == "agent" { "agent" } else { "human" };
        tx.execute(
            "INSERT OR IGNORE INTO principals
             (id, display_name, kind, building_role, created_at, disabled_at)
             VALUES (?1, ?2, ?3, 'member', ?4, ?5)",
            params![principal_id, name, principal_kind, joined_at, revoked_at],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO credentials
             (id, principal_id, label, secret_hash, created_at, last_used_at, revoked_at, legacy_source)
             VALUES (?1, ?2, 'Legacy member access', ?3, ?4, NULL, ?5, ?6)",
            params![
                credential_id,
                principal_id,
                secret_hash(&token),
                joined_at,
                revoked_at,
                format!("member:{}", secret_hash(&token))
            ],
        )?;
    }
    let smasters = {
        let mut stmt = tx.prepare("SELECT token, name, issued_at, revoked_at FROM smasters")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u64>(2)?,
                row.get::<_, Option<u64>>(3)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (token, name, issued_at, revoked_at) in smasters {
        let principal_id = hashed_id("pr_", &format!("smaster:{token}"));
        let credential_id = hashed_id("cr_", &format!("smaster:{token}"));
        tx.execute(
            "INSERT OR IGNORE INTO principals
             (id, display_name, kind, building_role, created_at, disabled_at)
             VALUES (?1, ?2, 'human', 'smaster', ?3, ?4)",
            params![principal_id, name, issued_at, revoked_at],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO credentials
             (id, principal_id, label, secret_hash, created_at, last_used_at, revoked_at, legacy_source)
             VALUES (?1, ?2, 'Legacy Smaster access', ?3, ?4, NULL, ?5, ?6)",
            params![
                credential_id,
                principal_id,
                secret_hash(&token),
                issued_at,
                revoked_at,
                format!("smaster:{}", secret_hash(&token))
            ],
        )?;
    }
    tx.commit()
}

fn sender_type_str(t: SenderType) -> &'static str {
    match t {
        SenderType::Agent => "agent",
        SenderType::User => "user",
    }
}
fn kind_str(k: protocol::MessageKind) -> &'static str {
    match k {
        protocol::MessageKind::Announce => "announce",
        protocol::MessageKind::Say => "say",
    }
}

fn parse_kind(s: &str) -> protocol::MessageKind {
    match s {
        "announce" => protocol::MessageKind::Announce,
        _ => protocol::MessageKind::Say,
    }
}

fn parse_sender_type(s: &str) -> SenderType {
    match s {
        "agent" => SenderType::Agent,
        _ => SenderType::User,
    }
}

fn current_time_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
