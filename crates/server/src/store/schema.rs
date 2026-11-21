//! The schema the store opens with: the core tables and the builders of the
//! tables an upgrade rebuilds.

/// The core tables and indexes, created when the store opens
/// (`Store::open_at`). A database that already has them is left as it is.
pub(super) const CORE_SCHEMA: &str = r#"
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
            -- An older table without 'master_agent' in the role CHECK is
            -- rebuilt once by `master_agents::widen_principal_roles`.
            CREATE TABLE IF NOT EXISTS principals (
                id TEXT PRIMARY KEY,
                display_name TEXT NOT NULL,
                kind TEXT NOT NULL CHECK (kind IN ('human', 'agent')),
                building_role TEXT NOT NULL
                    CHECK (building_role IN ('master', 'smaster', 'member', 'master_agent')),
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
            -- levels the reviewer requires with no stored counter to drift:
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
            "#;

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
pub(super) const STORAGE_MAINTENANCE_SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS storage_maintenance (
        task TEXT PRIMARY KEY,
        since INTEGER NOT NULL
    );
";
