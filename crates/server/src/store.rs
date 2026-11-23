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
mod building_settings;
mod dm;
pub(crate) use attachment_index::{AttachError, BlobServe};
use claims::PAST_CLAIM_DELIVERY_DEADLINE;
pub use claims::{MemberClaim, CLAIM_DELIVERY_WINDOW_MS, MEMBERSHIP_CLAIMS_PER_INVITE};
pub use dm::Dm;
pub use groups::{GroupLogEntry, GroupRecord};
pub use identity::{AdminSessionRecord, InviteRecord, MemberRecord, SmasterRecord};
pub use keys::KeyRecord;
pub use master_agent_admissions::AdmissionClaim;
pub use master_agents::{MasterAgentLimits, MasterAgentRecord, MasterAgentSetting, SettingChange};
pub use notifications::{
    NotificationPage, NotificationRow, NotificationSend, SendLimit, SentNotificationRow,
};
mod admission;
mod claims;
mod content;
mod groups;
mod identity;
mod keys;
mod lessons;
pub(crate) mod loca_removal;
mod master_agent_admissions;
mod master_agent_locas;
mod master_agents;
mod memory;
mod messages;
mod migrations;
mod notifications;
mod operators;
mod preferences;
mod profile_card;
mod rooms;
mod schema;
mod task_reviews;
mod vault;
mod wiki;
mod wiki_context;
mod wiki_maintenance;
mod work;
pub use lessons::LessonDecision;
use migrations::{
    add_column, add_invite_delivery_columns, attribute_messages_once, expire_delivery_copies,
    expire_invite_copies, migrate_admin_session_tokens, migrate_member_keys, migrate_smaster_codes,
    record_owed_checkpoint, rewrite_key_scopes, scrub_secret_residue,
    settle_owed_checkpoint_logged, DeliveryCopies,
};
pub use preferences::{PreferenceError, Preferences};
pub use profile_card::{CardPatch, ProfileCard, ProfileCardError};
#[cfg(test)]
pub(crate) use schema::LEGACY_MEMBER_TABLES;
use schema::STORAGE_MAINTENANCE_SCHEMA;
pub(crate) use schema::{
    admin_sessions_table_sql, invites_table_sql, member_suspensions_table_sql, members_table_sql,
    smasters_table_sql,
};
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
    /// An automation the Master appointed (`store/master_agents.rs`). It is
    /// not a Building rank: `is_building_admin` stays false for it.
    MasterAgent,
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
        conn.execute_batch(schema::CORE_SCHEMA)?;
        // Groups (PRINCIPLES "Groups"). An older database simply gains empty
        // tables: no loca belongs to a group, so every loca stays default.
        conn.execute_batch(groups::SCHEMA)?;
        // Notifications (docs/concepts.md "Notifications"): new tables only.
        conn.execute_batch(notifications::SCHEMA)?;
        // The Vault's rules (PRINCIPLES "Vault"): a new table only.
        conn.execute_batch(vault::SCHEMA)?;
        // Lessons proposed for the Vault: a new table only.
        conn.execute_batch(lessons::SCHEMA)?;
        // A task's review history (the Loca Board): a new table only.
        conn.execute_batch(task_reviews::SCHEMA)?;
        // The Building's own settings (its name): a new table only.
        conn.execute_batch(building_settings::SCHEMA)?;
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
        // The Master Agent rank: its registry and the journal of the
        // Building setting, and an older `principals` widened once to hold
        // the role.
        master_agents::widen_principal_roles(&mut conn)?;
        conn.execute_batch(master_agents::SCHEMA)?;
        conn.execute_batch(master_agent_locas::SCHEMA)?;
        conn.execute_batch(master_agent_admissions::SCHEMA)?;
        // Built before the seat step was recorded: the column is added empty.
        add_column(&conn, "master_agent_admissions", "seated_at INTEGER")?;
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
