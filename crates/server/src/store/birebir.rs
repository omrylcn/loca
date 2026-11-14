//! Birebir: a private loca for exactly two principals (PRINCIPLES "Birebir").

use rusqlite::{params, OptionalExtension};

use super::Store;

/// One birebir. `a` and `b` are principal ids with `a < b`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Birebir {
    pub room: String,
    pub a: String,
    pub b: String,
    pub opened_by: String,
    pub opened_at: u64,
    /// The party that called the Master in, while the Master is in.
    pub master_called_by: Option<String>,
    /// The time from which the called-in Master may read.
    pub master_since: Option<u64>,
    /// `master_since` was taken from the sequence that stamps messages, so it
    /// separates what was said before the call from what came after. A visit
    /// recorded before that rule carries `false`; its boundary came from the
    /// wall clock and cannot be trusted, so it is ended when the store opens.
    pub master_since_ordered: bool,
    pub history_a: bool,
    pub history_b: bool,
}

impl Birebir {
    pub fn has(&self, principal: &str) -> bool {
        self.a == principal || self.b == principal
    }
    pub fn other(&self, principal: &str) -> Option<&str> {
        if self.a == principal {
            Some(&self.b)
        } else if self.b == principal {
            Some(&self.a)
        } else {
            None
        }
    }
    /// Both sides allowed the called-in Master to read earlier history.
    pub fn history_open(&self) -> bool {
        self.history_a && self.history_b
    }
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Birebir> {
    Ok(Birebir {
        room: r.get(0)?,
        a: r.get(1)?,
        b: r.get(2)?,
        opened_by: r.get(3)?,
        opened_at: r.get::<_, i64>(4)? as u64,
        master_called_by: r.get(5)?,
        master_since: r.get::<_, Option<i64>>(6)?.map(|v| v as u64),
        history_a: r.get::<_, i64>(7)? != 0,
        history_b: r.get::<_, i64>(8)? != 0,
        master_since_ordered: r.get::<_, i64>(9)? != 0,
    })
}

const COLUMNS: &str =
    "room, a, b, opened_by, opened_at, master_called_by, master_since, history_a, history_b, \
     master_since_ordered";

impl Store {
    pub fn load_birebirs(&self) -> rusqlite::Result<Vec<Birebir>> {
        let Some(c) = self.conn() else {
            return Ok(Vec::new());
        };
        let mut st = c.prepare(&format!("SELECT {COLUMNS} FROM birebirs"))?;
        let rows = st.query_map([], row)?;
        rows.collect()
    }

    /// Persist a new birebir and its room state in one commit. Returns the
    /// existing row when the pair already has one (never a second room).
    pub fn create_birebir(
        &self,
        b: &Birebir,
        settings: &protocol::RoomSettings,
    ) -> rusqlite::Result<Birebir> {
        let Some(mut c) = self.conn() else {
            return Ok(b.clone());
        };
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let existing = tx
            .query_row(
                &format!("SELECT {COLUMNS} FROM birebirs WHERE a = ?1 AND b = ?2"),
                params![b.a, b.b],
                row,
            )
            .optional()?;
        if let Some(existing) = existing {
            return Ok(existing);
        }
        Self::write_room_state(&tx, &b.room, &protocol::ChatMode::Free, settings)?;
        tx.execute(
            "INSERT INTO birebirs (room, a, b, opened_by, opened_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![b.room, b.a, b.b, b.opened_by, b.opened_at as i64],
        )?;
        tx.commit()?;
        Ok(b.clone())
    }

    /// Record the Master's call-in state for a birebir.
    pub fn save_birebir_access(&self, b: &Birebir) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else {
            return Ok(());
        };
        c.execute(
            "UPDATE birebirs SET master_called_by = ?2, master_since = ?3,
                 history_a = ?4, history_b = ?5, master_since_ordered = ?6 WHERE room = ?1",
            params![
                b.room,
                b.master_called_by,
                b.master_since.map(|v| v as i64),
                b.history_a as i64,
                b.history_b as i64,
                b.master_since_ordered as i64
            ],
        )?;
        Ok(())
    }
}

/// End every Master visit whose call boundary was stamped with the wall clock
/// by an earlier version. Its true boundary cannot be recovered, and guessing
/// would either hide words the Master was entitled to or show words said
/// before the call, so the visit ends and the two call again if they wish.
/// One statement, safe to repeat: a visit opened under the ordered rule is
/// never touched, and a failure stops the open so the next start retries.
pub(super) fn end_unordered_birebir_visits(conn: &rusqlite::Connection) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE birebirs SET master_called_by = NULL, master_since = NULL,
             history_a = 0, history_b = 0
         WHERE master_since IS NOT NULL AND master_since_ordered = 0",
        [],
    )
}
