//! The locas a Master Agent opened. A Master Agent holds no Operator seat and
//! is not a member of the floor; this record is what its door asks.

use rusqlite::{params, OptionalExtension};

use super::{MasterAgentRecord, Store};

pub(super) const SCHEMA: &str = r#"
    CREATE TABLE IF NOT EXISTS master_agent_locas (
        room TEXT PRIMARY KEY,
        principal_id TEXT NOT NULL,
        group_id TEXT NOT NULL,
        opened_at INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS master_agent_locas_principal
        ON master_agent_locas(principal_id);
"#;

impl Store {
    /// Open a loca for a Master Agent: the loca, its floor and the record of
    /// who opened it, in one transaction. `Ok(false)` when the name is taken.
    pub fn create_master_agent_loca(
        &self,
        room: &str,
        settings: &protocol::RoomSettings,
        principal: &str,
        group: &str,
        at: u64,
    ) -> rusqlite::Result<bool> {
        let Some(mut c) = self.conn() else {
            return Err(rusqlite::Error::InvalidQuery);
        };
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM rooms WHERE room = ?1)",
            params![room],
            |row| row.get(0),
        )?;
        if exists {
            return Ok(false);
        }
        Self::write_room_state(&tx, room, &protocol::ChatMode::Free, settings)?;
        // Born on its floor, never on disk outside it.
        super::groups::write_room_group(&tx, room, Some(group), Some(principal), at)?;
        tx.execute(
            "INSERT INTO master_agent_locas (room, principal_id, group_id, opened_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![room, principal, group, at],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// The Master Agent principal that opened `room`, if one did.
    pub fn master_agent_loca_opener(&self, room: &str) -> Option<String> {
        let c = self.conn()?;
        c.query_row(
            "SELECT principal_id FROM master_agent_locas WHERE room = ?1",
            params![room],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten()
    }

    /// The locas a Master Agent opened (every Master Agent's when `None`).
    pub fn master_agent_rooms(&self, principal: Option<&str>) -> Vec<String> {
        let Some(c) = self.conn() else {
            return Vec::new();
        };
        let rows: rusqlite::Result<Vec<String>> = (|| {
            let mut st = c.prepare(
                "SELECT room FROM master_agent_locas
                 WHERE ?1 IS NULL OR principal_id = ?1 ORDER BY room",
            )?;
            let rows = st.query_map(params![principal], |r| r.get(0))?;
            rows.collect()
        })();
        rows.unwrap_or_default()
    }

    /// How many locas a Master Agent opened on `group` that are still there
    /// (not sealed or deleted).
    pub fn master_agent_locas_on_floor(&self, principal: &str, group: &str) -> u32 {
        self.count_master_agent_locas(
            "SELECT COUNT(*) FROM master_agent_locas m JOIN rooms r ON r.room = m.room
             WHERE m.principal_id = ?1 AND m.group_id = ?2 AND r.sealed_at IS NULL",
            params![principal, group],
        )
    }

    /// How many locas a Master Agent opened at or after `since`.
    pub fn master_agent_locas_since(&self, principal: &str, since: u64) -> u32 {
        self.count_master_agent_locas(
            "SELECT COUNT(*) FROM master_agent_locas WHERE principal_id = ?1 AND opened_at >= ?2",
            params![principal, since as i64],
        )
    }

    /// A count that cannot be read counts as over every limit.
    fn count_master_agent_locas(&self, sql: &str, args: impl rusqlite::Params) -> u32 {
        let Some(c) = self.conn() else {
            return u32::MAX;
        };
        c.query_row(sql, args, |r| r.get::<_, i64>(0))
            .map(|n| u32::try_from(n).unwrap_or(u32::MAX))
            .unwrap_or(u32::MAX)
    }

    /// The registry row of a Master Agent principal, revoked or not.
    pub fn master_agent_by_principal(&self, principal: &str) -> Option<MasterAgentRecord> {
        self.master_agents()
            .ok()?
            .into_iter()
            .find(|record| record.principal_id == principal)
    }
}
