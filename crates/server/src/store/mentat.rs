//! Mentat rooms (`protocol::MentatRoom`) and the requests to consult their
//! expert (`protocol::MentatRequest`). A loca with a `loca_mentat` row is a
//! mentat room from its birth to its end: the row is written with the room
//! and never removed while the room lives. Requests are inserted, then moved
//! through their states; nothing is deleted. Topics are stored as given and
//! never read for meaning.
use super::{params, Store};
use protocol::{MentatLimits, MentatRequest, MentatRequestStatus, MentatRoom};

/// New tables only, so a database from an earlier version opens unchanged:
/// none of its locas is a mentat room. A request is kept whole, as JSON, with
/// its status beside it for the queries that read by status.
pub(super) const SCHEMA: &str = r#"
    CREATE TABLE IF NOT EXISTS loca_mentat (
        room TEXT PRIMARY KEY,
        topic TEXT NOT NULL,
        mentat TEXT,
        mentat_principal TEXT,
        founder TEXT,
        limits TEXT NOT NULL,
        set_by TEXT NOT NULL,
        set_at INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS mentat_requests (
        room TEXT NOT NULL,
        id INTEGER NOT NULL,
        status TEXT NOT NULL,
        ended_at INTEGER,
        data TEXT NOT NULL,
        PRIMARY KEY (room, id)
    );
"#;

fn status_str(status: MentatRequestStatus) -> &'static str {
    match status {
        MentatRequestStatus::Waiting => "waiting",
        MentatRequestStatus::Called => "called",
        MentatRequestStatus::Active => "active",
        MentatRequestStatus::Ended => "ended",
        MentatRequestStatus::Skipped => "skipped",
        MentatRequestStatus::Expired => "expired",
        MentatRequestStatus::Withdrawn => "withdrawn",
    }
}

fn request_of(room: &str, data: &str) -> rusqlite::Result<MentatRequest> {
    let mut request: MentatRequest = serde_json::from_str(data).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })?;
    // The room column is the authority: it follows a rename.
    request.room = room.to_string();
    Ok(request)
}

impl Store {
    /// Found a mentat room: the loca, its place in its group and its mentat
    /// record, in one commit. It gets no Operator. `false` when a loca of
    /// that name already exists.
    pub fn create_mentat_room(
        &self,
        settings: &protocol::RoomSettings,
        m: &MentatRoom,
        group: Option<&str>,
        at: u64,
    ) -> rusqlite::Result<bool> {
        let Some(mut c) = self.conn() else {
            return Ok(true);
        };
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM rooms WHERE room = ?1)",
            params![m.room],
            |row| row.get(0),
        )?;
        if exists {
            return Ok(false);
        }
        let mode = protocol::ChatMode::Restricted {
            allow: m.mentat.iter().cloned().collect(),
        };
        Self::write_room_state(&tx, &m.room, &mode, settings)?;
        if group.is_some() {
            super::groups::write_room_group(&tx, &m.room, group, m.founder.as_deref(), at)?;
        }
        let limits = serde_json::to_string(&m.limits).unwrap_or_else(|_| "{}".into());
        tx.execute(
            "INSERT INTO loca_mentat (room, topic, mentat, mentat_principal, founder, limits,
                                      set_by, set_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                m.room,
                m.topic,
                m.mentat,
                m.mentat_principal,
                m.founder,
                limits,
                m.set_by,
                m.set_at
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Every mentat room.
    pub fn mentat_rooms(&self) -> rusqlite::Result<Vec<MentatRoom>> {
        let Some(c) = self.conn() else {
            return Ok(Vec::new());
        };
        let mut statement = c.prepare(
            "SELECT room, topic, mentat, mentat_principal, founder, limits, set_by, set_at
             FROM loca_mentat ORDER BY room",
        )?;
        let rows = statement.query_map([], |row| {
            let limits: String = row.get(5)?;
            Ok(MentatRoom {
                room: row.get(0)?,
                topic: row.get(1)?,
                mentat: row.get(2)?,
                mentat_principal: row.get(3)?,
                founder: row.get(4)?,
                limits: serde_json::from_str::<MentatLimits>(&limits)
                    .unwrap_or_default()
                    .filled(),
                set_by: row.get(6)?,
                set_at: row.get(7)?,
            })
        })?;
        rows.collect()
    }

    /// Write a mentat room's record: at its birth, and whenever its mentat,
    /// topic or limits change.
    pub fn save_mentat_room(&self, m: &MentatRoom) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else {
            return Ok(());
        };
        let limits = serde_json::to_string(&m.limits).unwrap_or_else(|_| "{}".into());
        c.execute(
            "INSERT INTO loca_mentat (room, topic, mentat, mentat_principal, founder, limits,
                                      set_by, set_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(room) DO UPDATE SET topic = ?2, mentat = ?3, mentat_principal = ?4,
                 limits = ?6, set_by = ?7, set_at = ?8",
            params![
                m.room,
                m.topic,
                m.mentat,
                m.mentat_principal,
                m.founder,
                limits,
                m.set_by,
                m.set_at
            ],
        )?;
        Ok(())
    }

    /// The requests of `room` still in the line or under way, oldest first.
    pub fn open_mentat_requests(&self, room: &str) -> rusqlite::Result<Vec<MentatRequest>> {
        let Some(c) = self.conn() else {
            return Ok(Vec::new());
        };
        let mut statement = c.prepare(
            "SELECT data FROM mentat_requests
             WHERE room = ?1 AND status IN ('waiting', 'called', 'active') ORDER BY id",
        )?;
        let rows = statement.query_map(params![room], |row| {
            request_of(room, &row.get::<_, String>(0)?)
        })?;
        rows.collect()
    }

    /// The requests of `room` that are over, newest first.
    pub fn closed_mentat_requests(
        &self,
        room: &str,
        limit: usize,
    ) -> rusqlite::Result<Vec<MentatRequest>> {
        let Some(c) = self.conn() else {
            return Ok(Vec::new());
        };
        let mut statement = c.prepare(
            "SELECT data FROM mentat_requests
             WHERE room = ?1 AND status NOT IN ('waiting', 'called', 'active')
             ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = statement.query_map(params![room, limit as i64], |row| {
            request_of(room, &row.get::<_, String>(0)?)
        })?;
        rows.collect()
    }

    /// The highest request id `room` ever used (0 for none), so new ids
    /// continue past ended requests.
    pub fn last_mentat_request_id(&self, room: &str) -> rusqlite::Result<u64> {
        let Some(c) = self.conn() else {
            return Ok(0);
        };
        c.query_row(
            "SELECT COALESCE(MAX(id), 0) FROM mentat_requests WHERE room = ?1",
            params![room],
            |row| row.get(0),
        )
    }

    /// Write a request as it now stands (insert or replace by id).
    pub fn save_mentat_request(&self, r: &MentatRequest) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else {
            return Ok(());
        };
        let data = serde_json::to_string(r).unwrap_or_else(|_| "{}".into());
        c.execute(
            "INSERT INTO mentat_requests (room, id, status, ended_at, data)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(room, id) DO UPDATE SET status = ?3, ended_at = ?4, data = ?5",
            params![r.room, r.id, status_str(r.status), r.ended_at, data],
        )?;
        Ok(())
    }
}
