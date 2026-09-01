use super::*;

impl Store {
    pub fn message_owner(&self, room: &str, message_id: u64) -> rusqlite::Result<Option<String>> {
        let Some(c) = self.conn() else {
            return Ok(None);
        };
        c.query_row(
            "SELECT sender FROM messages WHERE room = ?1 AND id = ?2",
            params![room, message_id],
            |row| row.get(0),
        )
        .optional()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn set_message_reaction(
        &self,
        room: &str,
        message_id: u64,
        principal: &str,
        reactor: &str,
        emoji: &str,
        active: bool,
        at: u64,
    ) -> rusqlite::Result<Vec<MessageReaction>> {
        let Some(mut c) = self.conn() else {
            return Ok(Vec::new());
        };
        let tx = c.transaction()?;
        if active {
            tx.execute(
                "INSERT OR IGNORE INTO message_reactions
                 (room, message_id, principal, reactor, emoji, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![room, message_id, principal, reactor, emoji, at],
            )?;
        } else {
            tx.execute(
                "DELETE FROM message_reactions
                 WHERE room = ?1 AND message_id = ?2 AND principal = ?3 AND emoji = ?4",
                params![room, message_id, principal, emoji],
            )?;
        }
        let result = Self::read_message_reactions(&tx, room, Some(message_id))?;
        tx.commit()?;
        Ok(result)
    }

    pub fn message_reactions(&self, room: &str) -> rusqlite::Result<Vec<MessageReaction>> {
        let Some(c) = self.conn() else {
            return Ok(Vec::new());
        };
        Self::read_message_reactions(&c, room, None)
    }

    fn read_message_reactions(
        c: &Connection,
        room: &str,
        message_id: Option<u64>,
    ) -> rusqlite::Result<Vec<MessageReaction>> {
        let mut stmt = c.prepare(
            "SELECT message_id, emoji, reactor FROM message_reactions
             WHERE room = ?1 AND (?2 IS NULL OR message_id = ?2)
             ORDER BY message_id, emoji, created_at, reactor",
        )?;
        let rows = stmt.query_map(params![room, message_id], |row| {
            Ok((
                row.get::<_, u64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut grouped: Vec<MessageReaction> = Vec::new();
        for row in rows {
            let (mid, emoji, actor) = row?;
            if let Some(last) = grouped
                .last_mut()
                .filter(|r| r.message_id == mid && r.emoji == emoji)
            {
                last.actors.push(actor);
            } else {
                grouped.push(MessageReaction {
                    message_id: mid,
                    emoji,
                    actors: vec![actor],
                });
            }
        }
        Ok(grouped)
    }

    /// Persist a message. Returns the DB result so the caller can refuse to
    /// broadcast something that never landed on disk (PRINCIPLES: "mesaj
    /// kaybolmaz"). Memory-only mode (no connection) is NOT a failure — there
    /// is deliberately no disk, so it returns Ok.
    pub fn insert_message(
        &self,
        m: &Message,
        operation: Option<(&str, &str)>,
    ) -> rusqlite::Result<()> {
        let Some(mut c) = self.conn() else {
            return Ok(());
        };
        let st = sender_type_str(m.sender_type);
        let (principal, op_id) = operation
            .map(|(principal, op_id)| (Some(principal), Some(op_id)))
            .unwrap_or((None, None));
        let tx = c.transaction()?;
        tx.execute(
            "INSERT INTO messages
             (id, room, sender, sender_type, target, text, reply_to, ts, kind, principal, op_id, attachments)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                m.id,
                m.room,
                m.sender,
                st,
                m.target,
                m.text,
                m.reply_to,
                m.ts,
                kind_str(m.kind),
                principal,
                op_id,
                attachments_to_json(&m.attachments)
            ],
        )?;
        Self::reset_silence_care(&tx, &m.room, m.ts)?;
        // Attachment refs land in the SAME transaction as the message row, so a
        // message and its pending→referenced flip commit atomically — a crash
        // can never leave a durable message whose cited blob has no reference.
        Self::write_attachment_refs(&tx, m)?;
        tx.commit()
            .inspect_err(|e| tracing::error!(error = %e, "message + silence reset failed"))
    }
    /// Persist a message and the room state it advances as one commit. Used by
    /// round-robin chat, where accepting a message also moves the turn: a
    /// crash or failed INSERT must not save only one half of that event.
    pub fn insert_message_with_room(
        &self,
        m: &Message,
        mode: &ChatMode,
        settings: &RoomSettings,
        operation: Option<(&str, &str)>,
    ) -> rusqlite::Result<()> {
        let Some(mut c) = self.conn() else {
            return Ok(());
        };
        let mode_j = serde_json::to_string(mode).unwrap_or_else(|_| "{\"mode\":\"free\"}".into());
        let set_j = serde_json::to_string(settings).unwrap_or_default();
        let (principal, op_id) = operation
            .map(|(principal, op_id)| (Some(principal), Some(op_id)))
            .unwrap_or((None, None));
        let tx = c.transaction()?;
        tx.execute(
            "INSERT OR REPLACE INTO rooms (room, mode, settings) VALUES (?1, ?2, ?3)",
            params![m.room, mode_j, set_j],
        )?;
        tx.execute(
            "INSERT INTO messages
             (id, room, sender, sender_type, target, text, reply_to, ts, kind, principal, op_id, attachments)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                m.id,
                m.room,
                m.sender,
                sender_type_str(m.sender_type),
                m.target,
                m.text,
                m.reply_to,
                m.ts,
                kind_str(m.kind),
                principal,
                op_id,
                attachments_to_json(&m.attachments)
            ],
        )?;
        Self::reset_silence_care(&tx, &m.room, m.ts)?;
        // Attachment refs land in the SAME transaction as the message row, so a
        // message and its pending→referenced flip commit atomically — a crash
        // can never leave a durable message whose cited blob has no reference.
        Self::write_attachment_refs(&tx, m)?;
        tx.commit()
            .inspect_err(|e| tracing::error!(error = %e, "message + room state commit failed"))
    }
    /// A direct caretaker summon is part of accepting the message, not a
    /// best-effort side effect. The message, optional room-state transition,
    /// Attention ledger and every outbox delivery commit together.
    pub fn insert_message_with_care(
        &self,
        m: &Message,
        room_state: Option<(&ChatMode, &RoomSettings)>,
        operation: Option<(&str, &str)>,
        delivery_room: &str,
        signals: &[protocol::CareSignal],
    ) -> rusqlite::Result<()> {
        let Some(mut c) = self.conn() else {
            return Ok(());
        };
        let (principal, op_id) = operation
            .map(|(principal, op_id)| (Some(principal), Some(op_id)))
            .unwrap_or((None, None));
        let tx = c.transaction()?;
        if let Some((mode, settings)) = room_state {
            let mode_json =
                serde_json::to_string(mode).unwrap_or_else(|_| "{\"mode\":\"free\"}".into());
            let settings_json = serde_json::to_string(settings).unwrap_or_default();
            tx.execute(
                "INSERT OR REPLACE INTO rooms (room, mode, settings) VALUES (?1, ?2, ?3)",
                params![m.room, mode_json, settings_json],
            )?;
        }
        tx.execute(
            "INSERT INTO messages
             (id, room, sender, sender_type, target, text, reply_to, ts, kind, principal, op_id, attachments)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                m.id,
                m.room,
                m.sender,
                sender_type_str(m.sender_type),
                m.target,
                m.text,
                m.reply_to,
                m.ts,
                kind_str(m.kind),
                principal,
                op_id,
                attachments_to_json(&m.attachments)
            ],
        )?;
        Self::reset_silence_care(&tx, &m.room, m.ts)?;
        // Attachment refs land in the SAME transaction as the message row, so a
        // message and its pending→referenced flip commit atomically — a crash
        // can never leave a durable message whose cited blob has no reference.
        Self::write_attachment_refs(&tx, m)?;
        for signal in signals {
            Self::write_care(&tx, delivery_room, signal)?;
        }
        tx.commit().inspect_err(
            |e| tracing::error!(error = %e, "message + caretaker attention commit failed"),
        )
    }
    /// Re-waking a waiter whose dependency just replied is part of accepting
    /// that reply, not a best-effort side effect. In one transaction: insert
    /// the message (+ optional room-state transition), suppress the wait's
    /// current-generation overdue (and ack its outbox row), advance the wait to
    /// a fresh generation (the wait row STAYS — a reply is progress, not
    /// completion), and persist the durable wake delivery. On any failure the
    /// whole thing rolls back, so the message never lands without the wake nor
    /// the wake without the message.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_message_with_wait_wake(
        &self,
        m: &Message,
        room_state: Option<(&ChatMode, &RoomSettings)>,
        operation: Option<(&str, &str)>,
        delivery_room: &str,
        waiter: &str,
        at: u64,
        wait: &WaitState,
        wake: &protocol::CareSignal,
    ) -> rusqlite::Result<()> {
        let Some(mut c) = self.conn() else {
            return Ok(());
        };
        let (principal, op_id) = operation
            .map(|(principal, op_id)| (Some(principal), Some(op_id)))
            .unwrap_or((None, None));
        let tx = c.transaction()?;
        if let Some((mode, settings)) = room_state {
            let mode_json =
                serde_json::to_string(mode).unwrap_or_else(|_| "{\"mode\":\"free\"}".into());
            let settings_json = serde_json::to_string(settings).unwrap_or_default();
            tx.execute(
                "INSERT OR REPLACE INTO rooms (room, mode, settings) VALUES (?1, ?2, ?3)",
                params![m.room, mode_json, settings_json],
            )?;
        }
        tx.execute(
            "INSERT INTO messages
             (id, room, sender, sender_type, target, text, reply_to, ts, kind, principal, op_id, attachments)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                m.id,
                m.room,
                m.sender,
                sender_type_str(m.sender_type),
                m.target,
                m.text,
                m.reply_to,
                m.ts,
                kind_str(m.kind),
                principal,
                op_id,
                attachments_to_json(&m.attachments)
            ],
        )?;
        Self::reset_silence_care(&tx, &m.room, m.ts)?;
        // Attachment refs land in the SAME transaction as the message row, so a
        // message and its pending→referenced flip commit atomically — a crash
        // can never leave a durable message whose cited blob has no reference.
        Self::write_attachment_refs(&tx, m)?;
        Self::resolve_wait_attentions(&tx, &m.room, waiter, at)?;
        Self::write_wait(&tx, wait)?;
        Self::write_care(&tx, delivery_room, wake)?;
        tx.commit()
            .inspect_err(|e| tracing::error!(error = %e, "message + wait-reply wake commit failed"))
    }
    fn reset_silence_care(c: &Connection, room: &str, at: u64) -> rusqlite::Result<()> {
        let prefix = format!("attention:{room}:silence");
        c.execute(
            "DELETE FROM care_marks WHERE room = ?1 AND signal_key = 'silence'",
            params![room],
        )?;
        c.execute(
            "UPDATE attentions SET resolved_at = COALESCE(resolved_at, ?2)
             WHERE id = ?1 OR substr(id, 1, length(?1) + 1) = ?1 || ':'",
            params![prefix, at],
        )?;
        c.execute(
            "UPDATE care_outbox SET acked_at = COALESCE(acked_at, ?2)
             WHERE attention_id = ?1
                OR substr(attention_id, 1, length(?1) + 1) = ?1 || ':'",
            params![prefix, at],
        )?;
        Ok(())
    }
    pub fn message_by_operation(
        &self,
        room: &str,
        principal: &str,
        op_id: &str,
    ) -> rusqlite::Result<Option<Message>> {
        let Some(c) = self.conn() else {
            return Ok(None);
        };
        c.query_row(
            "SELECT id, sender, sender_type, target, text, reply_to, ts, kind, attachments
             FROM messages WHERE room = ?1 AND principal = ?2 AND op_id = ?3",
            params![room, principal, op_id],
            |r| {
                Ok(Message {
                    attachments: attachments_from_json(r.get::<_, Option<String>>(8)?),
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
            },
        )
        .optional()
        .inspect_err(|e| tracing::error!(error = %e, "message_by_operation failed"))
    }
    /// Read one ordered page from the durable message archive.
    ///
    /// `None` means this Store is intentionally memory-only. Callers may then
    /// fall back to the Hub's bounded hot tail; a persistent Store always
    /// answers from SQLite so reconnect recovery is not capped by that tail.
    pub fn messages_after(
        &self,
        room: &str,
        after_id: u64,
        limit: usize,
    ) -> rusqlite::Result<Option<Vec<Message>>> {
        let Some(c) = self.conn() else {
            return Ok(None);
        };
        let mut stmt = c.prepare(
            "SELECT id, sender, sender_type, target, text, reply_to, ts, kind, attachments
             FROM messages
             WHERE room = ?1 AND id > ?2
             ORDER BY id
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![room, after_id, limit as u64], |r| {
            Ok(Message {
                attachments: attachments_from_json(r.get::<_, Option<String>>(8)?),
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
        })?;
        let messages = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Some(messages))
    }
}
