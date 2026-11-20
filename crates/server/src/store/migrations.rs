//! Upgrading an older database while the store opens: added columns, rebuilt
//! tables, scrubbed secrets and owed checkpoints.

use super::*;

/// Add one column to an existing table while the store opens. A database that
/// already has the column answers "duplicate column name", and only that
/// answer counts as success. Every other failure (a missing table, a locked or
/// read-only file, a full disk) is returned, so `Store::open` stops with the
/// real error instead of serving a half-upgraded schema.
pub(super) fn add_column(
    conn: &Connection,
    table: &str,
    column_definition: &str,
) -> rusqlite::Result<()> {
    let sql = format!("ALTER TABLE {table} ADD COLUMN {column_definition}");
    match conn.execute(&sql, []) {
        Ok(_) => Ok(()),
        Err(error) if is_duplicate_column(&error) => Ok(()),
        Err(error) => Err(error),
    }
}

pub(super) fn is_duplicate_column(error: &rusqlite::Error) -> bool {
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
pub(super) fn attribute_messages_once(conn: &mut Connection) -> rusqlite::Result<()> {
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

/// The chore `migrate_smaster_codes` leaves behind: remove the clear-text
/// codes the rebuilt table left in free pages and in the WAL.
pub(super) const SCRUB_SMASTER_CODES: &str = "scrub_smaster_codes";

/// The same chore for `migrate_admin_session_tokens`: remove the clear-text
/// session tokens.
pub(super) const SCRUB_ADMIN_SESSION_TOKENS: &str = "scrub_admin_session_tokens";

/// The same chore for `migrate_member_keys`: remove the clear-text
/// membership keys and invites.
pub(super) const SCRUB_MEMBER_KEYS: &str = "scrub_member_keys";

/// Every chore `scrub_secret_residue` finishes. They are one job (rewrite the
/// file, empty the WAL), so one run clears all that are recorded.
pub(super) const SECRET_SCRUBS: [&str; 3] = [
    SCRUB_SMASTER_CODES,
    SCRUB_ADMIN_SESSION_TOKENS,
    SCRUB_MEMBER_KEYS,
];

/// Record, inside the migration's own transaction, that the file still holds
/// the secrets it removed from the tables, so the clean-up runs even if this
/// open stops before it.
pub(super) fn record_secret_scrub(
    tx: &rusqlite::Transaction<'_>,
    task: &str,
) -> rusqlite::Result<()> {
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
pub(super) const CHECKPOINT_JOIN_KEYS: &str = "checkpoint_join_bootstrap_keys";

/// Which key delivery copies `expire_delivery_copies` clears.
#[derive(Clone, Copy)]
pub(super) enum DeliveryCopies<'a> {
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
pub(super) fn expire_delivery_copies(
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
pub(super) fn expire_invite_copies(
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
pub(super) fn add_invite_delivery_columns(conn: &mut Connection, now: u64) -> rusqlite::Result<()> {
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
pub(super) const KEY_SCOPED_COLUMNS: [(&str, &str); 3] = [
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
pub(super) fn rewrite_key_scopes(conn: &mut Connection) -> rusqlite::Result<usize> {
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
pub(super) fn record_owed_checkpoint(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT OR IGNORE INTO storage_maintenance (task, since) VALUES (?1, ?2)",
        params![CHECKPOINT_JOIN_KEYS, current_time_ms()],
    )
    .map(|_| ())
}

pub(super) fn checkpoint_owed(conn: &Connection) -> rusqlite::Result<bool> {
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
pub(super) fn settle_owed_checkpoint(conn: &Connection) -> rusqlite::Result<bool> {
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
pub(super) fn settle_owed_checkpoint_logged(conn: &Connection) {
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
pub(super) fn migrate_smaster_codes(conn: &mut Connection) -> rusqlite::Result<bool> {
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

pub(super) fn rebuild_smaster_registry(conn: &mut Connection) -> rusqlite::Result<usize> {
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
pub(super) fn migrate_admin_session_tokens(conn: &mut Connection) -> rusqlite::Result<bool> {
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

pub(super) fn rebuild_admin_sessions(conn: &mut Connection) -> rusqlite::Result<usize> {
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
pub(super) fn migrate_member_keys(conn: &mut Connection) -> rusqlite::Result<bool> {
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
pub(super) type LegacyMemberRow = (String, String, String, u64, String, Option<u64>);
/// One `invites` row as an earlier build wrote it: the invite, room, member
/// key, name, kind, issue, issuer, revocation, delivery copy and first use.
pub(super) type LegacyInviteRow = (
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
pub(super) struct MemberTableShapes {
    members: bool,
    invites: bool,
    suspensions: bool,
}

pub(super) fn rebuild_member_tables(
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
pub(super) fn scrub_secret_residue(conn: &Connection) -> rusqlite::Result<()> {
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
