//! What the Master sets for the whole Building at runtime, one row per
//! setting. Today that is only the Building's display name; everything else a
//! Building runs on comes from its environment and is not stored here.
use super::*;

/// A new table only, so a database from an earlier version opens unchanged:
/// its Building simply has no name.
pub(super) const SCHEMA: &str = r#"
    CREATE TABLE IF NOT EXISTS building_settings (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL,
        updated_by_principal TEXT,
        updated_at INTEGER NOT NULL
    );
"#;

const NAME_KEY: &str = "name";

impl Store {
    /// The Building's display name, or `None` when it has none (or there is
    /// no database).
    pub fn building_name(&self) -> rusqlite::Result<Option<String>> {
        let Some(c) = self.conn() else {
            return Ok(None);
        };
        c.query_row(
            "SELECT value FROM building_settings WHERE key = ?1",
            params![NAME_KEY],
            |row| row.get(0),
        )
        .optional()
    }

    /// Set (`Some`) or clear (`None`) the Building's display name. The hub
    /// validates it first and refuses a memory-only Building before calling.
    pub fn set_building_name(
        &self,
        name: Option<&str>,
        by_principal: Option<&str>,
        at: u64,
    ) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else {
            return Ok(());
        };
        match name {
            Some(name) => c.execute(
                "INSERT INTO building_settings (key, value, updated_by_principal, updated_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value,
                     updated_by_principal = excluded.updated_by_principal,
                     updated_at = excluded.updated_at",
                params![NAME_KEY, name, by_principal, at as i64],
            )?,
            None => c.execute(
                "DELETE FROM building_settings WHERE key = ?1",
                params![NAME_KEY],
            )?,
        };
        Ok(())
    }
}
