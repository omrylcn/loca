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
mod dm;
pub(crate) use attachment_index::{AttachError, BlobServe};
use claims::PAST_CLAIM_DELIVERY_DEADLINE;
pub use claims::{MemberClaim, CLAIM_DELIVERY_WINDOW_MS, MEMBERSHIP_CLAIMS_PER_INVITE};
pub use dm::Dm;
pub use groups::{GroupLogEntry, GroupRecord};
pub use identity::{AdminSessionRecord, InviteRecord, MemberRecord, SmasterRecord};
pub use keys::KeyRecord;
pub use notifications::{
    NotificationPage, NotificationRow, NotificationSend, SendLimit, SentNotificationRow,
};
mod claims;
mod content;
mod groups;
mod identity;
mod keys;
pub(crate) mod loca_removal;
mod memory;
mod messages;
mod notifications;
mod operators;
mod preferences;
mod profile_card;
mod rooms;
mod vault;
mod wiki;
mod wiki_context;
mod wiki_maintenance;
mod work;
pub use preferences::{PreferenceError, Preferences};
pub use profile_card::{CardPatch, ProfileCard, ProfileCardError};
pub use vault::VaultRevision;
pub use wiki::{
    WikiCommit, WikiError, WikiPageEdit, WikiPageRemoval, WikiPageSnapshot, WikiSnapshot,
};
pub use wiki_context::{WikiContextAck, WikiContextSelection};
pub use wiki_maintenance::{WikiBatch, WikiLease};

/// Unicode case-insensitive, literal substring matching shared by SQL and notes.
/// Capital dotted I folds to i too, so Turkish names do not diverge from notes.
pub(crate) fn search_fold(text: &str) -> String {
    text.replace('\u{0130}', "i").to_lowercase()
}

/// A stored scope key (`messages.principal`, `message_reactions.principal`,
/// `attachment_pending.uploader`) with the membership key or invite it names,
/// as earlier versions wrote it, replaced by that one's management id:
/// `mb:<key>` becomes `mb:<mbid_>` and `dv:<invite>` becomes `dv:<ivid_>`,
/// with the derivations the hub uses (`Hub::member_management_id`,
/// `Hub::invite_management_id`). Every other value, an id-form one included,
/// is `None`: the rewrite is idempotent.
pub(crate) fn scope_key_by_id(scope: &str) -> Option<String> {
    if let Some(key) = scope.strip_prefix("mb:") {
        return (!key.is_empty() && !key.starts_with("mbid_"))
            .then(|| format!("mb:{}", hashed_id("mbid_", &format!("member:{key}"))));
    }
    if let Some(invite) = scope.strip_prefix("dv:") {
        return (!invite.is_empty() && !invite.starts_with("ivid_"))
            .then(|| format!("dv:{}", hashed_id("ivid_", &format!("invite:{invite}"))));
    }
    None
}

use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

use protocol::{
    Attention, AttentionStatus, ChatMode, Goal, GoalCompletion, GoalStatus, Message,
    MessageReaction, Note, RoomSettings, SenderType, Task, TaskStatus, WaitState,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryReadError {
    PersistenceUnavailable,
    NotConfigured,
    InvariantViolation,
    Storage,
}

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

pub struct ModerationRecord {
    pub room: String,
    pub name: String,
    pub kind: String,
    pub authority_rank: u8,
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
        Self::open_at(path, current_time_ms())
    }

    /// `open`, with the time the open happens at given by the caller, so a
    /// test can open a database as it would be found after a long stop. The
    /// time is used only for what the open itself expires: join-request key
    /// copies past `JOIN_BOOTSTRAP_WINDOW_MS` and invite delivery copies past
    /// `INVITE_DELIVERY_WINDOW_MS` (and, on the upgrade that adds them, which
    /// live invites are still young enough to get one).
    pub fn open_at(path: Option<&str>, now: u64) -> rusqlite::Result<Self> {
        let Some(path) = path else {
            return Ok(Store {
                conn: None,
                blobs: None,
            });
        };
        let mut conn = Connection::open(path)?;
        conn.create_scalar_function(
            "loca_fold",
            1,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8
                | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
            |ctx| Ok(search_fold(&ctx.get::<String>(0)?)),
        )?;
        conn.create_scalar_function(
            "loca_scope_by_id",
            1,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8
                | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
            |ctx| {
                let scope = ctx.get::<String>(0)?;
                Ok(scope_key_by_id(&scope).unwrap_or(scope))
            },
        )?;
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
            -- Loca memory is deliberately separate from notes. Notes have a
            -- general DELETE path; memory has no DELETE operation anywhere.
            CREATE TABLE IF NOT EXISTS loca_memory (
                room TEXT PRIMARY KEY,
                owner TEXT,
                short TEXT NOT NULL DEFAULT '',
                long TEXT NOT NULL DEFAULT '',
                short_updated_at INTEGER,
                long_updated_at INTEGER,
                version INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS loca_memory_entries (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                room TEXT NOT NULL,
                text TEXT NOT NULL,
                decided_by TEXT NOT NULL,
                decided_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS loca_memory_entries_room
                ON loca_memory_entries(room, id);
            CREATE TABLE IF NOT EXISTS loca_wiki (
                room TEXT PRIMARY KEY,
                editor_principal TEXT NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 0,
                interval_messages INTEGER NOT NULL DEFAULT 30,
                revision INTEGER NOT NULL DEFAULT 0,
                reviewed_through INTEGER NOT NULL DEFAULT 0,
                reviewed_at INTEGER,
                edited_at INTEGER
            );
            CREATE TABLE IF NOT EXISTS loca_wiki_pages (
                room TEXT NOT NULL,
                slug TEXT NOT NULL,
                title TEXT NOT NULL,
                body TEXT NOT NULL,
                sources TEXT NOT NULL,
                revision INTEGER NOT NULL,
                PRIMARY KEY(room, slug)
            );
            CREATE TABLE IF NOT EXISTS loca_wiki_history (
                room TEXT NOT NULL,
                slug TEXT NOT NULL,
                revision INTEGER NOT NULL,
                title TEXT NOT NULL,
                body TEXT NOT NULL,
                sources TEXT NOT NULL,
                editor_principal TEXT NOT NULL,
                reason TEXT NOT NULL,
                edited_at INTEGER NOT NULL,
                PRIMARY KEY(room, slug, revision)
            );
            CREATE TABLE IF NOT EXISTS loca_wiki_batches (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                room TEXT NOT NULL,
                editor_principal TEXT NOT NULL,
                after_id INTEGER NOT NULL,
                through_id INTEGER NOT NULL,
                status TEXT NOT NULL DEFAULT 'queued',
                reason TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                epoch INTEGER NOT NULL DEFAULT 0,
                attempts INTEGER NOT NULL DEFAULT 0,
                lease_expires_at INTEGER NOT NULL DEFAULT 0
            );
            CREATE UNIQUE INDEX IF NOT EXISTS wiki_one_active_batch
                ON loca_wiki_batches(room)
                WHERE status IN ('queued', 'running', 'blocked');
            CREATE TABLE IF NOT EXISTS loca_wiki_leases (
                batch_id INTEGER NOT NULL,
                epoch INTEGER NOT NULL,
                started_at INTEGER NOT NULL,
                PRIMARY KEY(batch_id, epoch)
            );
            CREATE TABLE IF NOT EXISTS loca_wiki_receipts (
                room TEXT NOT NULL,
                batch_id INTEGER NOT NULL,
                epoch INTEGER NOT NULL,
                editor_principal TEXT NOT NULL,
                request TEXT NOT NULL,
                revision INTEGER NOT NULL,
                PRIMARY KEY(room, batch_id, epoch)
            );
            CREATE TABLE IF NOT EXISTS wiki_native_runtime (
                room TEXT NOT NULL, principal TEXT NOT NULL, runtime TEXT NOT NULL,
                ready_at INTEGER NOT NULL, PRIMARY KEY(room, principal)
            );
            CREATE TABLE IF NOT EXISTS wiki_context_selection (
                room TEXT NOT NULL, principal TEXT NOT NULL, slugs TEXT NOT NULL,
                reason TEXT NOT NULL, version INTEGER NOT NULL DEFAULT 1,
                requested_at INTEGER NOT NULL, PRIMARY KEY(room, principal)
            );
            CREATE TABLE IF NOT EXISTS wiki_context_receipts (
                room TEXT NOT NULL, principal TEXT NOT NULL, context_id TEXT NOT NULL,
                packet_id TEXT NOT NULL, revision INTEGER NOT NULL,
                selection_version INTEGER NOT NULL, accepted_at INTEGER NOT NULL,
                PRIMARY KEY(room, principal, context_id, packet_id)
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
            -- Davets (`invites`) are created after this batch from
            -- `invites_table_sql`, like the Smaster registry below.
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
            -- Memberships (`members`) are created after this batch from
            -- `members_table_sql`.
            -- The Smaster registry (`smasters`) is created after this batch
            -- from `smasters_table_sql`, so a fresh database and a migrated one
            -- get the same shape from one definition.
            -- Bans and mutes: per-loca door state that must survive a restart.
            -- Without this a restart empties the sets while davets reload from
            -- disk, so a banned name walks right back in. `kind` is 'ban' or
            -- 'mute'. Keyed by (room, name, kind); clearing = DELETE the row.
            CREATE TABLE IF NOT EXISTS bans (
                room TEXT NOT NULL,
                name TEXT NOT NULL,
                kind TEXT NOT NULL,        -- 'ban' | 'mute'
                at INTEGER NOT NULL,
                authority_rank INTEGER NOT NULL DEFAULT 3,
                decided_by TEXT,
                PRIMARY KEY (room, name, kind)
            );
            -- Browser admin sessions (`admin_sessions`) are created after
            -- this batch from `admin_sessions_table_sql`, like the Smaster
            -- registry below.
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
            -- Suspensions (`member_suspensions`) are created after this batch
            -- from `member_suspensions_table_sql`.
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
            -- A membership key a davet holder claimed (`POST
            -- /membership/claim`): one row per set-up, each with a credential
            -- of its own for the member's principal. `invite_id` is the
            -- davet's `ivid_`, never the davet. `claim_hash` is the hash of
            -- the set-up's claim id, NULL for an older kit that sends none
            -- (each such call is a set-up of its own). `delivery_token` is
            -- the bounded clear-text copy that lets the same set-up collect
            -- the same key again; it is cleared by the set-up's ACK or 24
            -- hours after the claim (see `store/claims.rs`), and is only ever
            -- written for a set-up that has a claim id.
            CREATE TABLE IF NOT EXISTS membership_claims (
                credential_id TEXT PRIMARY KEY REFERENCES credentials(id),
                invite_id TEXT NOT NULL,
                claim_hash TEXT,
                claimed_at INTEGER NOT NULL,
                delivery_token TEXT,
                acked_at INTEGER,
                expired_at INTEGER,
                UNIQUE (invite_id, claim_hash)
            );
            -- Personal appearance preferences (theme, accent, density, font
            -- size). One validated JSON document per principal, so the look
            -- follows the person to every device instead of one browser.
            CREATE TABLE IF NOT EXISTS principal_preferences (
                principal_id TEXT PRIMARY KEY REFERENCES principals(id),
                json TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );
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
            -- A birebir: a private loca for exactly two principals. `a` < `b`
            -- so the pair maps to one row; the room id is derived from it.
            -- The Master's call-in is the only third-party access, recorded
            -- here with who called and since when; earlier history opens only
            -- when both sides allow it.
            -- Every Master pairing code handed out. The code itself is never
            -- stored: only a short fingerprint to recognise it, when it was
            -- issued, until when it could be used, and what became of it.
            CREATE TABLE IF NOT EXISTS pairing_log (
                id TEXT PRIMARY KEY,
                fingerprint TEXT NOT NULL,
                issued_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                session_ttl_ms INTEGER NOT NULL,
                used_at INTEGER,
                superseded_at INTEGER
            );
            CREATE TABLE IF NOT EXISTS birebirs (
                room TEXT PRIMARY KEY,
                a TEXT NOT NULL,
                b TEXT NOT NULL,
                opened_by TEXT NOT NULL,
                opened_at INTEGER NOT NULL,
                master_called_by TEXT,
                master_since INTEGER,
                history_a INTEGER NOT NULL DEFAULT 0,
                history_b INTEGER NOT NULL DEFAULT 0,
                UNIQUE (a, b)
            );
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
        // Groups (PRINCIPLES "Groups"). An older database simply gains empty
        // tables: no loca belongs to a group, so every loca stays default.
        conn.execute_batch(groups::SCHEMA)?;
        // Notifications (docs/concepts.md "Notifications"): new tables only.
        conn.execute_batch(notifications::SCHEMA)?;
        // The Vault's rules (PRINCIPLES "Vault"): a new table only.
        conn.execute_batch(vault::SCHEMA)?;
        // The Smaster registry. A database written by v0.13.0 or earlier
        // already has an older `smasters` table holding the codes in clear
        // text; this is then a no-op and `migrate_smaster_codes` below
        // rebuilds that table into this shape.
        conn.execute_batch(&smasters_table_sql("IF NOT EXISTS smasters"))?;
        // Persisted browser admin sessions. Likewise, a database written
        // before this version has an older `admin_sessions` table keyed by
        // the session token itself; `migrate_admin_session_tokens` below
        // rebuilds it into this shape.
        conn.execute_batch(&admin_sessions_table_sql("IF NOT EXISTS admin_sessions"))?;
        // Memberships, invites and suspensions. A database written before
        // this version already has the older tables, keyed by the key and
        // the invite themselves; these are then no-ops and
        // `migrate_member_keys` below rebuilds those tables into this shape.
        conn.execute_batch(&members_table_sql("IF NOT EXISTS members"))?;
        conn.execute_batch(&invites_table_sql("IF NOT EXISTS invites"))?;
        conn.execute_batch(&member_suspensions_table_sql(
            "IF NOT EXISTS member_suspensions",
        ))?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS invites_room ON invites(room)",
            [],
        )?;
        conn.execute_batch(STORAGE_MAINTENANCE_SCHEMA)?;
        // Older databases predate the bootstrap-ACK column. Adding it is a no-op
        // once present. It gates re-delivery: an approved request's mb_ stays
        // re-fetchable via /bootstrap until the client explicitly ACKs (after
        // verifying mb_ with /whoami), so a crash between receiving and
        // persisting the credential can never lose it. `bootstrap_delivered_at`
        // becomes a first-delivery timestamp for observability, no longer the
        // gate; only the ACK closes the window.
        add_column(&conn, "join_requests", "bootstrap_acked_at INTEGER")?;
        // When an approved request's key copy was cleared because nobody
        // collected and acknowledged it within `JOIN_BOOTSTRAP_WINDOW_MS`.
        // NULL on every other row; it is what lets the requester be told
        // "expired" rather than "already finalized".
        add_column(&conn, "join_requests", "bootstrap_expired_at INTEGER")?;
        // Older databases predate message kinds; adding the column is a no-op
        // once it exists, and existing rows are plain speech by definition.
        add_column(&conn, "messages", "kind TEXT NOT NULL DEFAULT 'say'")?;
        add_column(&conn, "messages", "principal TEXT")?;
        // Unknown legacy decisions are Master-protected, never downgraded
        // during migration. A column that cannot be added stops the open.
        add_column(&conn, "bans", "authority_rank INTEGER NOT NULL DEFAULT 3")?;
        add_column(&conn, "bans", "decided_by TEXT")?;
        add_column(&conn, "messages", "op_id TEXT")?;
        // Attachments ride on the message row as a JSON array of the ref
        // objects (id/sha/name/mime/size), so a reloaded message shows its
        // chips exactly like the live broadcast — the word AND its files are
        // durable together. NULL/absent means no attachments. Older databases
        // predate the column; adding it is a no-op once present.
        add_column(&conn, "messages", "attachments TEXT")?;
        // Memory frames carry a durable monotonic revision. Existing rows begin
        // at zero and advance on every owner or body mutation.
        add_column(&conn, "loca_memory", "version INTEGER NOT NULL DEFAULT 0")?;
        conn.execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_messages_operation
             ON messages(room, principal, op_id)
             WHERE principal IS NOT NULL AND op_id IS NOT NULL",
            [],
        )?;
        // An invite now seats a MEMBER — identity comes from the membership
        // record, the invite's name/kind are an audit snapshot. Legacy rows
        // get an empty member and are bound at load (migration). Only an
        // older `invites` table (keyed by the invite) can lack the column; the
        // current shape names the member by its id (`member_id`).
        if table_has_column(&conn, "invites", "token")? {
            add_column(&conn, "invites", "member TEXT NOT NULL DEFAULT ''")?;
        }
        // An invite minted for delivery keeps a clear-text delivery copy until
        // its first use (`first_used_at`) or `INVITE_DELIVERY_WINDOW_MS` after
        // issue, whichever comes first; while it exists the member's Lobby is
        // sent that invite on every connect (see `expire_invite_copies`).
        add_invite_delivery_columns(&mut conn, now)?;
        // A deleted loca is SEALED, not destroyed (PRINCIPLES: "seal not
        // destroy" — the record survives so "what happened here" stays
        // answerable). `sealed_at` marks it; the row and its history stay on
        // disk, and boot re-tombstones it so it never silently reopens.
        add_column(&conn, "rooms", "sealed_at INTEGER")?;
        // Goal/task care is based on explicit progress, never on unrelated
        // chat. Existing rows start at creation time so upgrades do not
        // manufacture artificial recent progress.
        add_column(&conn, "tasks", "progress_at INTEGER NOT NULL DEFAULT 0")?;
        conn.execute(
            "UPDATE tasks SET progress_at = created_at WHERE progress_at = 0",
            [],
        )?;
        // Who a task was given to, as the verified principal behind the name
        // (`Task::assigned_principal`). NULL on every older row: nothing is
        // inferred from a name that someone else may wear today.
        add_column(&conn, "tasks", "assigned_principal TEXT")?;
        // Who made the current assignment and with what authority, and the
        // authority and principal a task was declared with (the Lead's
        // bounds). NULL on every older row: an assigned older task counts as
        // the Operator's, and an older task as declared by an operator.
        add_column(&conn, "tasks", "assigned_by TEXT")?;
        add_column(&conn, "tasks", "assigned_as TEXT")?;
        add_column(&conn, "tasks", "created_as TEXT")?;
        add_column(&conn, "tasks", "created_principal TEXT")?;
        // The Loca Board's review note and no-go reason; NULL elsewhere.
        add_column(&conn, "tasks", "review_note TEXT")?;
        add_column(&conn, "tasks", "review_reason TEXT")?;
        add_column(&conn, "goals", "progress_at INTEGER NOT NULL DEFAULT 0")?;
        add_column(&conn, "goals", "checkpoint TEXT")?;
        add_column(&conn, "goals", "stale_after_secs INTEGER")?;
        conn.execute(
            "UPDATE goals SET progress_at = created_at WHERE progress_at = 0",
            [],
        )?;
        add_column(
            &conn,
            "care_outbox",
            "attention_id TEXT NOT NULL DEFAULT ''",
        )?;
        conn.execute(
            "UPDATE care_outbox SET attention_id = id WHERE attention_id = ''",
            [],
        )?;
        // v0.9.4: Everyone reminders deliver by canonical principal. Legacy rows
        // (Lead/Person and everything written before this) keep NULL here and stay
        // on the name-based delivery path.
        add_column(&conn, "care_outbox", "owner_principal_id TEXT")?;
        // A person's optional full name and profile icon (`store/profile_card.rs`):
        // display only, never an address or an authority. NULL is "none set",
        // so every older row reads as before.
        add_column(&conn, "principals", "full_name TEXT")?;
        add_column(&conn, "principals", "icon TEXT")?;
        // The avatar's colour and shape beside that icon, display only and
        // from closed lists; NULL is "none set", so older rows read as before.
        add_column(&conn, "principals", "avatar_color TEXT")?;
        add_column(&conn, "principals", "avatar_shape TEXT")?;
        // A called-in Master's boundary is ordered with message writes since
        // this column exists. Visits recorded before it end here, once.
        add_column(
            &conn,
            "birebirs",
            "master_since_ordered INTEGER NOT NULL DEFAULT 0",
        )?;
        let ended = dm::end_unordered_dm_visits(&conn)?;
        if ended > 0 {
            tracing::warn!(
                ended,
                "ended Master visits recorded before the ordered call boundary"
            );
        }
        conn.execute(
            "CREATE INDEX IF NOT EXISTS care_outbox_delivery_principal
                ON care_outbox(delivery_room, owner_principal_id, acked_at)",
            [],
        )?;
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
        // Senders, reactors and uploaders that earlier versions stored by a
        // membership key or an invite are named by its id. A failure fails the
        // open and leaves every old value; the WAL clean-up after it is the
        // owed chore settled below. It runs before the clean-up of the
        // migrations' residue, so the one `VACUUM` that follows a migration
        // also rewrites the pages these rows were freed from.
        let rewritten = with_temp_store_in_memory(&mut conn, rewrite_key_scopes)?;
        if rewritten > 0 {
            tracing::info!(
                rewritten,
                "older messages, reactions and uploads now name their sender by its membership or invite id, not its key"
            );
        }
        migrate_smaster_codes(&mut conn)?;
        migrate_admin_session_tokens(&mut conn)?;
        migrate_member_keys(&mut conn)?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS invites_member ON invites(member_id)",
            [],
        )?;
        // One clean-up for whatever the migrations above left in the file,
        // so a database that needed several is rewritten once.
        scrub_secret_residue(&conn)?;
        // A server that was stopped across a delivery deadline must not come
        // back still holding the key copy. A copy that cannot be cleared is an
        // error like any other the open meets; the WAL clean-up after it is
        // best-effort and, when busy, stays owed (see `settle_owed_checkpoint`).
        let expired = expire_delivery_copies(&mut conn, now, DeliveryCopies::PastDeadline)?;
        if expired > 0 {
            tracing::info!(
                expired,
                "cleared key delivery copies (join requests, membership claims) whose window closed while the server was stopped"
            );
        }
        // The same for invite delivery copies: one issued before the stop and
        // never used is not delivered past its deadline.
        let expired = expire_invite_copies(&mut conn, now, None)?;
        if expired > 0 {
            tracing::info!(
                expired,
                "cleared invite delivery copies whose window closed while the server was stopped"
            );
        }
        settle_owed_checkpoint_logged(&conn);
        // Sessions that ended while the server was stopped go at the open,
        // like any other. A row written before every session had a lifetime
        // (`expires_at = 0`) belonged to a session held only in the old
        // process's memory, so nothing can present it any more: it goes too.
        let swept = identity::sweep_session_rows(&conn, now)?;
        if swept > 0 {
            tracing::info!(
                swept,
                "removed sessions that ended while the server was stopped"
            );
        }
        attribute_messages_once(&mut conn)?;
        // Bind legacy stewardship once. Never resolve an unresolved label on
        // a later admission: that would hand old authority to a reused name.
        let has_owner_principal = {
            let mut statement = conn.prepare("PRAGMA table_info(loca_memory)")?;
            let columns = statement
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            columns.iter().any(|column| column == "owner_principal_id")
        };
        if !has_owner_principal {
            let tx = conn.transaction()?;
            tx.execute(
                "ALTER TABLE loca_memory ADD COLUMN owner_principal_id TEXT REFERENCES principals(id)",
                [],
            )?;
            tx.execute(
                "UPDATE loca_memory SET owner_principal_id = (
                    SELECT id FROM principals WHERE display_name = loca_memory.owner
                    AND disabled_at IS NULL
                 ) WHERE owner IS NOT NULL AND
                 (SELECT count(*) FROM principals WHERE display_name = loca_memory.owner) = 1",
                [],
            )?;
            tx.commit()?;
        }
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

    /// What the requester may know about its own request, if `id` + `secret`
    /// match; `Ok(None)` otherwise (a caller learns nothing about a request
    /// that is not theirs). A storage error is an error, never "no such
    /// request": the requester's helper forgets its request when told it does
    /// not exist, so a passing database fault must not read as that.
    pub fn join_request_view(
        &self,
        id: &str,
        secret: &str,
        now: u64,
    ) -> rusqlite::Result<Option<JoinRequestView>> {
        let Some(c) = self.conn() else {
            return Ok(None);
        };
        // `bootstrap_ready` stays true from approval until the client ACKs, so a
        // requester that restarts mid-flow re-discovers it and re-bootstraps —
        // gated on `bootstrap_acked_at`, not first delivery — but never past
        // the delivery deadline, even before anything has cleared the copy.
        // `bootstrap_expired` is never true once the ACK is in: an acknowledged
        // request ended normally, whatever the clock says afterwards.
        c.query_row(
            &format!(
                "SELECT status, name, \
                        (mb_token IS NOT NULL AND bootstrap_acked_at IS NULL \
                         AND NOT {past}), \
                        (status = 'approved' AND bootstrap_acked_at IS NULL \
                         AND (bootstrap_expired_at IS NOT NULL \
                              OR (mb_token IS NOT NULL AND {past}))) \
                 FROM join_requests WHERE id = :id AND secret_hash = :secret_hash",
                past = PAST_JOIN_BOOTSTRAP_DEADLINE
            ),
            rusqlite::named_params! {
                ":id": id,
                ":secret_hash": secret_hash(secret),
                ":now": now,
                ":window": JOIN_BOOTSTRAP_WINDOW_MS,
            },
            |r| {
                Ok(JoinRequestView {
                    status: r.get(0)?,
                    name: r.get(1)?,
                    bootstrap_ready: r.get(2)?,
                    bootstrap_expired: r.get(3)?,
                })
            },
        )
        .optional()
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

    /// Is there already a live (pending or being-approved) request for this
    /// name, in any letter case? Keeps names unique across live requests so two
    /// requesters can never both be approved into the same name, or into two
    /// names one `@mention` would call together.
    pub fn has_pending_join_request_named(&self, name: &str) -> bool {
        let Some(c) = self.conn() else { return false };
        c.query_row(
            "SELECT 1 FROM join_requests \
             WHERE lower(name) = lower(?1) AND status IN ('pending', 'approving') LIMIT 1",
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
    /// can never be handed out. Returns the created membership's record so the
    /// caller can mirror it into the in-memory roster.
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

        // 2) The name must be free, in any letter case (a member admitted
        //    directly since the request was filed may wear it). Checked and
        //    inserted under the same held connection, so no `/members` create
        //    can slip in between.
        if tx
            .query_row(
                "SELECT 1 FROM members \
                 WHERE lower(name) = lower(?1) AND revoked_at IS NULL LIMIT 1",
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
        //    401 at the Lobby until a restart (review blocker). The member row
        //    keeps the key's hash and ids, never the key.
        let ids = identity::MemberIds::from_key(mb_token);
        let mut member = MemberRecord {
            id: ids.id.clone(),
            secret_hash: ids.secret_hash.clone(),
            principal_id: None,
            name,
            kind,
            joined_at: now,
            admitted_by: by.to_string(),
        };
        match identity::insert_member_record(&tx, "members", &ids, &member, None, "Member access") {
            Ok(principal_id) => member.principal_id = Some(principal_id),
            Err(_) => {
                let _ = tx.rollback();
                return ApproveTxn::Failed;
            }
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
            ApproveTxn::Committed(member)
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

    /// Deliver the issued mb_, RE-FETCHABLE until the client ACKs or the
    /// delivery deadline passes. It returns the same mb_ on every call while
    /// the request is approved, secret-matched, not-yet-acked and inside
    /// `JOIN_BOOTSTRAP_WINDOW_MS` of its approval, so a client that crashes or
    /// drops the connection between receiving mb_ and persisting it can simply
    /// ask again — the credential is never lost inside the window.
    /// `bootstrap_delivered_at` records the FIRST delivery time for
    /// observability only; it does not gate re-delivery.
    ///
    /// A call at or after the deadline gets `Expired` and clears the copy in
    /// the same call (see `expire_delivery_copies`), so a late caller is the
    /// last one who could have asked. `Unavailable` covers everything a caller
    /// may not learn more about: unknown id, wrong secret, not approved, or
    /// already acknowledged. A storage error is returned as an error, never as
    /// either of those answers and never as a delivery.
    pub fn claim_join_request_bootstrap(
        &self,
        id: &str,
        secret: &str,
        now: u64,
    ) -> rusqlite::Result<BootstrapClaim> {
        let Some(mut c) = self.conn() else {
            return Ok(BootstrapClaim::Unavailable);
        };
        let row: Option<(Option<String>, bool, bool, bool, bool)> = c
            .query_row(
                &format!(
                    "SELECT mb_token, bootstrap_acked_at IS NOT NULL, \
                            bootstrap_expired_at IS NOT NULL, {PAST_JOIN_BOOTSTRAP_DEADLINE}, \
                            bootstrap_delivered_at IS NOT NULL \
                     FROM join_requests \
                     WHERE id = :id AND secret_hash = :secret_hash AND status = 'approved'"
                ),
                rusqlite::named_params! {
                    ":id": id,
                    ":secret_hash": secret_hash(secret),
                    ":now": now,
                    ":window": JOIN_BOOTSTRAP_WINDOW_MS,
                },
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        let Some((mb, acked, cleared, past, delivered)) = row else {
            return Ok(BootstrapClaim::Unavailable);
        };
        if acked {
            return Ok(BootstrapClaim::Unavailable);
        }
        let Some(mb) = mb else {
            // Approved, not acknowledged, no copy: only the deadline clears it
            // that way (approval writes the copy in its own transaction).
            return Ok(if cleared {
                BootstrapClaim::Expired
            } else {
                BootstrapClaim::Unavailable
            });
        };
        if past {
            // The held connection guard spans the read above and this clear,
            // so no other call can deliver the copy in between.
            expire_delivery_copies(&mut c, now, DeliveryCopies::JoinRequest(id))?;
            settle_owed_checkpoint_logged(&c);
            return Ok(BootstrapClaim::Expired);
        }
        // First-delivery timestamp, observability only — never blocks
        // re-delivery, and a failure to write it does not withhold a key that
        // was read. It rewrites the row that holds the copy, so it runs with
        // `secure_delete` like the two updates that clear it: otherwise the
        // row's previous image, key included, would stay in the page's free
        // space after the copy itself is gone.
        if !delivered {
            let stamped = with_secure_delete(&mut c, |conn| {
                conn.execute(
                    "UPDATE join_requests SET bootstrap_delivered_at = ?2 \
                     WHERE id = ?1 AND bootstrap_delivered_at IS NULL",
                    params![id, now],
                )
            });
            if let Err(error) = stamped {
                tracing::warn!(%error, "could not record a join-request key's first delivery");
            }
        }
        Ok(BootstrapClaim::Delivered(mb))
    }

    /// Finalize the bootstrap: the client has verified its mb_ (via /whoami) and
    /// persisted it, so close the re-fetch window. After this the bootstrap
    /// endpoint answers `Unavailable`. Returns `Some(true)` when this call set the ACK,
    /// `Some(false)` when the request was already acked by this same secret (a
    /// safe retry), and `None` when the request is unknown, not owned by this
    /// secret, or not in the approved state. Requiring the secret means only the
    /// requester can close their own window. A storage error is an error.
    ///
    /// An ACK after the deadline still succeeds: the requester may have
    /// collected the key in time and only lost the ACK, and it must not see a
    /// failure on every retry. Such an ACK records the receipt and nothing
    /// else: the cleared copy stays cleared and the window stays closed (the
    /// UPDATE below only ever sets `mb_token` to NULL).
    pub fn ack_join_request_bootstrap(
        &self,
        id: &str,
        secret: &str,
        now: u64,
    ) -> rusqlite::Result<Option<bool>> {
        let Some(mut c) = self.conn() else {
            return Ok(None);
        };
        // Ownership + state, reading the current ack timestamp. We deliberately do
        // NOT require `mb_token IS NOT NULL` here: a completed ACK scrubs mb_token
        // (below), so a repeat ACK by the same secret must still resolve as an
        // idempotent success, not a 404. The held connection guard spans this
        // SELECT and the UPDATE, so a concurrent bootstrap/ACK cannot interleave.
        let row: Option<(Option<u64>, bool)> = c
            .query_row(
                "SELECT bootstrap_acked_at, mb_token IS NOT NULL FROM join_requests \
                 WHERE id = ?1 AND secret_hash = ?2 AND status = 'approved'",
                params![id, secret_hash(secret)],
                |r| Ok((r.get::<_, Option<u64>>(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((acked_at, holds_copy)) = row else {
            return Ok(None);
        };
        if acked_at.is_some() {
            return Ok(Some(false)); // already finalized by this secret — idempotent
        }
        // Finalize atomically: stamp the ACK AND scrub the plaintext mb_ copy from
        // the request row. The live membership itself lives in the members and
        // credentials tables and is untouched — only this one-time-delivery copy
        // is erased. `secure_delete` zeroes the row's old image in its page;
        // the WAL frames that still hold it go with the truncating checkpoint
        // owed below (best-effort, see `settle_owed_checkpoint`).
        let changed = with_secure_delete(&mut c, |conn| {
            let tx = conn.transaction()?;
            let changed = tx.execute(
                "UPDATE join_requests SET bootstrap_acked_at = ?2, mb_token = NULL \
                 WHERE id = ?1 AND bootstrap_acked_at IS NULL",
                params![id, now],
            )?;
            if holds_copy {
                record_owed_checkpoint(&tx)?;
            }
            tx.commit()?;
            Ok(changed)
        })?;
        if holds_copy {
            settle_owed_checkpoint_logged(&c);
        }
        Ok(Some(changed == 1))
    }

    /// Clear every key delivery copy whose window has closed: a join
    /// request's (`JOIN_BOOTSTRAP_WINDOW_MS` after approval) and a membership
    /// claim's (`CLAIM_DELIVERY_WINDOW_MS` after the claim). Called by the
    /// hub's background sweep; the open and a late collection do the same.
    /// Returns how many copies were cleared. Then retries a WAL clean-up that
    /// is owed, from this sweep or an earlier ACK or expiry, best-effort.
    pub fn expire_join_request_bootstraps(&self, now: u64) -> rusqlite::Result<usize> {
        let Some(mut c) = self.conn() else {
            return Ok(0);
        };
        let cleared = expire_delivery_copies(&mut c, now, DeliveryCopies::PastDeadline)?;
        settle_owed_checkpoint_logged(&c);
        Ok(cleared)
    }

    /// The first use of an invite (the first session or loca socket taken with
    /// it): the holder has proved it has the invite, so its delivery copy
    /// ends and `first_used_at` is stamped, in one write with
    /// `secure_delete` on. By the invite's hash. Returns whether this call
    /// cleared a copy. The WAL clean-up it leaves is owed and paid
    /// best-effort (as for join requests).
    pub fn end_invite_delivery_by_hash(
        &self,
        invite_hash: &str,
        now: u64,
    ) -> rusqlite::Result<bool> {
        let Some(mut c) = self.conn() else {
            return Ok(false);
        };
        let cleared = with_secure_delete(&mut c, |conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let cleared = tx.execute(
                "UPDATE invites SET delivery_token = NULL, first_used_at = ?2 \
                 WHERE secret_hash = ?1 AND delivery_token IS NOT NULL",
                params![invite_hash, now],
            )?;
            if cleared > 0 {
                record_owed_checkpoint(&tx)?;
            }
            tx.commit()?;
            Ok(cleared > 0)
        })?;
        if cleared {
            settle_owed_checkpoint_logged(&c);
        }
        Ok(cleared)
    }

    /// Clear the invite delivery copies whose window has closed at `now`
    /// (`INVITE_DELIVERY_WINDOW_MS` after issue) or whose invite has ended
    /// (only the invite whose hash is `only_hash`, when given: a delivery
    /// attempted too late). Called by the hub's background sweep and by a
    /// Lobby connect that finds a copy past its deadline; the open does the
    /// same. Then retries an owed WAL clean-up, best-effort. Returns how many
    /// copies were cleared.
    pub fn expire_invite_deliveries(
        &self,
        now: u64,
        only_hash: Option<&str>,
    ) -> rusqlite::Result<usize> {
        let Some(mut c) = self.conn() else {
            return Ok(0);
        };
        let cleared = expire_invite_copies(&mut c, now, only_hash)?;
        settle_owed_checkpoint_logged(&c);
        Ok(cleared)
    }

    /// The live invites whose delivery copy is still held, by that copy. Read
    /// once at start; an unreadable table stops the start.
    pub fn load_invite_deliveries(&self) -> rusqlite::Result<Vec<String>> {
        let Some(c) = self.conn() else {
            return Ok(Vec::new());
        };
        let mut statement = c.prepare(
            "SELECT delivery_token FROM invites \
             WHERE revoked_at IS NULL AND delivery_token IS NOT NULL",
        )?;
        let rows = statement.query_map([], |r| r.get(0))?;
        rows.collect()
    }

    /// Test-only: an invite's raw `(delivery_token, first_used_at)`, by the
    /// invite's hash, or `None` when the row is absent.
    #[cfg(test)]
    pub fn invite_delivery_raw_by_hash(
        &self,
        invite_hash: &str,
    ) -> Option<(Option<String>, Option<u64>)> {
        let c = self.conn()?;
        c.query_row(
            "SELECT delivery_token, first_used_at FROM invites WHERE secret_hash = ?1",
            params![invite_hash],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok()
    }

    /// Test-only: whether a truncating WAL checkpoint is still owed after a
    /// key copy was cleared: `Ok(true)` while the WAL may still hold an old copy.
    #[cfg(test)]
    pub fn join_key_checkpoint_owed(&self) -> rusqlite::Result<bool> {
        let Some(c) = self.conn() else {
            return Ok(false);
        };
        checkpoint_owed(&c)
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
        #[cfg(test)]
        if self.conn.is_some() {
            crate::sync::work::SQLITE.with(|taken| taken.set(taken.get() + 1));
        }
        self.conn.as_ref().map(|m| m.lock_or_recover())
    }
}

/// How many live sessions one credential may hold at once: 16. Taking a 17th
/// ends the credential's oldest live session instead of refusing the new one,
/// because the new session is the one somebody is holding right now and a
/// client that retries must never be locked out. Every session is counted
/// under the credential it is filed under (`principal_sessions.credential_id`):
/// a member's browser, Lobby and per-loca invite sessions all come from the
/// member's one key and share one count, and every Master pairing session
/// counts under the root-recovery credential. Sixteen is several devices and
/// browser tabs plus a listener in each of a handful of locas, each holding a
/// session for up to 12 hours; a client that renews on every restart or
/// expiry stays far below it, while a loop taking sessions cannot grow the
/// table past it. Sessions with no credential row (the sandbox's shared
/// `ROOM_TOKEN`, the open house, a memory-only store) are not counted here;
/// they only have their lifetime.
pub const SESSIONS_PER_CREDENTIAL: usize = 16;

/// How long an approved join request's key stays collectable with its request
/// secret: 24 hours, counted from the approval (`join_requests.decided_at`,
/// written in the same transaction as the copy, so a restart neither resets
/// nor extends it).
///
/// The clients collect within seconds when the requester is there: the
/// skill's helper and the browser door both poll every 3 s and collect on the
/// first poll after approval, and the helper acknowledges right after it has
/// verified and stored the key. The window only matters for a requester that
/// stopped waiting before the approval came (an agent session that ended, a
/// helper given `LOCA_JOIN_POLL_TIMEOUT`, a closed tab) and comes back. 24
/// hours lets that happen overnight and is the lifetime a Master's admission
/// rights have by default (`POST /admission-stock` without `ttl_hours`), the
/// same judgement of how long an admission should stay open unattended.
/// After it, a Master gives access again; the membership itself was kept.
pub const JOIN_BOOTSTRAP_WINDOW_MS: u64 = 24 * 60 * 60 * 1000;

/// SQL that is true when a `join_requests` row's delivery deadline has
/// passed at `:now` with the window in `:window`. An approval time that is missing
/// or not positive cannot place a deadline; it is taken as already past
/// rather than as "never", because the copy would otherwise be collectable
/// for ever, which is the failure this limit exists to end. Approval always
/// writes the time, so only a damaged or hand-edited row can lack it.
const PAST_JOIN_BOOTSTRAP_DEADLINE: &str =
    "(COALESCE(decided_at, 0) <= 0 OR :now >= decided_at + :window)";

/// How long an invite's delivery copy is kept: 24 hours, counted from the
/// invite's issue (`invites.issued_at`, written with the copy, so a restart
/// neither resets nor extends it), unless its first use ends it earlier.
/// Inside it, a member that was offline when called gets the invite on its
/// next Lobby connect; after it, the Master re-sends (`POST
/// /rooms/:id/call {"reissue": true}`). The same judgement as
/// `JOIN_BOOTSTRAP_WINDOW_MS`: long enough to come back overnight.
pub const INVITE_DELIVERY_WINDOW_MS: u64 = 24 * 60 * 60 * 1000;

/// SQL that is true when an `invites` row's delivery deadline has passed at
/// `:now` with the window in `:window`. An issue time that is missing or not
/// positive is taken as past, as `PAST_JOIN_BOOTSTRAP_DEADLINE` does.
const PAST_INVITE_DELIVERY_DEADLINE: &str =
    "(COALESCE(issued_at, 0) <= 0 OR :now >= issued_at + :window)";

/// What a requester's poll may show about its own request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinRequestView {
    pub status: String,
    pub name: String,
    /// The key can be collected now.
    pub bootstrap_ready: bool,
    /// Approved, never acknowledged, and the delivery window has closed: the
    /// key can no longer be collected and a Master must give access again.
    pub bootstrap_expired: bool,
}

/// The answer to a collection attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootstrapClaim {
    Delivered(String),
    /// Approved, never acknowledged, and past the delivery deadline. The copy
    /// is gone; the membership is not.
    Expired,
    /// Unknown id, wrong secret, not approved, or already acknowledged.
    Unavailable,
}

/// Outcome of atomically approving a join request (single transaction).
#[derive(Debug)]
pub enum ApproveTxn {
    /// Stock consumed, Lobby member inserted, request approved. Carries the new
    /// membership's record (never its key) so the caller can mirror it into
    /// the in-memory roster.
    Committed(MemberRecord),
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

pub(crate) fn secret_hash(secret: &str) -> String {
    hex_bytes(&Sha256::digest(secret.as_bytes()))
}

pub(crate) fn hex_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

/// Add one column to an existing table while the store opens. A database that
/// already has the column answers "duplicate column name", and only that
/// answer counts as success. Every other failure (a missing table, a locked or
/// read-only file, a full disk) is returned, so `Store::open` stops with the
/// real error instead of serving a half-upgraded schema.
fn add_column(conn: &Connection, table: &str, column_definition: &str) -> rusqlite::Result<()> {
    let sql = format!("ALTER TABLE {table} ADD COLUMN {column_definition}");
    match conn.execute(&sql, []) {
        Ok(_) => Ok(()),
        Err(error) if is_duplicate_column(&error) => Ok(()),
        Err(error) => Err(error),
    }
}

fn is_duplicate_column(error: &rusqlite::Error) -> bool {
    let message = match error {
        rusqlite::Error::SqliteFailure(_, Some(message)) => message,
        rusqlite::Error::SqlInputError { msg, .. } => msg,
        _ => return false,
    };
    message.starts_with("duplicate column name")
}

/// `messages.sender_principal` names the verified principal that wrote a
/// message, so a rename relabels that person's words and nobody else's. It is
/// added once; the same transaction attributes the only older rows whose
/// writer is written out in full: a Smaster's socket posts, whose operation
/// scope is `sm:pr:<principal id>`, when that principal exists. Every other
/// older row stays unattributed and keeps its label; no row is ever claimed
/// by its name, and the secret-bearing `mb:`/`dv:` scopes are not read.
fn attribute_messages_once(conn: &mut Connection) -> rusqlite::Result<()> {
    let has_column = {
        let mut statement = conn.prepare("PRAGMA table_info(messages)")?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        columns.iter().any(|column| column == "sender_principal")
    };
    if has_column {
        return Ok(());
    }
    let tx = conn.transaction()?;
    add_column(&tx, "messages", "sender_principal TEXT")?;
    let attributed = tx.execute(
        "UPDATE messages SET sender_principal = substr(principal, 7)
         WHERE substr(principal, 1, 6) = 'sm:pr:'
           AND substr(principal, 7) IN (SELECT id FROM principals)",
        [],
    )?;
    tx.commit()?;
    if attributed > 0 {
        tracing::info!(
            attributed,
            "attributed older Smaster messages to their principal"
        );
    }
    Ok(())
}

/// The Smaster registry's shape. `target` is the table name, optionally
/// preceded by `IF NOT EXISTS`. The code a Smaster signs in with is not a
/// column: only its `secret_hash` (the same hash the `credentials` table keeps)
/// and the identifiers that were derived from the code once, when it was
/// minted or when an older registry was migrated. They never change, so a
/// Smaster keeps its principal, its management id and its credential.
/// `fingerprint` is exactly what the key ledger shows (the prefix and four
/// characters, see `keys::fingerprint`), never more of the code.
pub(crate) fn smasters_table_sql(target: &str) -> String {
    format!(
        "CREATE TABLE {target} (
            management_id TEXT PRIMARY KEY,
            secret_hash TEXT NOT NULL UNIQUE,
            principal_id TEXT NOT NULL UNIQUE REFERENCES principals(id),
            credential_id TEXT NOT NULL UNIQUE REFERENCES credentials(id),
            fingerprint TEXT NOT NULL,
            name TEXT NOT NULL,
            issued_at INTEGER NOT NULL,
            revoked_at INTEGER
        );"
    )
}

/// The shape of the persisted browser admin sessions (Master and Smaster).
/// A deploy must not turn a signed-in administrator into a stranger, so these
/// sessions outlive a restart until they expire; invite sessions stay in
/// memory only. `target` is as for `smasters_table_sql`. The session token is
/// not a column: `secret_hash` is its hash, the same value
/// `principal_sessions.id` holds for that session, and a presented token is
/// hashed and looked up. The hash itself is not a token.
pub(crate) fn admin_sessions_table_sql(target: &str) -> String {
    format!(
        "CREATE TABLE {target} (
            secret_hash TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            kind TEXT NOT NULL,
            expires_at INTEGER NOT NULL
        );"
    )
}

/// Test-only: replace a current database's `members`, `invites` and
/// `member_suspensions` with the tables v0.13.0 wrote (each keyed by the key
/// or the invite itself, and `invites` without the delivery columns later
/// builds added), empty, so a test can write rows the way that version did.
#[cfg(test)]
pub(crate) const LEGACY_MEMBER_TABLES: &str = "
    DROP INDEX IF EXISTS invites_member;
    DROP TABLE member_suspensions;
    DROP TABLE invites;
    DROP TABLE members;
    CREATE TABLE invites (
        token TEXT PRIMARY KEY,
        room TEXT NOT NULL,
        name TEXT NOT NULL,
        kind TEXT NOT NULL,
        issued_at INTEGER NOT NULL,
        issued_by TEXT NOT NULL,
        revoked_at INTEGER,
        member TEXT NOT NULL DEFAULT ''
    );
    CREATE INDEX invites_room ON invites(room);
    CREATE TABLE members (
        token TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        kind TEXT NOT NULL,
        joined_at INTEGER NOT NULL,
        admitted_by TEXT NOT NULL,
        revoked_at INTEGER
    );
    CREATE TABLE member_suspensions (
        member_token TEXT PRIMARY KEY REFERENCES members(token),
        suspended_at INTEGER NOT NULL,
        reason TEXT NOT NULL
    );
";

/// The shape of `members`. `target` is as for `smasters_table_sql`. The
/// membership key is not a column: only its `secret_hash` (the same hash its
/// credential keeps) and the identifiers derived from it once, when it was
/// minted or when an older table was migrated (`MemberIds`). They never
/// change, so a member keeps its management id, principal and credential.
/// `fingerprint` is exactly what the key ledger shows (`keys::fingerprint`).
pub(crate) fn members_table_sql(target: &str) -> String {
    format!(
        "CREATE TABLE {target} (
            id TEXT PRIMARY KEY,
            secret_hash TEXT NOT NULL UNIQUE,
            principal_id TEXT NOT NULL UNIQUE REFERENCES principals(id),
            credential_id TEXT NOT NULL UNIQUE REFERENCES credentials(id),
            fingerprint TEXT NOT NULL,
            name TEXT NOT NULL,
            kind TEXT NOT NULL,
            joined_at INTEGER NOT NULL,
            admitted_by TEXT NOT NULL,
            revoked_at INTEGER
        );"
    )
}

/// The shape of `invites` (invites). The invite is not a column: its
/// `secret_hash`, its management id (`ivid_`) and its fingerprint are.
/// `member_id` is the membership it seats (`mbid_`), empty for an invite from
/// before invites named one. `delivery_token` is the bounded clear-text copy an
/// invite minted for delivery keeps until its first use (`first_used_at`) or
/// `INVITE_DELIVERY_WINDOW_MS` after issue, the only place an invite is stored
/// in clear text.
pub(crate) fn invites_table_sql(target: &str) -> String {
    format!(
        "CREATE TABLE {target} (
            id TEXT PRIMARY KEY,
            secret_hash TEXT NOT NULL UNIQUE,
            fingerprint TEXT NOT NULL,
            room TEXT NOT NULL,
            member_id TEXT NOT NULL DEFAULT '',
            name TEXT NOT NULL,
            kind TEXT NOT NULL,
            issued_at INTEGER NOT NULL,
            issued_by TEXT NOT NULL,
            revoked_at INTEGER,
            delivery_token TEXT,
            first_used_at INTEGER
        );"
    )
}

/// The shape of `member_suspensions`: a suspension names its membership by
/// id. Suspension is reversible and distinct from membership revocation.
pub(crate) fn member_suspensions_table_sql(target: &str) -> String {
    format!(
        "CREATE TABLE {target} (
            member_id TEXT PRIMARY KEY REFERENCES members(id),
            suspended_at INTEGER NOT NULL,
            reason TEXT NOT NULL
        );"
    )
}

/// Storage chores kept in the database so a chore interrupted by a crash or
/// a refusal runs again instead of being forgotten. The `SECRET_SCRUBS` must
/// finish before the store serves; `CHECKPOINT_JOIN_KEYS` is done whenever
/// the WAL can be emptied, while serving.
const STORAGE_MAINTENANCE_SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS storage_maintenance (
        task TEXT PRIMARY KEY,
        since INTEGER NOT NULL
    );
";

/// The chore `migrate_smaster_codes` leaves behind: remove the clear-text
/// codes the rebuilt table left in free pages and in the WAL.
const SCRUB_SMASTER_CODES: &str = "scrub_smaster_codes";

/// The same chore for `migrate_admin_session_tokens`: remove the clear-text
/// session tokens.
const SCRUB_ADMIN_SESSION_TOKENS: &str = "scrub_admin_session_tokens";

/// The same chore for `migrate_member_keys`: remove the clear-text
/// membership keys and invites.
const SCRUB_MEMBER_KEYS: &str = "scrub_member_keys";

/// Every chore `scrub_secret_residue` finishes. They are one job (rewrite the
/// file, empty the WAL), so one run clears all that are recorded.
const SECRET_SCRUBS: [&str; 3] = [
    SCRUB_SMASTER_CODES,
    SCRUB_ADMIN_SESSION_TOKENS,
    SCRUB_MEMBER_KEYS,
];

/// Record, inside the migration's own transaction, that the file still holds
/// the secrets it removed from the tables, so the clean-up runs even if this
/// open stops before it.
fn record_secret_scrub(tx: &rusqlite::Transaction<'_>, task: &str) -> rusqlite::Result<()> {
    debug_assert!(SECRET_SCRUBS.contains(&task));
    tx.execute(
        "INSERT OR REPLACE INTO storage_maintenance (task, since) VALUES (?1, ?2)",
        params![task, current_time_ms()],
    )
    .map(|_| ())
}

/// The chore an ACK or an expiry leaves when it clears a key delivery copy
/// (a join request's or a membership claim's): empty the WAL, whose earlier
/// frames (the approval or the claim, each collection) still hold the row
/// with the key. Clearing an invite's delivery copy (at its first use or its
/// deadline) owes the same chore under the same name. The task keeps its
/// first name, which databases already hold. Unlike `SECRET_SCRUBS` it runs
/// while the server serves, so it never runs `VACUUM` and never fails
/// anything: a busy checkpoint leaves it recorded and the next ACK, expiry,
/// sweep (every 15 seconds) or open tries again.
const CHECKPOINT_JOIN_KEYS: &str = "checkpoint_join_bootstrap_keys";

/// Which key delivery copies `expire_delivery_copies` clears.
#[derive(Clone, Copy)]
enum DeliveryCopies<'a> {
    /// Every copy whose deadline has passed, in both tables (the open and the
    /// sweep).
    PastDeadline,
    /// This join request's copy, if its deadline has passed (a late
    /// collection).
    JoinRequest(&'a str),
    /// This membership claim's copy, `(invite id, claim hash)`: the caller has
    /// found that its delivery is over (a late collection, or a key that was
    /// revoked before it was collected).
    Claim(&'a str, &'a str),
}

/// Clear key delivery copies and stamp when each was cleared: a join
/// request's `mb_token` (`bootstrap_expired_at`, for an approved,
/// unacknowledged request past `JOIN_BOOTSTRAP_WINDOW_MS`) and a membership
/// claim's `delivery_token` (`expired_at`, unacknowledged, past
/// `CLAIM_DELIVERY_WINDOW_MS`). `which` narrows it to one copy. One
/// transaction with `secure_delete` on, so the rows' old images are zeroed in
/// their pages, and the WAL clean-up that remains is recorded with it.
/// Returns how many copies were cleared.
///
/// This ends the delivery copy only. The membership the approval created,
/// and the credential a claim minted, are not revoked and their keys keep
/// working: the requester may well have collected the key in time and only
/// its ACK was lost. Neither key is stored anywhere else in clear text: the
/// member tables keep only its hash and ids.
fn expire_delivery_copies(
    conn: &mut Connection,
    now: u64,
    which: DeliveryCopies<'_>,
) -> rusqlite::Result<usize> {
    with_secure_delete(conn, |conn| {
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut cleared = 0;
        let join_id = match which {
            DeliveryCopies::PastDeadline => Some(None),
            DeliveryCopies::JoinRequest(id) => Some(Some(id)),
            DeliveryCopies::Claim(..) => None,
        };
        if let Some(only_id) = join_id {
            cleared += tx.execute(
                &format!(
                    "UPDATE join_requests SET mb_token = NULL, bootstrap_expired_at = :now \
                     WHERE status = 'approved' AND mb_token IS NOT NULL \
                       AND bootstrap_acked_at IS NULL AND {PAST_JOIN_BOOTSTRAP_DEADLINE} \
                       AND (:id IS NULL OR id = :id)"
                ),
                rusqlite::named_params! {
                    ":id": only_id,
                    ":now": now,
                    ":window": JOIN_BOOTSTRAP_WINDOW_MS,
                },
            )?;
        }
        match which {
            DeliveryCopies::PastDeadline => {
                cleared += tx.execute(
                    &format!(
                        "UPDATE membership_claims SET delivery_token = NULL, expired_at = :now \
                         WHERE delivery_token IS NOT NULL AND acked_at IS NULL \
                           AND {PAST_CLAIM_DELIVERY_DEADLINE}"
                    ),
                    rusqlite::named_params! {
                        ":now": now,
                        ":window": CLAIM_DELIVERY_WINDOW_MS,
                    },
                )?;
            }
            DeliveryCopies::Claim(invite_id, claim_hash) => {
                cleared += tx.execute(
                    "UPDATE membership_claims SET delivery_token = NULL, expired_at = :now \
                     WHERE delivery_token IS NOT NULL AND acked_at IS NULL \
                       AND invite_id = :invite AND claim_hash = :claim",
                    rusqlite::named_params! {
                        ":now": now,
                        ":invite": invite_id,
                        ":claim": claim_hash,
                    },
                )?;
            }
            DeliveryCopies::JoinRequest(_) => {}
        }
        if cleared > 0 {
            record_owed_checkpoint(&tx)?;
        }
        tx.commit()?;
        Ok(cleared)
    })
}

/// Clear the delivery copy of every invite whose delivery deadline has passed
/// at `now` or that has ended (only the one whose hash is `only_hash`, when
/// given). The counterpart of `expire_join_bootstraps` for
/// `invites.delivery_token`: one transaction with `secure_delete` on, and the
/// WAL clean-up recorded with it under the same chore. Returns how many
/// copies were cleared.
///
/// This ends the delivery copy only; the invite itself stays live, and after
/// this the server holds only its hash.
fn expire_invite_copies(
    conn: &mut Connection,
    now: u64,
    only_hash: Option<&str>,
) -> rusqlite::Result<usize> {
    with_secure_delete(conn, |conn| {
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let cleared = tx.execute(
            &format!(
                "UPDATE invites SET delivery_token = NULL \
                 WHERE delivery_token IS NOT NULL \
                   AND (revoked_at IS NOT NULL OR {PAST_INVITE_DELIVERY_DEADLINE}) \
                   AND (:hash IS NULL OR secret_hash = :hash)"
            ),
            rusqlite::named_params! {
                ":hash": only_hash,
                ":now": now,
                ":window": INVITE_DELIVERY_WINDOW_MS,
            },
        )?;
        if cleared > 0 {
            record_owed_checkpoint(&tx)?;
        }
        tx.commit()?;
        Ok(cleared)
    })
}

/// Add `invites.delivery_token` and `invites.first_used_at`. On the open
/// that adds them (the upgrade), every live invite issued less than
/// `INVITE_DELIVERY_WINDOW_MS` before `now` gets its delivery copy, so a
/// member called while offline just before the upgrade is still sent it, as
/// the previous version would have; older invites get none. One transaction,
/// so an interrupted upgrade leaves neither the columns nor the copies.
fn add_invite_delivery_columns(conn: &mut Connection, now: u64) -> rusqlite::Result<()> {
    if table_has_column(conn, "invites", "delivery_token")? {
        return add_column(conn, "invites", "first_used_at INTEGER");
    }
    let tx = conn.transaction()?;
    add_column(&tx, "invites", "delivery_token TEXT")?;
    add_column(&tx, "invites", "first_used_at INTEGER")?;
    tx.execute(
        &format!(
            "UPDATE invites SET delivery_token = token \
             WHERE revoked_at IS NULL AND NOT {PAST_INVITE_DELIVERY_DEADLINE}"
        ),
        rusqlite::named_params! {
            ":now": now,
            ":window": INVITE_DELIVERY_WINDOW_MS,
        },
    )?;
    tx.commit()
}

/// The columns that remember who sent, reacted or uploaded by a scope key,
/// which earlier versions built from a membership key (`mb:<key>`) or an
/// invite (`dv:<invite>`).
const KEY_SCOPED_COLUMNS: [(&str, &str); 3] = [
    ("messages", "principal"),
    ("message_reactions", "principal"),
    ("attachment_pending", "uploader"),
];

/// Rewrite every `KEY_SCOPED_COLUMNS` value that names a membership key or an
/// invite to the id form (`scope_key_by_id`), so the database keeps no key in
/// them and a retry, an un-reaction or an upload of the same member after
/// the upgrade finds what it stored before: the hub names that member
/// `mb:<mbid_>` now (`Hub::operation_scope`, its REST `legacy` key
/// included). One IMMEDIATE transaction with `secure_delete` on, so the old
/// cells are zeroed in their pages and either every row is rewritten or, on
/// any error, none is and `Store::open` fails with it. The WAL frames that
/// still hold the old rows are the owed checkpoint chore
/// (`record_owed_checkpoint`), as for a cleared delivery copy. Runs on every
/// open and changes nothing once nothing is left, so it also takes in rows
/// an older build wrote after a rollback. Returns how many rows changed.
///
/// The mapping is one-to-one, so a rewritten key can meet an existing one
/// only where an older build stored the same thing twice under the two
/// forms (after a rollback): the same member's same reaction, which is kept
/// once; or the same operation, whose second message is kept but no longer
/// answers that `op_id` (the first does).
fn rewrite_key_scopes(conn: &mut Connection) -> rusqlite::Result<usize> {
    with_secure_delete(conn, |conn| {
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut rewritten = 0;
        for (table, column) in KEY_SCOPED_COLUMNS {
            let named_by_key = format!(
                "substr({column}, 1, 3) IN ('mb:', 'dv:') AND loca_scope_by_id({column}) <> {column}"
            );
            rewritten += tx.execute(
                &format!(
                    "UPDATE OR IGNORE {table} SET {column} = loca_scope_by_id({column}) \
                     WHERE {named_by_key}"
                ),
                [],
            )?;
            // What is left met its id form already (see above); only these
            // two tables have a key the scope is part of.
            rewritten += match table {
                "messages" => tx.execute(
                    &format!(
                        "UPDATE messages SET principal = NULL, op_id = NULL WHERE {named_by_key}"
                    ),
                    [],
                )?,
                "message_reactions" => tx.execute(
                    &format!("DELETE FROM message_reactions WHERE {named_by_key}"),
                    [],
                )?,
                _ => 0,
            };
        }
        if rewritten > 0 {
            record_owed_checkpoint(&tx)?;
        }
        tx.commit()?;
        Ok(rewritten)
    })
}

/// Record, in the transaction that clears a key copy, that the WAL still owes
/// a truncating checkpoint.
fn record_owed_checkpoint(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT OR IGNORE INTO storage_maintenance (task, since) VALUES (?1, ?2)",
        params![CHECKPOINT_JOIN_KEYS, current_time_ms()],
    )
    .map(|_| ())
}

fn checkpoint_owed(conn: &Connection) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM storage_maintenance WHERE task = ?1)",
        params![CHECKPOINT_JOIN_KEYS],
        |r| r.get(0),
    )
}

/// Pay the owed WAL clean-up if it is owed: `wal_checkpoint(TRUNCATE)` copies
/// the current pages into the database file and cuts the WAL to zero bytes,
/// and only a checkpoint that did all of that removes the record. It does
/// not wait: another connection reading the file (a backup, `sqlite3` on the
/// live file) makes it answer busy at once, and the record stays for the next
/// try. Returns whether nothing is owed any more.
fn settle_owed_checkpoint(conn: &Connection) -> rusqlite::Result<bool> {
    if !checkpoint_owed(conn)? {
        return Ok(true);
    }
    conn.busy_timeout(std::time::Duration::ZERO)?;
    let checkpoint = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
        ))
    });
    conn.busy_timeout(STORE_BUSY_TIMEOUT)?;
    let (busy, log, checkpointed) = match checkpoint {
        Ok(counts) => counts,
        Err(rusqlite::Error::SqliteFailure(error, _))
            if matches!(
                error.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            ) =>
        {
            return Ok(false)
        }
        Err(error) => return Err(error),
    };
    if busy != 0 || log != checkpointed {
        return Ok(false);
    }
    conn.execute(
        "DELETE FROM storage_maintenance WHERE task = ?1",
        params![CHECKPOINT_JOIN_KEYS],
    )?;
    Ok(true)
}

/// `settle_owed_checkpoint` for the paths that must not fail because of it
/// (a collection, an ACK, the sweep, the open): the outcome is logged, and an
/// error is never read as "done".
fn settle_owed_checkpoint_logged(conn: &Connection) {
    match settle_owed_checkpoint(conn) {
        Ok(true) => {}
        Ok(false) => tracing::warn!(
            "the database is busy, so old key or invite delivery copies stay in its write-ahead log until a later checkpoint"
        ),
        Err(error) => tracing::error!(
            %error,
            "could not empty the write-ahead log after clearing a key or invite delivery copy; it stays owed"
        ),
    }
}

/// The wait rusqlite gives every connection it opens, restored after a
/// checkpoint that must not wait.
const STORE_BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Run a rebuild with `secure_delete` on, so the pages it frees (the old
/// table and its primary-key index) are zeroed and hold no secret even before
/// `scrub_secret_residue` runs. Restored afterwards: this connection's
/// ordinary deletes keep SQLite's default cost.
fn with_secure_delete<T>(
    conn: &mut Connection,
    rebuild: impl FnOnce(&mut Connection) -> rusqlite::Result<T>,
) -> rusqlite::Result<T> {
    let secure_delete: i64 = conn.query_row("PRAGMA secure_delete", [], |row| row.get(0))?;
    conn.execute_batch("PRAGMA secure_delete = ON")?;
    let rebuilt = rebuild(conn);
    let restore = match secure_delete {
        0 => "PRAGMA secure_delete = OFF",
        2 => "PRAGMA secure_delete = FAST",
        _ => "PRAGMA secure_delete = ON",
    };
    let restored = conn.execute_batch(restore);
    let value = rebuilt?;
    restored?;
    Ok(value)
}

/// Run a rewrite of rows that hold secrets with SQLite's temporary files
/// (a statement journal, which keeps the original image of every page the
/// statement changes) in memory: in a file of their own, the old keys would
/// be written to disk again, outside the database, where no clean-up
/// reaches. Restored afterwards. `VACUUM` is not run under this: it copies
/// the whole database, which has no secret left by then, and in memory it
/// would need as much RAM as the database is large.
fn with_temp_store_in_memory<T>(
    conn: &mut Connection,
    rewrite: impl FnOnce(&mut Connection) -> rusqlite::Result<T>,
) -> rusqlite::Result<T> {
    let temp_store: i64 = conn.query_row("PRAGMA temp_store", [], |row| row.get(0))?;
    conn.execute_batch("PRAGMA temp_store = MEMORY")?;
    let rewritten = rewrite(conn);
    let restored = conn.execute_batch(&format!("PRAGMA temp_store = {temp_store}"));
    let value = rewritten?;
    restored?;
    Ok(value)
}

fn table_has_column(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let mut statement = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(columns.iter().any(|name| name == column))
}

/// Rebuild a Smaster registry written by v0.13.0 or earlier, whose primary
/// key is the code itself, into `smasters_table_sql`'s shape. One
/// transaction: every row's identifiers are computed from its code with the
/// derivations v0.13.0 used, its principal and legacy credential are bound
/// (as `migrate_legacy_principals` used to do on every open), the new table
/// is filled, and the old one is dropped. Any failure rolls all of it back,
/// leaving the old table as it was, and `Store::open` fails with that error.
/// Returns whether a migration ran.
///
/// This is one-way. v0.13.0 reads `smasters.token` on every open, so it
/// cannot open a database after this ran; going back means restoring the
/// snapshot taken before the upgrade (docs/backup-restore.md).
fn migrate_smaster_codes(conn: &mut Connection) -> rusqlite::Result<bool> {
    if !table_has_column(conn, "smasters", "token")? {
        return Ok(false);
    }
    let count = with_secure_delete(conn, rebuild_smaster_registry)?;
    tracing::warn!(
        smasters = count,
        "Smaster codes are no longer stored; this database can no longer be opened by v0.13.0 or earlier"
    );
    Ok(true)
}

fn rebuild_smaster_registry(conn: &mut Connection) -> rusqlite::Result<usize> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let rows = {
        let mut statement =
            tx.prepare("SELECT token, name, issued_at, revoked_at FROM smasters")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u64>(2)?,
                row.get::<_, Option<u64>>(3)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    tx.execute_batch(&smasters_table_sql("smasters_hashed"))?;
    for (code, name, issued_at, revoked_at) in &rows {
        let ids = identity::SmasterIds::from_code(code);
        // The same bindings, with the same identifiers, that v0.13.0 made on
        // every open. Existing rows are kept as they are (a renamed display
        // name, a disabled principal, a revoked credential, its last use).
        tx.execute(
            "INSERT OR IGNORE INTO principals
             (id, display_name, kind, building_role, created_at, disabled_at)
             VALUES (?1, ?2, 'human', 'smaster', ?3, ?4)",
            params![ids.principal_id, name, issued_at, revoked_at],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO credentials
             (id, principal_id, label, secret_hash, created_at, last_used_at, revoked_at, legacy_source)
             VALUES (?1, ?2, 'Legacy Smaster access', ?3, ?4, NULL, ?5, ?6)",
            params![
                ids.credential_id,
                ids.principal_id,
                ids.secret_hash,
                issued_at,
                revoked_at,
                format!("smaster:{}", ids.secret_hash)
            ],
        )?;
        tx.execute(
            "INSERT INTO smasters_hashed
             (management_id, secret_hash, principal_id, credential_id, fingerprint,
              name, issued_at, revoked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                ids.management_id,
                ids.secret_hash,
                ids.principal_id,
                ids.credential_id,
                ids.fingerprint,
                name,
                issued_at,
                revoked_at
            ],
        )?;
    }
    record_secret_scrub(&tx, SCRUB_SMASTER_CODES)?;
    tx.execute_batch(
        "DROP TABLE smasters;
         ALTER TABLE smasters_hashed RENAME TO smasters;",
    )?;
    tx.commit()?;
    Ok(rows.len())
}

/// Rebuild an `admin_sessions` table written before this version, whose
/// primary key is the session token itself, into `admin_sessions_table_sql`'s
/// shape. Each token is replaced by its `secret_hash`, which is also the
/// `principal_sessions.id` of the same session, so whoever was signed in stays
/// signed in, as the same principal, until the same expiry. One transaction:
/// any failure rolls it back, leaving the old table as it was, and
/// `Store::open` fails with that error. Expired rows are carried over and
/// removed by `load_admin_sessions`, as before. Returns whether a migration
/// ran.
///
/// This is one-way. Older versions read and write `admin_sessions.token`;
/// the table has no such column after this ran (docs/backup-restore.md).
fn migrate_admin_session_tokens(conn: &mut Connection) -> rusqlite::Result<bool> {
    if !table_has_column(conn, "admin_sessions", "token")? {
        return Ok(false);
    }
    let count = with_secure_delete(conn, rebuild_admin_sessions)?;
    tracing::warn!(
        sessions = count,
        "admin session tokens are no longer stored; older versions can no longer use this database's sessions"
    );
    Ok(true)
}

fn rebuild_admin_sessions(conn: &mut Connection) -> rusqlite::Result<usize> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let rows = {
        let mut statement =
            tx.prepare("SELECT token, name, kind, expires_at FROM admin_sessions")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, u64>(3)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    tx.execute_batch(&admin_sessions_table_sql("admin_sessions_hashed"))?;
    for (token, name, kind, expires_at) in &rows {
        tx.execute(
            "INSERT INTO admin_sessions_hashed (secret_hash, name, kind, expires_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![secret_hash(token), name, kind, expires_at],
        )?;
    }
    record_secret_scrub(&tx, SCRUB_ADMIN_SESSION_TOKENS)?;
    tx.execute_batch(
        "DROP TABLE admin_sessions;
         ALTER TABLE admin_sessions_hashed RENAME TO admin_sessions;",
    )?;
    tx.commit()?;
    Ok(rows.len())
}

/// Rebuild `members`, `invites` and `member_suspensions` written before this
/// version, keyed by the membership key and the invite themselves, into
/// `members_table_sql`'s, `invites_table_sql`'s and
/// `member_suspensions_table_sql`'s shapes. One IMMEDIATE transaction with
/// `secure_delete` on: every member's identifiers are computed from its key
/// with the derivations v0.13.0 used, its principal and legacy credential
/// are bound when they are not yet (as `migrate_legacy_principals` used to
/// do on every open), every invite's from the invite, and its membership is
/// named by that membership's id; an invite's delivery copy and first use are
/// carried over exactly as they were (the copy is still cleared at its use or
/// its deadline). Then the old tables are dropped and the new ones take
/// their names. Any failure rolls all of it back, leaving the old tables as
/// they were, and `Store::open` fails with that error. Returns whether a
/// migration ran.
///
/// A database written before memberships or suspensions existed has only
/// some of the older tables; the open has just created the others, empty, in
/// the new shape. Each table is read in the shape it has, and all three are
/// rebuilt: any one of them in the older shape starts the migration.
///
/// This is one-way. Earlier builds read `members.token` on every open, so
/// they cannot open a database after this ran; going back means restoring the
/// snapshot taken before the upgrade (docs/backup-restore.md).
fn migrate_member_keys(conn: &mut Connection) -> rusqlite::Result<bool> {
    let older = MemberTableShapes {
        members: table_has_column(conn, "members", "token")?,
        invites: table_has_column(conn, "invites", "token")?,
        suspensions: table_has_column(conn, "member_suspensions", "member_token")?,
    };
    if !(older.members || older.invites || older.suspensions) {
        return Ok(false);
    }
    let (members, invites) = with_temp_store_in_memory(conn, |conn| {
        with_secure_delete(conn, |conn| rebuild_member_tables(conn, older))
    })?;
    tracing::warn!(
        members,
        invites,
        "membership keys and invites are no longer stored; earlier builds can no longer open this database"
    );
    Ok(true)
}

/// One `members` row as an earlier build wrote it.
type LegacyMemberRow = (String, String, String, u64, String, Option<u64>);
/// One `invites` row as an earlier build wrote it: the invite, room, member
/// key, name, kind, issue, issuer, revocation, delivery copy and first use.
type LegacyInviteRow = (
    String,
    String,
    String,
    String,
    String,
    i64,
    String,
    Option<i64>,
    Option<String>,
    Option<i64>,
);

/// Which of the three tables are in the shape earlier builds wrote. A table
/// that is not was created, empty, by this open.
#[derive(Clone, Copy)]
struct MemberTableShapes {
    members: bool,
    invites: bool,
    suspensions: bool,
}

fn rebuild_member_tables(
    conn: &mut Connection,
    older: MemberTableShapes,
) -> rusqlite::Result<(usize, usize)> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let members: Vec<LegacyMemberRow> = if !older.members {
        Vec::new()
    } else {
        let mut statement = tx
            .prepare("SELECT token, name, kind, joined_at, admitted_by, revoked_at FROM members")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let invites: Vec<LegacyInviteRow> = if !older.invites {
        Vec::new()
    } else {
        let mut statement = tx.prepare(
            "SELECT token, room, member, name, kind, issued_at, issued_by, revoked_at,
                    delivery_token, first_used_at
             FROM invites",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let suspensions: Vec<(String, u64, String)> = if !older.suspensions {
        Vec::new()
    } else {
        let mut statement =
            tx.prepare("SELECT member_token, suspended_at, reason FROM member_suspensions")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    tx.execute_batch(&members_table_sql("members_hashed"))?;
    for (key, name, kind, joined_at, admitted_by, revoked_at) in &members {
        let ids = identity::MemberIds::from_key(key);
        let member = MemberRecord {
            id: ids.id.clone(),
            secret_hash: ids.secret_hash.clone(),
            principal_id: None,
            name: name.clone(),
            kind: kind.clone(),
            joined_at: *joined_at,
            admitted_by: admitted_by.clone(),
        };
        // The same bindings, with the same identifiers and label, that
        // every earlier open made (`migrate_legacy_principals`), revoked or
        // disabled with the membership. Existing bindings are kept as they
        // are (renamed, disabled, revoked, their last use).
        identity::insert_member_record(
            &tx,
            "members_hashed",
            &ids,
            &member,
            *revoked_at,
            "Legacy member access",
        )?;
    }
    record_secret_scrub(&tx, SCRUB_MEMBER_KEYS)?;
    tx.execute_batch(
        "DROP TABLE member_suspensions;
         DROP TABLE invites;
         DROP TABLE members;
         ALTER TABLE members_hashed RENAME TO members;",
    )?;
    // Created after the rename, so the suspensions' foreign key names the
    // new `members`.
    tx.execute_batch(&invites_table_sql("invites"))?;
    tx.execute_batch(&member_suspensions_table_sql("member_suspensions"))?;
    tx.execute_batch(
        "CREATE INDEX invites_room ON invites(room);
         CREATE INDEX invites_member ON invites(member_id);",
    )?;
    let member_id = |key: &str| {
        if key.is_empty() {
            String::new()
        } else {
            identity::MemberIds::from_key(key).id
        }
    };
    for (invite, room, member, name, kind, issued_at, issued_by, revoked_at, copy, first_used) in
        &invites
    {
        let ids = identity::InviteIds::from_invite(invite);
        tx.execute(
            "INSERT INTO invites
             (id, secret_hash, fingerprint, room, member_id, name, kind, issued_at, issued_by,
              revoked_at, delivery_token, first_used_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                ids.id,
                ids.secret_hash,
                ids.fingerprint,
                room,
                member_id(member),
                name,
                kind,
                issued_at,
                issued_by,
                revoked_at,
                copy,
                first_used
            ],
        )?;
    }
    for (member, suspended_at, reason) in &suspensions {
        tx.execute(
            "INSERT INTO member_suspensions (member_id, suspended_at, reason)
             VALUES (?1, ?2, ?3)",
            params![member_id(member), suspended_at, reason],
        )?;
    }
    tx.commit()?;
    Ok((members.len(), invites.len()))
}

/// Remove what the migrations above left of the old secrets in the file.
///
/// Dropping a table does not erase it: its pages go to the free list, and
/// earlier updates (`INSERT OR REPLACE`, renames, revocations, expired
/// sessions deleted at start) left old copies of rows in pages that were
/// freed long before `secure_delete` was turned on. Earlier writes of those
/// pages can also still sit in the WAL, which SQLite reuses but does not
/// shrink or wipe. So, once a migration has committed: `VACUUM` rewrites the
/// database into fresh pages with no free list and no deleted cells, and
/// `wal_checkpoint(TRUNCATE)` copies that into the main file and cuts the WAL
/// to zero bytes. `secure_delete` alone would miss the pages freed before it
/// was on; `VACUUM` alone would leave the old frames in the WAL until the
/// checkpoint. A checkpoint that answers busy (another connection is reading
/// this file) has not done this, so it is an error: the open fails and the
/// chores stay recorded, to run again on the next open. Runs only while one
/// of `SECRET_SCRUBS` is recorded, that is, after a migration, never on an
/// ordinary open; one run finishes all of them, so a database that needed
/// several migrations is rewritten once.
fn scrub_secret_residue(conn: &Connection) -> rusqlite::Result<()> {
    let placeholders = vec!["?"; SECRET_SCRUBS.len()].join(", ");
    let pending: Vec<String> = {
        let mut statement = conn.prepare(&format!(
            "SELECT task FROM storage_maintenance WHERE task IN ({placeholders}) ORDER BY task"
        ))?;
        let rows =
            statement.query_map(rusqlite::params_from_iter(SECRET_SCRUBS), |row| row.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    if pending.is_empty() {
        return Ok(());
    }
    let tasks = pending.join(", ");
    conn.execute_batch("VACUUM")?;
    let (busy, log, checkpointed): (i64, i64, i64) =
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    if busy != 0 || log != checkpointed {
        return Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
            Some(format!(
                "another connection holds the database, so the old secrets ({tasks}) could not be \
                 removed from its write-ahead log (checkpoint busy={busy}, log={log}, \
                 checkpointed={checkpointed}); stop every other process using it and start again"
            )),
        ));
    }
    conn.execute(
        &format!("DELETE FROM storage_maintenance WHERE task IN ({placeholders})"),
        rusqlite::params_from_iter(SECRET_SCRUBS),
    )?;
    tracing::info!(%tasks, "removed the old secrets' residue from the database file");
    Ok(())
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
