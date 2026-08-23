use super::*;

impl Store {
    pub(in crate::store) fn resolve_wait_attentions(
        c: &Connection,
        room: &str,
        waiter: &str,
        at: u64,
    ) -> rusqlite::Result<()> {
        let overdue_prefix = format!("attention:{room}:wait:{waiter}");
        let cycle_ids = {
            let mut stmt = c.prepare(
                "SELECT id, signal FROM attentions
                 WHERE room = ?1 AND resolved_at IS NULL",
            )?;
            let rows = stmt.query_map(params![room], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            let mut ids = Vec::new();
            for row in rows {
                let (id, json) = row?;
                let signal: protocol::CareSignal =
                    serde_json::from_str(&json).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            1,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?;
                if signal.reason == protocol::CareReason::WaitCycle
                    && signal.participants.iter().any(|name| name == waiter)
                {
                    ids.push(id);
                }
            }
            ids
        };
        c.execute(
            "UPDATE attentions SET resolved_at = COALESCE(resolved_at, ?2)
             WHERE id = ?1 OR substr(id, 1, length(?1) + 1) = ?1 || ':'",
            params![overdue_prefix, at],
        )?;
        c.execute(
            "UPDATE care_outbox SET acked_at = COALESCE(acked_at, ?2)
             WHERE attention_id = ?1 OR substr(attention_id, 1, length(?1) + 1) = ?1 || ':'",
            params![overdue_prefix, at],
        )?;
        for id in cycle_ids {
            if let Some(signal_key) = id.strip_prefix(&format!("attention:{room}:")) {
                c.execute(
                    "DELETE FROM care_marks WHERE room = ?1 AND signal_key = ?2",
                    params![room, signal_key],
                )?;
            }
            c.execute(
                "UPDATE attentions SET resolved_at = COALESCE(resolved_at, ?2) WHERE id = ?1",
                params![id, at],
            )?;
            c.execute(
                "UPDATE care_outbox SET acked_at = COALESCE(acked_at, ?2)
                 WHERE attention_id = ?1",
                params![id, at],
            )?;
        }
        Ok(())
    }
    pub fn load_care_marks(&self, room: &str) -> Vec<(String, u64, u32)> {
        let Some(c) = self.conn() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if let Ok(mut stmt) = c.prepare(
            "SELECT signal_key, last_signal_at, signal_count
             FROM care_marks WHERE room = ?1",
        ) {
            if let Ok(rows) = stmt.query_map(params![room], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            }) {
                out.extend(rows.flatten());
            }
        }
        out
    }
    pub fn enqueue_care(
        &self,
        delivery_room: &str,
        signal: &protocol::CareSignal,
    ) -> rusqlite::Result<()> {
        let Some(mut c) = self.conn() else {
            return Ok(());
        };
        let tx = c.transaction()?;
        Self::write_care(&tx, delivery_room, signal)?;
        tx.commit()
            .inspect_err(|e| tracing::error!(error = %e, "enqueue attention failed"))
    }
    pub fn enqueue_care_with_mark(
        &self,
        room: &str,
        signal_key: &str,
        last_signal_at: u64,
        signal_count: u32,
        delivery_room: &str,
        signal: &protocol::CareSignal,
    ) -> rusqlite::Result<()> {
        let Some(mut c) = self.conn() else {
            return Ok(());
        };
        let tx = c.transaction()?;
        tx.execute(
            "INSERT OR REPLACE INTO care_marks
             (room, signal_key, last_signal_at, signal_count)
             VALUES (?1, ?2, ?3, ?4)",
            params![room, signal_key, last_signal_at, signal_count],
        )?;
        Self::write_care(&tx, delivery_room, signal)?;
        tx.commit()
            .inspect_err(|e| tracing::error!(error = %e, "care mark + attention commit failed"))
    }
    pub fn enqueue_care_with_waits(
        &self,
        waits: &[WaitState],
        delivery_room: &str,
        signal: &protocol::CareSignal,
    ) -> rusqlite::Result<()> {
        let Some(mut c) = self.conn() else {
            return Ok(());
        };
        let tx = c.transaction()?;
        for wait in waits {
            Self::write_wait(&tx, wait)?;
        }
        Self::write_care(&tx, delivery_room, signal)?;
        tx.commit()
            .inspect_err(|e| tracing::error!(error = %e, "wait + attention commit failed"))
    }
    pub(in crate::store) fn write_care(
        c: &Connection,
        delivery_room: &str,
        signal: &protocol::CareSignal,
    ) -> rusqlite::Result<()> {
        let json = serde_json::to_string(signal)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        let attention_id = if signal.attention_id.is_empty() {
            signal.id.as_str()
        } else {
            signal.attention_id.as_str()
        };
        let writable = c.execute(
            "INSERT INTO attentions
             (id, room, signal, owner, created_at, delivered_at, claimed_by, claimed_at, resolved_at)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL, NULL, NULL)
             ON CONFLICT(id) DO UPDATE SET
                signal = excluded.signal,
                owner = excluded.owner
              WHERE attentions.claimed_at IS NULL AND attentions.resolved_at IS NULL",
            // The attention ledger is keyed to the origin loca; a re-homed
            // caretaker envelope (room == home loca) still files its work under
            // the source loca so GET /rooms/{source}/attentions finds it.
            params![attention_id, signal.origin_room(), json, signal.owner, signal.at],
        )?;
        if writable == 0 {
            // A claim freezes accountability. A later retry may choose another
            // currently healthy runtime, but it must never reassign work that
            // somebody has already accepted (nor enqueue that false delivery).
            return Ok(());
        }
        if let Some(owner) = signal.owner.as_deref() {
            c.execute(
                "INSERT OR IGNORE INTO care_outbox
                 (id, attention_id, delivery_room, owner, signal, created_at, acked_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)",
                params![
                    signal.id,
                    attention_id,
                    delivery_room,
                    owner,
                    json,
                    signal.at
                ],
            )?;
        }
        Ok(())
    }
    fn attention_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Attention> {
        let json: String = row.get(0)?;
        let signal: protocol::CareSignal = serde_json::from_str(&json).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
        let delivered_at: Option<u64> = row.get(1)?;
        let claimed_by: Option<String> = row.get(2)?;
        let claimed_at: Option<u64> = row.get(3)?;
        let resolved_at: Option<u64> = row.get(4)?;
        let created_at: u64 = row.get(5)?;
        let status = if resolved_at.is_some() {
            AttentionStatus::Resolved
        } else if claimed_at.is_some() {
            AttentionStatus::Claimed
        } else {
            AttentionStatus::Open
        };
        // The attention belongs to the origin loca even when the envelope was
        // re-homed for cross-loca delivery.
        let origin_room = signal.origin_room().to_string();
        Ok(Attention {
            id: if signal.attention_id.is_empty() {
                signal.id
            } else {
                signal.attention_id
            },
            room: origin_room,
            reason: signal.reason,
            subject: signal.subject,
            audience: signal.audience,
            owner: signal.owner,
            participants: signal.participants,
            created_by: signal.created_by,
            created_at,
            attempt: signal.attempt,
            escalated: signal.escalated,
            status,
            delivered_at,
            claimed_by,
            claimed_at,
            resolved_at,
        })
    }
    pub fn attentions(&self, room: &str) -> Vec<Attention> {
        let Some(c) = self.conn() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if let Ok(mut stmt) = c.prepare(
            "SELECT signal, delivered_at, claimed_by, claimed_at, resolved_at, created_at
             FROM attentions WHERE room = ?1 ORDER BY created_at, id",
        ) {
            if let Ok(rows) = stmt.query_map(params![room], Self::attention_from_row) {
                out.extend(rows.flatten());
            }
        }
        out
    }
    pub fn attention(&self, id: &str) -> Option<Attention> {
        let c = self.conn()?;
        c.query_row(
            "SELECT signal, delivered_at, claimed_by, claimed_at, resolved_at, created_at
             FROM attentions WHERE id = ?1",
            params![id],
            Self::attention_from_row,
        )
        .optional()
        .ok()
        .flatten()
    }
    pub fn claim_attention(&self, id: &str, by: &str, at: u64) -> rusqlite::Result<bool> {
        let Some(c) = self.conn() else {
            return Ok(true);
        };
        c.execute(
            "UPDATE attentions SET claimed_by = ?2, claimed_at = ?3
             WHERE id = ?1 AND claimed_at IS NULL AND resolved_at IS NULL",
            params![id, by, at],
        )
        .map(|changed| changed > 0)
    }
    pub fn resolve_attention(&self, id: &str, at: u64) -> rusqlite::Result<bool> {
        let Some(mut c) = self.conn() else {
            return Ok(true);
        };
        let tx = c.transaction()?;
        let changed = tx.execute(
            "UPDATE attentions SET resolved_at = ?2
             WHERE id = ?1 AND resolved_at IS NULL",
            params![id, at],
        )?;
        if changed > 0 {
            tx.execute(
                "UPDATE care_outbox SET acked_at = COALESCE(acked_at, ?2)
                 WHERE attention_id = ?1",
                params![id, at],
            )?;
        }
        tx.commit()?;
        Ok(changed > 0)
    }
    pub fn pending_care(&self, delivery_room: &str, owner: &str) -> Vec<protocol::CareSignal> {
        let Some(c) = self.conn() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        // Never replay a wake whose backing attention is already Resolved: the
        // condition is settled, so re-delivering it on reconnect would be a
        // stale nudge. A missing attention row (legacy/no-op writes) still
        // replays — its receipt is the only durable state.
        if let Ok(mut stmt) = c.prepare(
            "SELECT o.signal FROM care_outbox o
             LEFT JOIN attentions a ON a.id = o.attention_id
             WHERE o.delivery_room = ?1 AND o.owner = ?2 AND o.acked_at IS NULL
               AND (a.id IS NULL OR a.resolved_at IS NULL)
             ORDER BY o.created_at, o.id",
        ) {
            if let Ok(rows) = stmt.query_map(params![delivery_room, owner], |row| {
                let json: String = row.get(0)?;
                serde_json::from_str::<protocol::CareSignal>(&json).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })
            }) {
                // Invariant belt-and-suspenders: a socket bound to
                // `delivery_room` must only ever receive envelopes homed to that
                // room. delivery_room is derived from signal.room on write, but
                // refuse any legacy/divergent row here too.
                out.extend(rows.flatten().filter(|signal| signal.room == delivery_room));
            }
        }
        out
    }
    pub fn attention_id_for_delivery(&self, delivery_id: &str) -> Option<String> {
        let c = self.conn()?;
        c.query_row(
            "SELECT attention_id FROM care_outbox WHERE id = ?1",
            params![delivery_id],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten()
    }
    pub fn ack_care(&self, id: &str, owner: &str, at: u64) -> rusqlite::Result<bool> {
        let Some(mut c) = self.conn() else {
            // The Hub validates and records hot delivery receipts in
            // memory-only mode. A no-op store must never authenticate an
            // arbitrary receipt by itself.
            return Ok(false);
        };
        let tx = c.transaction()?;
        let attention_id: Option<String> = tx
            .query_row(
                "SELECT attention_id FROM care_outbox
                 WHERE id = ?1 AND owner = ?2 AND acked_at IS NULL",
                params![id, owner],
                |row| row.get(0),
            )
            .optional()?;
        let changed = tx.execute(
            "UPDATE care_outbox SET acked_at = ?3
             WHERE id = ?1 AND owner = ?2 AND acked_at IS NULL",
            params![id, owner, at],
        )?;
        if changed > 0 {
            tx.execute(
                "UPDATE attentions SET delivered_at = COALESCE(delivered_at, ?2)
                 WHERE id = ?1",
                params![attention_id.as_deref().unwrap_or(id), at],
            )?;
        }
        tx.commit()
            .inspect_err(|e| tracing::error!(error = %e, "ack_care failed"))?;
        Ok(changed > 0)
    }
}
