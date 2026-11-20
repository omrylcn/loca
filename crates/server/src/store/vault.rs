//! The Vault's rules document (PRINCIPLES "Vault"): every revision of one
//! plain text per loca, with who wrote it and when. Rows are only inserted;
//! the newest revision is the document. The text is stored as given and
//! never read for meaning.
use super::{params, sender_type_str, ChatMode, OptionalExtension, RoomSettings, Store};

/// A new table only, so a database from an earlier version opens unchanged:
/// none of its locas has rules.
pub(super) const SCHEMA: &str = r#"
    CREATE TABLE IF NOT EXISTS loca_vault (
        room TEXT NOT NULL,
        rev INTEGER NOT NULL,
        body TEXT NOT NULL,
        updated_by TEXT NOT NULL,
        updated_by_type TEXT NOT NULL,
        updated_by_principal TEXT,
        updated_at INTEGER NOT NULL,
        PRIMARY KEY (room, rev)
    );
"#;

/// One revision as the hub writes it.
pub struct VaultRevision<'a> {
    pub room: &'a str,
    pub rev: u64,
    pub body: &'a str,
    pub by: &'a str,
    pub by_type: protocol::SenderType,
    pub by_principal: Option<&'a str>,
    pub at: u64,
}

fn doc_of(room: &str, row: &rusqlite::Row<'_>) -> rusqlite::Result<protocol::VaultDoc> {
    Ok(protocol::VaultDoc {
        room: room.to_string(),
        rev: row.get(0)?,
        body: row.get(1)?,
        updated_by: Some(row.get(2)?),
        updated_at: Some(row.get(3)?),
    })
}

impl Store {
    /// The newest revision of `room`'s rules, or `None` before the first.
    pub fn vault(&self, room: &str) -> rusqlite::Result<Option<protocol::VaultDoc>> {
        let Some(c) = self.conn() else {
            return Ok(None);
        };
        c.query_row(
            "SELECT rev, body, updated_by, updated_at FROM loca_vault
             WHERE room = ?1 ORDER BY rev DESC LIMIT 1",
            params![room],
            |row| doc_of(room, row),
        )
        .optional()
    }

    /// Up to `limit` revisions of `room`'s rules, newest first.
    pub fn vault_history(
        &self,
        room: &str,
        limit: usize,
    ) -> rusqlite::Result<Vec<protocol::VaultDoc>> {
        let Some(c) = self.conn() else {
            return Ok(Vec::new());
        };
        let mut statement = c.prepare(
            "SELECT rev, body, updated_by, updated_at FROM loca_vault
             WHERE room = ?1 ORDER BY rev DESC LIMIT ?2",
        )?;
        let rows = statement.query_map(params![room, limit as i64], |row| doc_of(room, row))?;
        rows.collect()
    }

    /// Write a revision, the loca's settings that name it (`vault_rev`) and
    /// the Journal line that records it, in one transaction: all three land
    /// or none does.
    pub fn save_vault_revision(
        &self,
        revision: &VaultRevision<'_>,
        mode: &ChatMode,
        settings: &RoomSettings,
        entry: &protocol::JournalEntry,
    ) -> rusqlite::Result<()> {
        let Some(mut c) = self.conn() else {
            return Ok(());
        };
        let tx = c.transaction()?;
        tx.execute(
            "INSERT INTO loca_vault
                (room, rev, body, updated_by, updated_by_type, updated_by_principal, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                revision.room,
                revision.rev,
                revision.body,
                revision.by,
                sender_type_str(revision.by_type),
                revision.by_principal,
                revision.at
            ],
        )?;
        Self::write_room_state(&tx, revision.room, mode, settings)?;
        Self::write_journal(&tx, entry)?;
        tx.commit()
            .inspect_err(|e| tracing::error!(error = %e, "save_vault_revision failed"))
    }
}
