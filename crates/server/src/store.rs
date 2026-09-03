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

// Room attachments (docs/rfc-room-attachments.md). `attachments` is the
// content-addressed blob store (physical files); `attachment_index` is the
// SQLite lifecycle over it (quotas, pending→referenced, refcount, sweep).
mod attachment_index;
mod attachments;
mod attention;
pub(crate) use attachment_index::{AttachError, BlobServe};
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
    /// Content-addressed file store for attachment blobs. `Some` only in
    /// persistent mode (it lives beside the SQLite file, under the same data
    /// volume). Memory-only stores have no durable blob dir, so attachments
    /// are disabled there — the endpoints answer 503, matching how every other
    /// persistence method no-ops without a DB.
    blobs: Option<attachments::BlobStore>,
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
            return Ok(Store {
                conn: None,
                blobs: None,
            });
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
                owner_principal_id TEXT,
                signal TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                acked_at INTEGER
            );
            CREATE INDEX IF NOT EXISTS care_outbox_delivery
                ON care_outbox(delivery_room, owner, acked_at);
            -- The owner_principal_id index is created in the migration step, AFTER
            -- the ALTER that adds the column, so an older DB (whose CREATE TABLE
            -- IF NOT EXISTS is a no-op) does not reference a column it lacks yet.
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
            -- Room attachments (docs/rfc-room-attachments.md). The physical
            -- bytes live in the content-addressed BlobStore beside this DB;
            -- these tables are the metadata + reference index.
            --
            -- attachment_blobs: one row per unique sha that has a physical
            -- file on disk (pending OR referenced). It anchors the serve mime
            -- and the building-wide physical footprint. A blob's file is
            -- deleted, and its row removed, only when it has neither a pending
            -- nor a referenced row (see reference-count note on attachment_refs).
            CREATE TABLE IF NOT EXISTS attachment_blobs (
                sha TEXT PRIMARY KEY,
                mime TEXT NOT NULL,
                size INTEGER NOT NULL,
                created_at INTEGER NOT NULL
            );
            -- attachment_pending: a fresh upload not yet cited by any accepted
            -- message. TTL-swept so an upload that never sends leaves no orphan.
            -- Keyed by (sha, room): the same bytes uploaded in two rooms each
            -- hold their own pending slot, so one room's send can't consume the
            -- other's pending upload.
            CREATE TABLE IF NOT EXISTS attachment_pending (
                sha TEXT NOT NULL,
                room TEXT NOT NULL,
                uploader TEXT NOT NULL,
                name TEXT NOT NULL,
                mime TEXT NOT NULL,
                size INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                PRIMARY KEY (sha, room)
            );
            -- attachment_refs: the logical, per-room reference index. One row
            -- per (room, message, sha). This single table gives BOTH refcount
            -- levels loca-dev requires with no stored counter to drift:
            --   * per-room LOGICAL size / auth  = rows WHERE room = ?  (a blob
            --     shared by two rooms counts in each; deleting a room drops
            --     only its rows).
            --   * GLOBAL PHYSICAL liveness       = COUNT(*) WHERE sha = ?  (the
            --     file is deleted only when this reaches 0 across ALL rooms, so
            --     one room's deletion never corrupts another's shared file).
            -- message_id FKs messages ON DELETE CASCADE, so deleting a message
            -- automatically drops its refs; the sweep then collects any blob
            -- whose global count fell to 0.
            CREATE TABLE IF NOT EXISTS attachment_refs (
                room TEXT NOT NULL,
                message_id INTEGER NOT NULL,
                sha TEXT NOT NULL,
                name TEXT NOT NULL,
                mime TEXT NOT NULL,
                size INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                PRIMARY KEY (room, message_id, sha),
                FOREIGN KEY (message_id) REFERENCES messages(id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS attachment_refs_room_sha
                ON attachment_refs(room, sha);
            CREATE INDEX IF NOT EXISTS attachment_refs_sha
                ON attachment_refs(sha);
            "#,
        )?;
        // Older databases predate the bootstrap-ACK column. Adding it is a no-op
        // once present. It gates re-delivery: an approved request's mb_ stays
        // re-fetchable via /bootstrap until the client explicitly ACKs (after
        // verifying mb_ with /whoami), so a crash between receiving and
        // persisting the credential can never lose it. `bootstrap_delivered_at`
        // becomes a first-delivery timestamp for observability, no longer the
        // gate; only the ACK closes the window.
        let _ = conn.execute(
            "ALTER TABLE join_requests ADD COLUMN bootstrap_acked_at INTEGER",
            [],
        );
        // Older databases predate message kinds; adding the column is a no-op
        // once it exists, and existing rows are plain speech by definition.
        let _ = conn.execute(
            "ALTER TABLE messages ADD COLUMN kind TEXT NOT NULL DEFAULT 'say'",
            [],
        );
        let _ = conn.execute("ALTER TABLE messages ADD COLUMN principal TEXT", []);
        let _ = conn.execute("ALTER TABLE messages ADD COLUMN op_id TEXT", []);
        // Attachments ride on the message row as a JSON array of the ref
        // objects (id/sha/name/mime/size), so a reloaded message shows its
        // chips exactly like the live broadcast — the word AND its files are
        // durable together. NULL/absent means no attachments. Older databases
        // predate the column; adding it is a no-op once present.
        let _ = conn.execute("ALTER TABLE messages ADD COLUMN attachments TEXT", []);
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
        // v0.9.4: Everyone reminders deliver by canonical principal. Legacy rows
        // (Lead/Person and everything written before this) keep NULL here and stay
        // on the name-based delivery path.
        let _ = conn.execute(
            "ALTER TABLE care_outbox ADD COLUMN owner_principal_id TEXT",
            [],
        );
        let _ = conn.execute(
            "CREATE INDEX IF NOT EXISTS care_outbox_delivery_principal
                ON care_outbox(delivery_room, owner_principal_id, acked_at)",
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
        // Blobs live beside the SQLite file so the same Docker data volume +
        // backup/restore captures them (RFC decision 1: STORAGE_ROOT defaults
        // to the dir of DB_PATH). A blob dir that can't be created disables
        // attachments (the endpoints answer 503) rather than sinking the whole
        // server — every other room feature keeps working.
        let blob_root = std::path::Path::new(path)
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("attachments");
        let blobs = match attachments::BlobStore::new(&blob_root) {
            Ok(store) => Some(store),
            Err(e) => {
                tracing::error!(error = %e, path = %blob_root.display(),
                    "attachment blob store unavailable — attachments disabled");
                None
            }
        };
        Ok(Store {
            conn: Some(Mutex::new(conn)),
            blobs,
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
        let Some(mut c) = self.conn() else {
            return Ok(0);
        };
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
        // `bootstrap_ready` stays true from approval until the client ACKs, so a
        // requester that restarts mid-flow re-discovers it and re-bootstraps —
        // gated on `bootstrap_acked_at`, not first delivery.
        c.query_row(
            "SELECT status, name, \
                    (mb_token IS NOT NULL AND bootstrap_acked_at IS NULL) \
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
        let Some(c) = self.conn() else {
            return Vec::new();
        };
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

    /// Is there already a live (pending or being-approved) request for this exact
    /// name? Keeps names unique across live requests so two requesters can never
    /// both be approved into the same name.
    pub fn has_pending_join_request_named(&self, name: &str) -> bool {
        let Some(c) = self.conn() else { return false };
        c.query_row(
            "SELECT 1 FROM join_requests \
             WHERE name = ?1 AND status IN ('pending', 'approving') LIMIT 1",
            params![name],
            |_| Ok(()),
        )
        .is_ok()
    }

    /// Approve a pending join request as ONE transaction: guard that it is still
    /// pending, verify the name is free, consume exactly one available admission
    /// right, insert the new Lobby member, and mark the request approved carrying
    /// its `mb_`. Because the single `Mutex<Connection>` is held for the whole
    /// transaction nothing can interleave (no `/members` insert slips between the
    /// name check and the member insert), and because it is one commit there is
    /// NO partial state: a crash or any failed step leaves the request `pending`,
    /// the name free, and the stock intact — no leaked right, no request stranded
    /// in an intermediate `approving` state (which `claim` used to make
    /// unrecoverable). It never touches an existing member, so a foreign `mb_`
    /// can never be handed out. Returns the created Membership so the caller can
    /// mirror it into the in-memory roster.
    pub fn approve_join_request_atomic(
        &self,
        request_id: &str,
        mb_token: &str,
        by: &str,
        now: u64,
    ) -> ApproveTxn {
        let Some(mut c) = self.conn() else {
            return ApproveTxn::Failed;
        };
        let tx = match c.transaction() {
            Ok(tx) => tx,
            Err(_) => return ApproveTxn::Failed,
        };

        // 1) The request must still be pending. Flip it to `approving` INSIDE the
        //    txn so a concurrent approve of the same id finds nothing to claim;
        //    the flip is invisible outside the transaction and is undone by any
        //    rollback below, so no durable `approving` state is ever observable.
        match tx.execute(
            "UPDATE join_requests SET status = 'approving' \
             WHERE id = ?1 AND status = 'pending'",
            params![request_id],
        ) {
            Ok(1) => {}
            Ok(_) => {
                let _ = tx.rollback();
                return ApproveTxn::AlreadyDecided;
            }
            Err(_) => {
                let _ = tx.rollback();
                return ApproveTxn::Failed;
            }
        }

        // The requester's chosen name + kind (immutable since creation).
        let (name, kind): (String, String) = match tx.query_row(
            "SELECT name, kind FROM join_requests WHERE id = ?1",
            params![request_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ) {
            Ok(v) => v,
            Err(_) => {
                let _ = tx.rollback();
                return ApproveTxn::Failed;
            }
        };

        // 2) The name must be free. Checked and inserted under the same held
        //    connection, so no `/members` create can slip in between.
        if tx
            .query_row(
                "SELECT 1 FROM members WHERE name = ?1 AND revoked_at IS NULL LIMIT 1",
                params![name],
                |_| Ok(()),
            )
            .is_ok()
        {
            let _ = tx.rollback(); // undoes the 'approving' flip -> back to pending
            return ApproveTxn::NameTaken;
        }

        // 3) Consume exactly one available, unexpired admission right (oldest
        //    first). The `consumed_at IS NULL` guard keeps the write single-use.
        let right_id: String = match tx.query_row(
            "SELECT id FROM admission_stock \
             WHERE consumed_at IS NULL AND expires_at > ?1 \
             ORDER BY created_at, id LIMIT 1",
            params![now],
            |r| r.get(0),
        ) {
            Ok(id) => id,
            Err(_) => {
                let _ = tx.rollback();
                return ApproveTxn::NoStock;
            }
        };
        match tx.execute(
            "UPDATE admission_stock SET consumed_at = ?1, consumed_by_name = ?2 \
             WHERE id = ?3 AND consumed_at IS NULL",
            params![now, name, right_id],
        ) {
            Ok(1) => {}
            _ => {
                let _ = tx.rollback();
                return ApproveTxn::NoStock;
            }
        }

        // 4) Insert the fresh Lobby member — into `members` AND the identity-v2
        //    principals/credentials tables (exactly what `add_member` does), so
        //    the issued `mb_` authenticates IMMEDIATELY on a persistent store.
        //    Writing only `members` here made the credential resolve only after
        //    the next restart-time migration, so a freshly-approved agent got
        //    401 at the Lobby until a restart (review blocker).
        if tx
            .execute(
                "INSERT INTO members (token, name, kind, joined_at, admitted_by, revoked_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
                params![mb_token, name, kind, now, by],
            )
            .is_err()
        {
            let _ = tx.rollback();
            return ApproveTxn::Failed;
        }
        if identity::insert_principal_credential(
            &tx,
            "member",
            mb_token,
            &name,
            &kind,
            now,
            "Member access",
        )
        .is_err()
        {
            let _ = tx.rollback();
            return ApproveTxn::Failed;
        }

        // 5) Mark the request approved, carrying the mb_ for one-time bootstrap.
        //    (Split from the commit so the guard never moves `tx`.)
        if !matches!(
            tx.execute(
                "UPDATE join_requests \
                 SET status = 'approved', mb_token = ?2, decided_by = ?3, decided_at = ?4 \
                 WHERE id = ?1 AND status = 'approving'",
                params![request_id, mb_token, by, now],
            ),
            Ok(1)
        ) {
            let _ = tx.rollback();
            return ApproveTxn::Failed;
        }
        if tx.commit().is_ok() {
            ApproveTxn::Committed(protocol::Membership {
                token: mb_token.to_string(),
                name,
                kind,
                joined_at: now,
                admitted_by: by.to_string(),
            })
        } else {
            // commit failed — nothing is persisted; the request stays pending.
            ApproveTxn::Failed
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

    /// Deliver the issued mb_, RE-FETCHABLE until the client ACKs. It returns the
    /// same mb_ on every call while the request is approved, secret-matched, and
    /// not-yet-acked, so a client that crashes or drops the connection between
    /// receiving mb_ and persisting it can simply ask again — the credential is
    /// never lost. `bootstrap_delivered_at` records the FIRST delivery time for
    /// observability only; it no longer gates re-delivery. Only
    /// `ack_join_request_bootstrap` (called after the client verifies mb_ via
    /// /whoami) closes the window, after which this returns None.
    pub fn claim_join_request_bootstrap(&self, id: &str, secret: &str, now: u64) -> Option<String> {
        let c = self.conn()?;
        let mb: String = c
            .query_row(
                "SELECT mb_token FROM join_requests \
                 WHERE id = ?1 AND secret_hash = ?2 AND status = 'approved' \
                   AND mb_token IS NOT NULL AND bootstrap_acked_at IS NULL",
                params![id, secret_hash(secret)],
                |r| r.get(0),
            )
            .ok()?;
        // First-delivery timestamp, observability only — never blocks re-delivery.
        let _ = c.execute(
            "UPDATE join_requests SET bootstrap_delivered_at = ?2 \
             WHERE id = ?1 AND bootstrap_delivered_at IS NULL",
            params![id, now],
        );
        Some(mb)
    }

    /// Finalize the bootstrap: the client has verified its mb_ (via /whoami) and
    /// persisted it, so close the re-fetch window. After this the bootstrap
    /// endpoint returns None. Returns `Some(true)` when this call set the ACK,
    /// `Some(false)` when the request was already acked by this same secret (a
    /// safe retry), and `None` when the request is unknown, not owned by this
    /// secret, or not in the approved state. Requiring the secret means only the
    /// requester can close their own window.
    pub fn ack_join_request_bootstrap(&self, id: &str, secret: &str, now: u64) -> Option<bool> {
        let c = self.conn()?;
        // Ownership + state, reading the current ack timestamp. We deliberately do
        // NOT require `mb_token IS NOT NULL` here: a completed ACK scrubs mb_token
        // (below), so a repeat ACK by the same secret must still resolve as an
        // idempotent success, not a 404. The held connection guard spans this
        // SELECT and the UPDATE, so a concurrent bootstrap/ACK cannot interleave.
        let acked_at: Option<u64> = c
            .query_row(
                "SELECT bootstrap_acked_at FROM join_requests \
                 WHERE id = ?1 AND secret_hash = ?2 AND status = 'approved'",
                params![id, secret_hash(secret)],
                |r| r.get::<_, Option<u64>>(0),
            )
            .ok()?;
        if acked_at.is_some() {
            return Some(false); // already finalized by this secret — idempotent
        }
        // Finalize atomically: stamp the ACK AND scrub the plaintext mb_ copy from
        // the request row, so a DB dump never retains the delivered credential.
        // The live membership itself lives in the members/credentials tables and
        // is untouched — only this one-time-delivery copy is erased.
        let changed = c
            .execute(
                "UPDATE join_requests SET bootstrap_acked_at = ?2, mb_token = NULL \
                 WHERE id = ?1 AND bootstrap_acked_at IS NULL",
                params![id, now],
            )
            .ok()?;
        Some(changed == 1)
    }

    /// Test-only: read the raw stored `mb_token` for a request. `Some(Some(_))`
    /// when a credential copy is present, `Some(None)` when it has been scrubbed,
    /// `None` when the row is absent. Proves the ACK erases the credential.
    #[cfg(test)]
    pub fn join_request_mb_token_raw(&self, id: &str) -> Option<Option<String>> {
        let c = self.conn()?;
        c.query_row(
            "SELECT mb_token FROM join_requests WHERE id = ?1",
            params![id],
            |r| r.get::<_, Option<String>>(0),
        )
        .ok()
    }

    pub fn is_persistent(&self) -> bool {
        self.conn.is_some()
    }

    fn conn(&self) -> Option<std::sync::MutexGuard<'_, Connection>> {
        self.conn.as_ref().map(|m| m.lock_or_recover())
    }
}

/// Outcome of atomically approving a join request (single transaction).
#[derive(Debug)]
pub enum ApproveTxn {
    /// Stock consumed, Lobby member inserted, request approved. Carries the new
    /// membership so the caller can mirror it into the in-memory roster.
    Committed(protocol::Membership),
    /// The request was no longer pending (already approved/denied, or a racing
    /// approve won) — nothing changed and no stock was consumed.
    AlreadyDecided,
    /// The requested name already belongs to a live member — refused, and the
    /// request is left pending. No stock consumed.
    NameTaken,
    /// No available (unexpired) admission right — request left pending so the
    /// Master can retry after minting more. No stock consumed.
    NoStock,
    /// A DB error rolled the whole transaction back — nothing changed.
    Failed,
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

/// Serialize a message's attachment refs for the `messages.attachments` column.
/// An empty list stores NULL, so ordinary messages keep a NULL column and the
/// on-disk footprint is unchanged from before attachments existed.
fn attachments_to_json(attachments: &[protocol::Attachment]) -> Option<String> {
    if attachments.is_empty() {
        return None;
    }
    serde_json::to_string(attachments).ok()
}

/// Parse the `messages.attachments` column back into refs. NULL, absent, or a
/// value that fails to parse yields an empty list — a malformed column never
/// blocks reloading the message itself (the text is what must not be lost).
fn attachments_from_json(raw: Option<String>) -> Vec<protocol::Attachment> {
    raw.and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
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
