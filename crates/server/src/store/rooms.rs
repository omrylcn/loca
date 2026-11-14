use super::*;

/// Every table whose rows belong to a loca by its name, in a `room` column.
/// `rename_room` moves all of them together. A test reads the schema and
/// fails when a table with a `room` column is in neither this list nor
/// `ROOM_TABLES_NEVER_RENAMED`, so a new loca table cannot be left behind the
/// way `group_rooms`, the Wiki, attachments and operator appointments were.
pub(super) const ROOM_KEYED_TABLES: &[&str] = &[
    "messages",
    "message_reactions",
    "attachment_pending",
    "attachment_refs",
    "notes",
    "note_revisions",
    "tasks",
    "goals",
    "waits",
    "care_marks",
    "attentions",
    "rooms",
    "room_operator_assignments",
    "invites",
    "journal",
    "bans",
    "group_rooms",
    "loca_memory",
    "loca_memory_entries",
    "loca_wiki",
    "loca_wiki_pages",
    "loca_wiki_history",
    "loca_wiki_batches",
    "loca_wiki_receipts",
    "wiki_context_selection",
    "wiki_context_receipts",
    "wiki_native_runtime",
];

/// Tables with a `room` column that a rename must not carry. A birebir's id
/// is derived from its two people and its `bb-` id space is reserved, so a
/// birebir is never renamed and no loca is renamed into that space.
pub(super) const ROOM_TABLES_NEVER_RENAMED: &[&str] = &["birebirs"];

/// `attention:{from}:…` → `attention:{to}:…`; `None` when `id` does not name
/// `from`. Callers apply it only to a signal whose origin loca is `from`, so a
/// loca whose name merely starts the same way is never touched.
fn renamed_attention_ref(id: &str, from: &str, to: &str) -> Option<String> {
    id.strip_prefix(&format!("attention:{from}:"))
        .map(|suffix| format!("attention:{to}:{suffix}"))
}

/// Move every reference to the loca `from` inside one care signal to `to`:
/// delivery room, source room, the attention id and group that embed the
/// origin loca, and the context messages. Returns whether anything changed.
fn rename_in_care_signal(signal: &mut protocol::CareSignal, from: &str, to: &str) -> bool {
    let from_origin = signal.origin_room() == from;
    let mut changed = false;
    if signal.room == from {
        signal.room = to.to_string();
        changed = true;
    }
    if signal.source_room == from {
        signal.source_room = to.to_string();
        changed = true;
    }
    if from_origin {
        if let Some(next) = renamed_attention_ref(&signal.attention_id, from, to) {
            signal.attention_id = next;
            changed = true;
        }
        if let Some(next) = signal
            .group
            .as_deref()
            .and_then(|group| renamed_attention_ref(group, from, to))
        {
            signal.group = Some(next);
            changed = true;
        }
    }
    for message in &mut signal.context {
        if message.room == from {
            message.room = to.to_string();
            changed = true;
        }
    }
    changed
}

impl Store {
    /// Atomically move every durable record from one loca name to another.
    /// Re-running after a successful migration is a no-op; merging two
    /// existing locas is refused because their histories must never blur.
    pub fn rename_room(&self, from: &str, to: &str) -> rusqlite::Result<bool> {
        let Some(mut c) = self.conn() else {
            return Ok(false);
        };
        const TABLES: &[&str] = ROOM_KEYED_TABLES;
        // A loca recorded in a never-renamed table (a birebir) is neither the
        // source nor the target of a rename, and its id space is reserved.
        let mut never = from.starts_with("bb-") || to.starts_with("bb-");
        for table in ROOM_TABLES_NEVER_RENAMED {
            let sql = format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE room = ?1 OR room = ?2)");
            never |= c.query_row(&sql, params![from, to], |row| row.get::<_, bool>(0))?;
        }
        if never {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "cannot rename '{from}' to '{to}': a birebir is never renamed and its id space is reserved"
            )));
        }
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
        // A care signal names locas in several places: where its owner
        // receives it (`room`), where it came from when it was re-homed
        // (`source_room`), the attention id and group that embed the origin
        // loca, and the context messages it carries. The outbox row repeats
        // the delivery room and the attention id beside the JSON. All of them
        // move together and independently — a relay delivered in İye about
        // the renamed loca, and a relay delivered in the renamed loca about
        // another one, both stay whole. The delivery id is never changed, so
        // a late ACK remains valid.
        let parse = |column: usize, json: &str| -> rusqlite::Result<protocol::CareSignal> {
            serde_json::from_str(json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    column,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })
        };
        let render = |signal: &protocol::CareSignal| -> rusqlite::Result<String> {
            serde_json::to_string(signal)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
        };
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
                let mut signal = parse(3, &json)?;
                let from_origin = signal.origin_room() == from;
                let mut changed = rename_in_care_signal(&mut signal, from, to);
                let next_attention_id = if from_origin {
                    renamed_attention_ref(&attention_id, from, to).unwrap_or(attention_id.clone())
                } else {
                    attention_id.clone()
                };
                changed |= next_attention_id != attention_id;
                let next_delivery = if delivery_room == from {
                    changed = true;
                    to.to_string()
                } else {
                    delivery_room
                };
                if changed {
                    updates.push((id, next_attention_id, next_delivery, render(&signal)?));
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
        // The ledger files a signal under its origin loca (the `room` column,
        // already moved above) while its JSON may name the renamed loca as
        // the delivery room instead, so every row is read, not only the
        // renamed loca's own.
        let attention_updates = {
            let mut stmt = tx.prepare("SELECT id, signal FROM attentions")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            let mut updates = Vec::new();
            for row in rows {
                let (id, json) = row?;
                let mut signal = parse(1, &json)?;
                let from_origin = signal.origin_room() == from;
                let changed = rename_in_care_signal(&mut signal, from, to);
                let next_id = if from_origin {
                    renamed_attention_ref(&id, from, to).unwrap_or(id.clone())
                } else {
                    id.clone()
                };
                if changed || next_id != id {
                    updates.push((id, next_id, render(&signal)?));
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
    pub fn sealed_rooms(&self) -> rusqlite::Result<Vec<String>> {
        let Some(c) = self.conn() else {
            return Ok(Vec::new());
        };
        let mut stmt = c.prepare("SELECT room FROM rooms WHERE sealed_at IS NOT NULL")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect()
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
        let tx = c.transaction()?;
        Self::write_room_state(&tx, room, mode, settings)?;
        if settings.archived {
            Self::pause_room_attentions(&tx, room, current_time_ms())?;
        }
        tx.commit()
            .inspect_err(|e| tracing::error!(error = %e, "save_room failed"))
    }

    /// Save a loca's state together with the Journal line that records the
    /// change: both are written or neither is, so a change the loca's people
    /// are told about and the change that took effect cannot drift apart.
    pub fn save_room_with_journal(
        &self,
        room: &str,
        mode: &ChatMode,
        settings: &RoomSettings,
        entry: &protocol::JournalEntry,
    ) -> rusqlite::Result<()> {
        let Some(mut c) = self.conn() else {
            return Ok(());
        };
        let tx = c.transaction()?;
        Self::write_room_state(&tx, room, mode, settings)?;
        if settings.archived {
            Self::pause_room_attentions(&tx, room, current_time_ms())?;
        }
        Self::write_journal(&tx, entry)?;
        tx.commit()
            .inspect_err(|e| tracing::error!(error = %e, "save_room_with_journal failed"))
    }

    pub(super) fn require_unsealed_room(c: &Connection, room: &str) -> rusqlite::Result<()> {
        let sealed: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM rooms WHERE room = ?1 AND sealed_at IS NOT NULL)",
            params![room],
            |row| row.get(0),
        )?;
        if sealed {
            return Err(rusqlite::Error::InvalidParameterName(
                "room is permanently sealed".into(),
            ));
        }
        Ok(())
    }

    pub(super) fn write_room_state(
        c: &Connection,
        room: &str,
        mode: &ChatMode,
        settings: &RoomSettings,
    ) -> rusqlite::Result<()> {
        let mode_j = serde_json::to_string(mode).unwrap_or_else(|_| "{\"mode\":\"free\"}".into());
        let set_j = serde_json::to_string(settings).unwrap_or_default();
        let changed = c.execute(
            "INSERT INTO rooms (room, mode, settings) VALUES (?1, ?2, ?3)
             ON CONFLICT(room) DO UPDATE SET mode = excluded.mode, settings = excluded.settings
             WHERE rooms.sealed_at IS NULL",
            params![room, mode_j, set_j],
        )?;
        if changed == 0 {
            return Err(rusqlite::Error::InvalidParameterName(
                "room is permanently sealed".into(),
            ));
        }
        Ok(())
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
    pub fn load(&self) -> rusqlite::Result<Vec<RoomSnapshot>> {
        let Some(c) = self.conn() else {
            return Ok(Vec::new());
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
            let mut stmt = c.prepare(&sql)?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            for r in rows {
                rooms.insert(r?);
            }
        }

        // Older Buildings could issue a davet before any other room state was
        // written. Preserve those existing locas at boot; a new HTTP/WS entry
        // must still use explicit creation. This does not appoint an Operator.
        let mut stmt = c.prepare("SELECT DISTINCT room FROM invites WHERE revoked_at IS NULL")?;
        for room in stmt.query_map([], |row| row.get::<_, String>(0))? {
            rooms.insert(room?);
        }

        // Sealed locas keep their rows (history is preserved) but must NOT come
        // back as live rooms — they are re-tombstoned instead (see hub boot).
        // Query through the connection we already hold: calling `sealed_rooms()`
        // here would take the store lock a SECOND time and deadlock (`conn()`
        // returns a guard).
        let mut sealed: std::collections::HashSet<String> = Default::default();
        let mut stmt = c.prepare("SELECT room FROM rooms WHERE sealed_at IS NOT NULL")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        for room in rows {
            sealed.insert(room?);
        }

        rooms
            .into_iter()
            .filter(|room| !sealed.contains(room))
            .map(|room| self.load_room(&c, &room))
            .collect()
    }
    fn load_room(&self, c: &Connection, room: &str) -> rusqlite::Result<RoomSnapshot> {
        // messages
        let mut messages = Vec::new();
        let mut max_msg_id = 0u64;
        // Hot context only: the archive can be huge, memory holds the tail.
        {
            let mut stmt = c.prepare(
                "SELECT id, sender, sender_type, target, text, reply_to, ts, kind, attachments FROM
               (SELECT * FROM messages WHERE room = ?1 ORDER BY id DESC LIMIT 200)
             ORDER BY id",
            )?;
            let rows = stmt.query_map(params![room], |r| {
                Ok(Message {
                    attachments: attachments_from_json(r.get::<_, Option<String>>(8)?),
                    id: r.get(0)?,
                    room: room.to_string(),
                    sender: r.get(1)?,
                    sender_type: parse_sender_type(&r.get::<_, String>(2)?),
                    target: r.get(3)?,
                    text: r.get(4)?,
                    reply_to: r.get(5)?,
                    reply_to_sender: None,
                    ts: r.get(6)?,
                    kind: parse_kind(&r.get::<_, String>(7)?),
                })
            })?;
            for m in rows {
                let m = m?;
                max_msg_id = max_msg_id.max(m.id);
                messages.push(m);
            }
        }

        // notes
        let mut notes = Vec::new();
        let mut max_rev = 0u64;
        {
            let mut stmt = c.prepare(
            "SELECT key, title, body, can_write, updated_by, updated_at, rev FROM notes WHERE room = ?1",
        )?;
            let rows = stmt.query_map(params![room], |r| {
                let cw: String = r.get(3)?;
                Ok(Note {
                    key: r.get(0)?,
                    title: r.get(1)?,
                    body: r.get(2)?,
                    can_write: serde_json::from_str(&cw).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            3,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?,
                    updated_by: r.get(4)?,
                    updated_at: r.get(5)?,
                    rev: r.get(6)?,
                })
            })?;
            for n in rows {
                let n = n?;
                max_rev = max_rev.max(n.rev);
                notes.push(n);
            }
        }

        // The id counter must resume past the ARCHIVE's max, not the tail's.
        let mx = c.query_row(
            "SELECT COALESCE(MAX(id), 0) FROM messages WHERE room = ?1",
            params![room],
            |r| r.get::<_, u64>(0),
        )?;
        max_msg_id = max_msg_id.max(mx);

        // mode + settings
        let (mode, settings) = c
            .query_row(
                "SELECT mode, settings FROM rooms WHERE room = ?1",
                params![room],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?
            .map(|(m, s)| {
                let parse_error = |e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                };
                Ok::<_, rusqlite::Error>((
                    serde_json::from_str(&m).map_err(parse_error)?,
                    serde_json::from_str(&s).map_err(parse_error)?,
                ))
            })
            .transpose()?
            .unwrap_or((ChatMode::Free, RoomSettings::default()));

        Ok(RoomSnapshot {
            room: room.to_string(),
            messages,
            notes,
            mode,
            settings,
            max_msg_id,
            max_rev,
        })
    }
}
