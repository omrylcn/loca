use super::*;

impl Store {
    /// Atomically move every durable record from one loca name to another.
    /// Re-running after a successful migration is a no-op; merging two
    /// existing locas is refused because their histories must never blur.
    pub fn rename_room(&self, from: &str, to: &str) -> rusqlite::Result<bool> {
        let Some(mut c) = self.conn() else {
            return Ok(false);
        };
        const TABLES: &[&str] = &[
            "messages",
            "notes",
            "note_revisions",
            "tasks",
            "goals",
            "waits",
            "care_marks",
            "attentions",
            "rooms",
            "invites",
            "journal",
            "bans",
        ];
        let exists = |conn: &Connection, room: &str| -> rusqlite::Result<bool> {
            for table in TABLES {
                let sql = format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE room = ?1)");
                if conn.query_row(&sql, params![room], |row| row.get(0))? {
                    return Ok(true);
                }
            }
            Ok(false)
        };
        if !exists(&c, from)? {
            return Ok(false);
        }
        if exists(&c, to)? {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "cannot rename loca '{from}' to existing loca '{to}'"
            )));
        }
        let tx = c.transaction()?;
        for table in TABLES {
            let sql = format!("UPDATE {table} SET room = ?2 WHERE room = ?1");
            tx.execute(&sql, params![from, to])?;
        }
        // A care signal has two loca references: the room where its owner
        // receives it and the source room embedded in the durable JSON.
        // Keep the signal id stable so a late ACK remains valid.
        let outbox_updates = {
            let mut stmt =
                tx.prepare("SELECT id, attention_id, delivery_room, signal FROM care_outbox")?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?;
            let mut updates = Vec::new();
            for row in rows {
                let (id, attention_id, delivery_room, json) = row?;
                let mut signal: protocol::CareSignal =
                    serde_json::from_str(&json).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            3,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?;
                let mut changed = false;
                if signal.room == from {
                    signal.room = to.to_string();
                    changed = true;
                }
                let old_prefix = format!("attention:{from}:");
                let next_attention_id = attention_id
                    .strip_prefix(&old_prefix)
                    .map(|suffix| format!("attention:{to}:{suffix}"))
                    .unwrap_or(attention_id);
                if signal.attention_id != next_attention_id {
                    signal.attention_id = next_attention_id.clone();
                    changed = true;
                }
                let next_delivery = if delivery_room == from {
                    changed = true;
                    to
                } else {
                    delivery_room.as_str()
                };
                if changed {
                    updates.push((
                        id,
                        next_attention_id,
                        next_delivery.to_string(),
                        serde_json::to_string(&signal).map_err(|error| {
                            rusqlite::Error::ToSqlConversionFailure(Box::new(error))
                        })?,
                    ));
                }
            }
            updates
        };
        for (id, attention_id, delivery_room, signal) in outbox_updates {
            tx.execute(
                "UPDATE care_outbox
                 SET attention_id = ?2, delivery_room = ?3, signal = ?4
                 WHERE id = ?1",
                params![id, attention_id, delivery_room, signal],
            )?;
        }
        let attention_updates = {
            let mut stmt = tx.prepare("SELECT id, signal FROM attentions WHERE room = ?1")?;
            let rows = stmt.query_map(params![to], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            let mut updates = Vec::new();
            for row in rows {
                let (id, json) = row?;
                let mut signal: protocol::CareSignal =
                    serde_json::from_str(&json).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            1,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?;
                if signal.room == from {
                    signal.room = to.to_string();
                    let old_prefix = format!("attention:{from}:");
                    let next_id = id
                        .strip_prefix(&old_prefix)
                        .map(|suffix| format!("attention:{to}:{suffix}"))
                        .unwrap_or_else(|| id.clone());
                    if signal.attention_id == id {
                        signal.attention_id = next_id.clone();
                    }
                    updates.push((
                        id,
                        next_id,
                        serde_json::to_string(&signal).map_err(|error| {
                            rusqlite::Error::ToSqlConversionFailure(Box::new(error))
                        })?,
                    ));
                }
            }
            updates
        };
        for (id, next_id, signal) in attention_updates {
            tx.execute(
                "UPDATE attentions SET id = ?2, signal = ?3 WHERE id = ?1",
                params![id, next_id, signal],
            )?;
        }
        tx.commit()
            .inspect_err(|e| tracing::error!(error = %e, %from, %to, "rename_room failed"))?;
        Ok(true)
    }
    /// Seal a room: mark it closed-for-good WITHOUT destroying its record.
    /// Messages, notes, tasks, journal, invites all stay on disk so the history
    /// remains answerable (PRINCIPLES: "seal not destroy"; revoking is not
    /// forgetting). The `sealed_at` marker is what keeps it from reopening
    /// across a restart — boot reads it and re-tombstones the loca.
    pub fn seal_room(&self, room: &str, at: u64) -> rusqlite::Result<()> {
        let Some(mut c) = self.conn() else {
            return Ok(());
        };
        // A room may have been created only via a message/journal with no
        // `rooms` row yet; ensure a row exists to carry the seal marker.
        let tx = c.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO rooms (room, mode, settings) VALUES (?1, '{\"mode\":\"free\"}', '{}')",
            params![room],
        )?;
        tx.execute(
            "UPDATE rooms SET sealed_at = ?2 WHERE room = ?1",
            params![room, at as i64],
        )?;
        Self::retire_room_attentions(&tx, room, at)?;
        tx.commit()
            .inspect_err(|e| tracing::error!(error = %e, "seal_room failed"))
    }
    /// Rooms that were sealed — re-tombstoned at boot so a sealed loca never
    /// silently reopens (the tombstone set is otherwise memory-only).
    pub fn sealed_rooms(&self) -> Vec<String> {
        let Some(c) = self.conn() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if let Ok(mut stmt) = c.prepare("SELECT room FROM rooms WHERE sealed_at IS NOT NULL") {
            if let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0)) {
                out.extend(rows.flatten());
            }
        }
        out
    }
    pub fn save_room(
        &self,
        room: &str,
        mode: &ChatMode,
        settings: &RoomSettings,
    ) -> rusqlite::Result<()> {
        let Some(mut c) = self.conn() else {
            return Ok(());
        };
        let mode_j = serde_json::to_string(mode).unwrap_or_else(|_| "{\"mode\":\"free\"}".into());
        let set_j = serde_json::to_string(settings).unwrap_or_default();
        let tx = c.transaction()?;
        tx.execute(
            "INSERT OR REPLACE INTO rooms (room, mode, settings) VALUES (?1, ?2, ?3)",
            params![room, mode_j, set_j],
        )?;
        if settings.archived {
            Self::pause_room_attentions(&tx, room, current_time_ms())?;
        }
        tx.commit()
            .inspect_err(|e| tracing::error!(error = %e, "save_room failed"))
    }
    fn retire_room_attentions(c: &Connection, room: &str, at: u64) -> rusqlite::Result<()> {
        Self::pause_room_attentions(c, room, at)?;
        c.execute(
            "UPDATE attentions SET resolved_at = COALESCE(resolved_at, ?2)
             WHERE room = ?1",
            params![room, at],
        )?;
        Ok(())
    }
    fn pause_room_attentions(c: &Connection, room: &str, at: u64) -> rusqlite::Result<()> {
        c.execute(
            "UPDATE care_outbox SET acked_at = COALESCE(acked_at, ?2)
             WHERE attention_id IN (SELECT id FROM attentions WHERE room = ?1)",
            params![room, at],
        )?;
        c.execute("DELETE FROM care_marks WHERE room = ?1", params![room])?;
        Ok(())
    }
    pub fn load(&self) -> Vec<RoomSnapshot> {
        let Some(c) = self.conn() else {
            return Vec::new();
        };

        // A loca can begin with any durable room-scoped record. Discover the
        // union across every such table so a task-only or journal-only loca
        // does not disappear merely because nobody has spoken yet.
        let mut rooms: std::collections::BTreeSet<String> = Default::default();
        for tbl in [
            "messages",
            "notes",
            "tasks",
            "goals",
            "waits",
            "care_marks",
            "attentions",
            "journal",
            "rooms",
        ] {
            let sql = format!("SELECT DISTINCT room FROM {tbl}");
            if let Ok(mut stmt) = c.prepare(&sql) {
                if let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0)) {
                    for r in rows.flatten() {
                        rooms.insert(r);
                    }
                }
            }
        }

        // Sealed locas keep their rows (history is preserved) but must NOT come
        // back as live rooms — they are re-tombstoned instead (see hub boot).
        // Query through the connection we already hold: calling `sealed_rooms()`
        // here would take the store lock a SECOND time and deadlock (`conn()`
        // returns a guard).
        let mut sealed: std::collections::HashSet<String> = Default::default();
        if let Ok(mut stmt) = c.prepare("SELECT room FROM rooms WHERE sealed_at IS NOT NULL") {
            if let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0)) {
                sealed.extend(rows.flatten());
            }
        }

        rooms
            .into_iter()
            .filter(|room| !sealed.contains(room))
            .map(|room| self.load_room(&c, &room))
            .collect()
    }
    fn load_room(&self, c: &Connection, room: &str) -> RoomSnapshot {
        // messages
        let mut messages = Vec::new();
        let mut max_msg_id = 0u64;
        // Hot context only: the archive can be huge, memory holds the tail.
        if let Ok(mut stmt) = c.prepare(
            "SELECT id, sender, sender_type, target, text, reply_to, ts, kind FROM
               (SELECT * FROM messages WHERE room = ?1 ORDER BY id DESC LIMIT 200)
             ORDER BY id",
        ) {
            if let Ok(rows) = stmt.query_map(params![room], |r| {
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
                for m in rows.flatten() {
                    max_msg_id = max_msg_id.max(m.id);
                    messages.push(m);
                }
            }
        }

        // notes
        let mut notes = Vec::new();
        let mut max_rev = 0u64;
        if let Ok(mut stmt) = c.prepare(
            "SELECT key, title, body, can_write, updated_by, updated_at, rev FROM notes WHERE room = ?1",
        ) {
            if let Ok(rows) = stmt.query_map(params![room], |r| {
                let cw: String = r.get(3)?;
                Ok(Note {
                    key: r.get(0)?,
                    title: r.get(1)?,
                    body: r.get(2)?,
                    can_write: serde_json::from_str(&cw).unwrap_or_default(),
                    updated_by: r.get(4)?,
                    updated_at: r.get(5)?,
                    rev: r.get(6)?,
                })
            }) {
                for n in rows.flatten() {
                    max_rev = max_rev.max(n.rev);
                    notes.push(n);
                }
            }
        }

        // The id counter must resume past the ARCHIVE's max, not the tail's.
        if let Ok(mx) = c.query_row(
            "SELECT COALESCE(MAX(id), 0) FROM messages WHERE room = ?1",
            params![room],
            |r| r.get::<_, u64>(0),
        ) {
            max_msg_id = max_msg_id.max(mx);
        }

        // mode + settings
        let (mode, settings) = c
            .query_row(
                "SELECT mode, settings FROM rooms WHERE room = ?1",
                params![room],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .ok()
            .map(|(m, s)| {
                (
                    serde_json::from_str(&m).unwrap_or(ChatMode::Free),
                    serde_json::from_str(&s).unwrap_or_default(),
                )
            })
            .unwrap_or((ChatMode::Free, RoomSettings::default()));

        RoomSnapshot {
            room: room.to_string(),
            messages,
            notes,
            mode,
            settings,
            max_msg_id,
            max_rev,
        }
    }
}
