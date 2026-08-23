use super::*;

impl Store {
    /// Archive a superseded note version so "when/who changed this" stays
    /// answerable. The audit layer: never trimmed, wiped only with the room.
    pub fn add_note_revision(&self, room: &str, n: &Note) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else { return Ok(()) };
        c.execute(
            "INSERT OR REPLACE INTO note_revisions (room, key, rev, title, body, updated_by, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![room, n.key, n.rev, n.title, n.body, n.updated_by, n.updated_at],
        )
        .inspect_err(|e| tracing::error!(error = %e, "add_note_revision failed"))
        .map(|_| ())
    }
    /// A note's past versions, newest first (without the current one).
    pub fn note_history(&self, room: &str, key: &str) -> Vec<Note> {
        let Some(c) = self.conn() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if let Ok(mut stmt) = c.prepare(
            "SELECT rev, title, body, updated_by, updated_at FROM note_revisions
             WHERE room = ?1 AND key = ?2 ORDER BY rev DESC",
        ) {
            if let Ok(rows) = stmt.query_map(params![room, key], |r| {
                Ok(Note {
                    key: key.to_string(),
                    rev: r.get(0)?,
                    title: r.get(1)?,
                    body: r.get(2)?,
                    can_write: Vec::new(),
                    updated_by: r.get(3)?,
                    updated_at: r.get(4)?,
                })
            }) {
                out.extend(rows.flatten());
            }
        }
        out
    }
    /// Case-insensitive substring search across the FULL message archive
    /// (the DB is never trimmed; memory only holds the hot tail).
    pub fn search_messages(&self, room: &str, q: &str, limit: usize) -> Vec<Message> {
        let Some(c) = self.conn() else {
            return Vec::new();
        };
        let needle = format!("%{}%", q.to_lowercase());
        let mut out = Vec::new();
        if let Ok(mut stmt) = c.prepare(
            "SELECT id, sender, sender_type, target, text, reply_to, ts, kind FROM messages
             WHERE room = ?1 AND (lower(text) LIKE ?2 OR lower(sender) LIKE ?2)
             ORDER BY id DESC LIMIT ?3",
        ) {
            if let Ok(rows) = stmt.query_map(params![room, needle, limit as i64], |r| {
                Ok(Message {
                    id: r.get(0)?,
                    room: room.to_string(),
                    sender: r.get(1)?,
                    sender_type: parse_sender_type(&r.get::<_, String>(2)?),
                    target: r.get(3)?,
                    text: r.get(4)?,
                    reply_to: r.get(5)?,
                    ts: r.get(6)?,
                    kind: parse_kind(&r.get::<_, String>(7).unwrap_or_default()),
                })
            }) {
                out.extend(rows.flatten());
            }
        }
        out
    }
    pub fn append_journal(&self, e: &protocol::JournalEntry) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else { return Ok(()) };
        c.execute(
            "INSERT INTO journal (id, room, by, by_type, text, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![e.id, e.room, e.by, sender_type_str(e.by_type), e.text, e.at],
        )
        .inspect_err(|err| tracing::error!(error = %err, "append_journal failed"))
        .map(|_| ())
    }
    /// The whole journal for a room, oldest first.
    pub fn load_journal(&self, room: &str) -> Vec<protocol::JournalEntry> {
        let Some(c) = self.conn() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if let Ok(mut stmt) =
            c.prepare("SELECT id, by, by_type, text, at FROM journal WHERE room = ?1 ORDER BY id")
        {
            if let Ok(rows) = stmt.query_map(params![room], |r| {
                Ok(protocol::JournalEntry {
                    id: r.get(0)?,
                    room: room.to_string(),
                    by: r.get(1)?,
                    by_type: parse_sender_type(&r.get::<_, String>(2)?),
                    text: r.get(3)?,
                    at: r.get(4)?,
                })
            }) {
                out.extend(rows.flatten());
            }
        }
        out
    }
    pub fn upsert_note(&self, room: &str, n: &Note) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else { return Ok(()) };
        let cw = serde_json::to_string(&n.can_write).unwrap_or_else(|_| "[]".into());
        c.execute(
            "INSERT OR REPLACE INTO notes (room, key, title, body, can_write, updated_by, updated_at, rev)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![room, n.key, n.title, n.body, cw, n.updated_by, n.updated_at, n.rev],
        )
        .inspect_err(|e| tracing::error!(error = %e, "upsert_note failed"))
        .map(|_| ())
    }
    pub fn delete_note(&self, room: &str, key: &str) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else { return Ok(()) };
        c.execute(
            "DELETE FROM notes WHERE room = ?1 AND key = ?2",
            params![room, key],
        )
        .inspect_err(|e| tracing::error!(error = %e, "delete_note failed"))
        .map(|_| ())
    }
}
