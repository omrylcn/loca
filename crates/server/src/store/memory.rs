use super::*;

pub const SHORT_MEMORY_MAX_BYTES: usize = 4 * 1024;
pub const LONG_MEMORY_ENTRY_MAX_BYTES: usize = 8 * 1024;
pub const LONG_MEMORY_SOFT_BYTES: usize = 32 * 1024;
pub const LONG_MEMORY_HARD_BYTES: usize = 64 * 1024;

impl Store {
    pub fn loca_memory_version(&self, room: &str) -> rusqlite::Result<u64> {
        let Some(c) = self.conn() else {
            return Ok(0);
        };
        Ok(c.query_row(
            "SELECT version FROM loca_memory WHERE room = ?1",
            params![room],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(0))
    }

    pub fn loca_memory(&self, room: &str) -> rusqlite::Result<Option<protocol::LocaMemory>> {
        let Some(c) = self.conn() else {
            return Ok(None);
        };
        c.query_row(
            "SELECT owner, short, long, short_updated_at, long_updated_at, version
             FROM loca_memory WHERE room = ?1",
            params![room],
            |row| {
                let long: String = row.get(2)?;
                Ok(protocol::LocaMemory {
                    room: room.to_string(),
                    owner: row.get(0)?,
                    short: row.get(1)?,
                    over_budget: long.len() > LONG_MEMORY_SOFT_BYTES,
                    long,
                    short_updated_at: row.get(3)?,
                    long_updated_at: row.get(4)?,
                    version: row.get(5)?,
                })
            },
        )
        .optional()
    }

    pub fn memory_persistence_available(&self) -> bool {
        self.conn.is_some()
    }

    pub fn loca_memory_snapshot(
        &self,
        room: &str,
    ) -> rusqlite::Result<Option<protocol::LocaMemorySnapshot>> {
        let Some(mut memory) = self.loca_memory(room)? else {
            return Ok(None);
        };
        let full_long = memory.long.clone();
        let full_long_bytes = full_long.len();
        let Some(c) = self.conn() else {
            return Ok(None);
        };
        let mut stmt =
            c.prepare("SELECT id, text FROM loca_memory_entries WHERE room = ?1 ORDER BY id DESC")?;
        let entries = stmt
            .query_map(params![room], |row| {
                Ok((row.get::<_, u64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let flattened = entries
            .iter()
            .rev()
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        let provenance_inconsistent = flattened != full_long;

        let mut selected = Vec::new();
        let mut selected_bytes = 0usize;
        let mut suffix_closed = false;
        let mut uninjectable_ids = Vec::new();
        for (id, text) in &entries {
            if text.len() > LONG_MEMORY_ENTRY_MAX_BYTES {
                uninjectable_ids.push(*id);
                continue;
            }
            if suffix_closed {
                continue;
            }
            // The canonical separator costs two bytes only BETWEEN selected
            // entries. A single exactly-8192-byte entry therefore fits.
            let separator_bytes = usize::from(!selected.is_empty()) * 2;
            let candidate_bytes = selected_bytes + separator_bytes + text.len();
            if candidate_bytes <= LONG_MEMORY_ENTRY_MAX_BYTES {
                selected.push(text.as_str());
                selected_bytes = candidate_bytes;
            } else {
                suffix_closed = true;
            }
        }
        selected.reverse();
        memory.long = selected.join("\n\n");
        let long_omitted_entries = entries.len().saturating_sub(selected.len());
        let long_omitted_bytes = full_long_bytes.saturating_sub(memory.long.len());
        Ok(Some(protocol::LocaMemorySnapshot {
            memory,
            provenance_inconsistent,
            long_truncated: long_omitted_bytes > 0,
            long_omitted_bytes,
            long_omitted_entries,
            long_uninjectable_entry_ids: uninjectable_ids,
        }))
    }

    pub fn loca_memory_metadata(&self) -> rusqlite::Result<Vec<protocol::LocaMemoryMetadata>> {
        let Some(c) = self.conn() else {
            return Ok(Vec::new());
        };
        let mut stmt = c.prepare(
            "SELECT m.room, m.owner, m.short_updated_at, m.long_updated_at,
                    (SELECT count(*) FROM loca_memory_entries e
                     WHERE e.room = m.room AND (trim(e.decided_by) = '' OR e.decided_at = 0)),
                    length(CAST(m.long AS BLOB)) > ?1
             FROM loca_memory m
             ORDER BY m.room",
        )?;
        let rows = stmt.query_map(params![LONG_MEMORY_SOFT_BYTES], |row| {
            Ok(protocol::LocaMemoryMetadata {
                loca: row.get(0)?,
                owner: row.get(1)?,
                short_updated_at: row.get(2)?,
                long_updated_at: row.get(3)?,
                long_entries_without_provenance: row.get(4)?,
                over_budget: row.get(5)?,
            })
        })?;
        rows.collect()
    }

    pub fn loca_memory_entries(
        &self,
        room: &str,
        after_id: u64,
        limit: usize,
    ) -> Result<protocol::LocaMemoryEntryPage, MemoryReadError> {
        let Some(c) = self.conn() else {
            return Err(MemoryReadError::PersistenceUnavailable);
        };
        let long = c
            .query_row(
                "SELECT long FROM loca_memory WHERE room = ?1",
                params![room],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|_| MemoryReadError::Storage)?
            .ok_or(MemoryReadError::NotConfigured)?;

        // `long` is the product's bounded injection view; entries are its
        // provenance view. Refuse to serve two silently divergent truths.
        let flattened = {
            let mut stmt = c
                .prepare("SELECT text FROM loca_memory_entries WHERE room = ?1 ORDER BY id ASC")
                .map_err(|_| MemoryReadError::Storage)?;
            let rows = stmt
                .query_map(params![room], |row| row.get::<_, String>(0))
                .map_err(|_| MemoryReadError::Storage)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|_| MemoryReadError::Storage)?
                .join("\n\n")
        };
        if flattened != long {
            return Err(MemoryReadError::InvariantViolation);
        }

        let fetch_limit = limit.saturating_add(1);
        let mut stmt = c
            .prepare(
                "SELECT id, text, decided_by, decided_at
                 FROM loca_memory_entries
                 WHERE room = ?1 AND id > ?2
                 ORDER BY id ASC
                 LIMIT ?3",
            )
            .map_err(|_| MemoryReadError::Storage)?;
        let rows = stmt
            .query_map(params![room, after_id, fetch_limit as u64], |row| {
                let decided_by: String = row.get(2)?;
                let decided_at: u64 = row.get(3)?;
                Ok(protocol::LocaMemoryEntryProvenance {
                    id: row.get(0)?,
                    text: row.get(1)?,
                    decided_by: (!decided_by.trim().is_empty()).then_some(decided_by),
                    decided_at: (decided_at != 0).then_some(decided_at),
                })
            })
            .map_err(|_| MemoryReadError::Storage)?;
        let mut entries = rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| MemoryReadError::Storage)?;
        let has_more = entries.len() > limit;
        entries.truncate(limit);
        let next_after_id = has_more.then(|| entries.last().expect("non-empty page").id);
        Ok(protocol::LocaMemoryEntryPage {
            entries,
            next_after_id,
        })
    }

    pub fn set_memory_owner(&self, room: &str, owner: Option<&str>) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else { return Ok(()) };
        c.execute(
            "INSERT INTO loca_memory (room, owner, version) VALUES (?1, ?2, 1)
             ON CONFLICT(room) DO UPDATE SET
                owner = excluded.owner,
                version = loca_memory.version + 1",
            params![room, owner],
        )
        .map(|_| ())
    }

    fn require_memory_owner(
        tx: &rusqlite::Transaction<'_>,
        room: &str,
        actor: &str,
    ) -> Result<(), MemoryWriteError> {
        let owner = tx
            .query_row(
                "SELECT owner FROM loca_memory WHERE room = ?1",
                params![room],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(|_| MemoryWriteError::Storage)?
            .flatten();
        match owner {
            None => Err(MemoryWriteError::OwnerUnassigned),
            Some(owner) if owner != actor => Err(MemoryWriteError::NotOwner),
            Some(_) => Ok(()),
        }
    }

    pub fn write_short_memory(
        &self,
        room: &str,
        actor: &str,
        text: &str,
        at: u64,
    ) -> Result<protocol::LocaMemory, MemoryWriteError> {
        let Some(mut c) = self.conn() else {
            return Err(MemoryWriteError::PersistenceUnavailable);
        };
        let tx = c.transaction().map_err(|_| MemoryWriteError::Storage)?;
        Self::require_memory_owner(&tx, room, actor)?;
        if text.len() > SHORT_MEMORY_MAX_BYTES {
            return Err(MemoryWriteError::ShortTooLarge);
        }
        tx.execute(
            "UPDATE loca_memory
             SET short = ?2, short_updated_at = ?3, version = version + 1
             WHERE room = ?1",
            params![room, text, at],
        )
        .map_err(|_| MemoryWriteError::Storage)?;
        tx.commit().map_err(|_| MemoryWriteError::Storage)?;
        drop(c);
        self.loca_memory(room)
            .map_err(|_| MemoryWriteError::Storage)?
            .ok_or(MemoryWriteError::Storage)
    }

    pub fn append_long_memory(
        &self,
        room: &str,
        actor: &str,
        text: &str,
        at: u64,
    ) -> Result<protocol::LocaMemoryEntry, MemoryWriteError> {
        let Some(mut c) = self.conn() else {
            return Err(MemoryWriteError::PersistenceUnavailable);
        };
        let tx = c.transaction().map_err(|_| MemoryWriteError::Storage)?;
        Self::require_memory_owner(&tx, room, actor)?;
        if text.len() > LONG_MEMORY_ENTRY_MAX_BYTES {
            return Err(MemoryWriteError::EntryTooLarge);
        }
        let current_bytes = tx
            .query_row(
                "SELECT length(CAST(long AS BLOB)) FROM loca_memory WHERE room = ?1",
                params![room],
                |row| row.get::<_, usize>(0),
            )
            .map_err(|_| MemoryWriteError::Storage)?;
        let separator_bytes = usize::from(current_bytes > 0) * 2;
        let next_bytes = current_bytes
            .checked_add(separator_bytes)
            .and_then(|bytes| bytes.checked_add(text.len()))
            .ok_or(MemoryWriteError::LongTooLarge)?;
        if next_bytes > LONG_MEMORY_HARD_BYTES {
            return Err(MemoryWriteError::LongTooLarge);
        }
        tx.execute(
            "INSERT INTO loca_memory_entries (room, text, decided_by, decided_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![room, text, actor, at],
        )
        .map_err(|_| MemoryWriteError::Storage)?;
        let id = tx.last_insert_rowid() as u64;
        tx.execute(
            "UPDATE loca_memory
             SET long = CASE WHEN long = '' THEN ?2 ELSE long || '\n\n' || ?2 END,
                 long_updated_at = ?3,
                 version = version + 1
             WHERE room = ?1",
            params![room, text, at],
        )
        .map_err(|_| MemoryWriteError::Storage)?;
        tx.commit().map_err(|_| MemoryWriteError::Storage)?;
        Ok(protocol::LocaMemoryEntry {
            id,
            room: room.to_string(),
            text: text.to_string(),
            decided_by: Some(actor.to_string()),
            decided_at: Some(at),
            over_budget: next_bytes > LONG_MEMORY_SOFT_BYTES,
        })
    }
}
