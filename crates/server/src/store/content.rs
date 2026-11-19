use super::*;

impl Store {
    /// A note's past versions, newest first (without the current one).
    pub fn note_history(&self, room: &str, key: &str) -> rusqlite::Result<Vec<Note>> {
        let Some(c) = self.conn() else {
            return Ok(Vec::new());
        };
        let mut stmt = c.prepare(
            "SELECT rev, title, body, updated_by, updated_at FROM note_revisions
             WHERE room = ?1 AND key = ?2 ORDER BY rev DESC",
        )?;
        let rows = stmt.query_map(params![room, key], |r| {
            Ok(Note {
                key: key.to_string(),
                rev: r.get(0)?,
                title: r.get(1)?,
                body: r.get(2)?,
                can_write: Vec::new(),
                updated_by: r.get(3)?,
                updated_at: r.get(4)?,
            })
        })?;
        rows.collect()
    }
    /// Case-insensitive substring search across the FULL message archive
    /// (the DB is never trimmed; memory only holds the hot tail).
    pub fn search_messages(
        &self,
        room: &str,
        q: &str,
        limit: usize,
    ) -> rusqlite::Result<Vec<Message>> {
        self.search_messages_in(&[room], q, limit)
    }
    /// The same search over several rooms at once, newest first. Message ids
    /// are global and monotonic, so the id order is the time order across
    /// rooms. Only the rooms named are read: the caller decides them.
    pub fn search_messages_in(
        &self,
        rooms: &[&str],
        q: &str,
        limit: usize,
    ) -> rusqlite::Result<Vec<Message>> {
        /// Rooms per statement, well under SQLite's bound-parameter limit.
        const ROOMS_PER_QUERY: usize = 400;
        if rooms.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let Some(c) = self.conn() else {
            return Ok(Vec::new());
        };
        let needle = search_fold(q);
        let limit_param = limit as i64;
        let mut found = Vec::new();
        for chunk in rooms.chunks(ROOMS_PER_QUERY) {
            let marks = (0..chunk.len())
                .map(|i| format!("?{}", i + 3))
                .collect::<Vec<_>>()
                .join(", ");
            let mut stmt = c.prepare(&format!(
                "SELECT id, room, sender, sender_type, target, text, reply_to, ts, kind,
                        attachments, sender_principal FROM messages
                 WHERE room IN ({marks})
                   AND (instr(loca_fold(text), ?1) > 0 OR instr(loca_fold(sender), ?1) > 0)
                 ORDER BY id DESC LIMIT ?2"
            ))?;
            let mut values: Vec<&dyn rusqlite::ToSql> = vec![&needle, &limit_param];
            values.extend(chunk.iter().map(|room| room as &dyn rusqlite::ToSql));
            let rows = stmt.query_map(values.as_slice(), |r| {
                Ok(Message {
                    attachments: attachments_from_json(r.get::<_, Option<String>>(9)?),
                    id: r.get(0)?,
                    room: r.get(1)?,
                    sender: r.get(2)?,
                    sender_type: parse_sender_type(&r.get::<_, String>(3)?),
                    target: r.get(4)?,
                    text: r.get(5)?,
                    reply_to: r.get(6)?,
                    reply_to_sender: None,
                    sender_principal: r.get(10)?,
                    ts: r.get(7)?,
                    kind: parse_kind(&r.get::<_, String>(8)?),
                })
            })?;
            for row in rows {
                found.push(row?);
            }
        }
        found.sort_by(|a, b| b.id.cmp(&a.id));
        found.truncate(limit);
        Ok(found)
    }
    pub fn append_journal(&self, e: &protocol::JournalEntry) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else { return Ok(()) };
        Self::write_journal(&c, e)
    }
    /// One Journal line, on a connection or inside a caller's transaction.
    pub(super) fn write_journal(
        c: &Connection,
        e: &protocol::JournalEntry,
    ) -> rusqlite::Result<()> {
        c.execute(
            "INSERT INTO journal (id, room, by, by_type, text, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![e.id, e.room, e.by, sender_type_str(e.by_type), e.text, e.at],
        )
        .inspect_err(|err| tracing::error!(error = %err, "append_journal failed"))
        .map(|_| ())
    }
    /// The whole journal for a room, oldest first.
    pub fn load_journal(&self, room: &str) -> rusqlite::Result<Vec<protocol::JournalEntry>> {
        let Some(c) = self.conn() else {
            return Ok(Vec::new());
        };
        let mut stmt =
            c.prepare("SELECT id, by, by_type, text, at FROM journal WHERE room = ?1 ORDER BY id")?;
        let rows = stmt.query_map(params![room], |r| {
            Ok(protocol::JournalEntry {
                id: r.get(0)?,
                room: room.to_string(),
                by: r.get(1)?,
                by_type: parse_sender_type(&r.get::<_, String>(2)?),
                text: r.get(3)?,
                at: r.get(4)?,
            })
        })?;
        rows.collect()
    }
    /// Archive and replace in one transaction; a failed write creates no phantom revision.
    pub fn replace_note(
        &self,
        room: &str,
        previous: &Note,
        updated: &Note,
    ) -> rusqlite::Result<()> {
        let Some(mut c) = self.conn() else {
            return Ok(());
        };
        let tx = c.transaction()?;
        tx.execute("INSERT INTO note_revisions (room,key,rev,title,body,updated_by,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7)", params![room, previous.key, previous.rev, previous.title, previous.body, previous.updated_by, previous.updated_at])?;
        let cw = serde_json::to_string(&updated.can_write)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        tx.execute("INSERT OR REPLACE INTO notes (room,key,title,body,can_write,updated_by,updated_at,rev) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)", params![room, updated.key, updated.title, updated.body, cw, updated.updated_by, updated.updated_at, updated.rev])?;
        tx.commit()
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
