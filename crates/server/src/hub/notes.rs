//! Notes: a loca's editable current facts.

use super::{CreateNote, Hub, Note, NoteError, RecoverMutex, ServerFrame, UpdateNote};

impl Hub {
    /// All notes in a room (sorted by key for stable display).
    pub fn notes(&self, room: &str) -> Vec<Note> {
        let rooms = self.rooms.lock_or_recover();
        let mut list: Vec<Note> = rooms
            .get(room)
            .map(|r| r.notes.values().cloned().collect())
            .unwrap_or_default();
        list.sort_by(|a, b| a.key.cmp(&b.key));
        list
    }

    /// One note by key.
    pub fn note(&self, room: &str, key: &str) -> Option<Note> {
        let rooms = self.rooms.lock_or_recover();
        rooms.get(room).and_then(|r| r.notes.get(key).cloned())
    }

    /// Create a note. Errors if the key already exists (use `update_note`).
    pub fn create_note(&self, room: &str, req: CreateNote) -> Result<Note, NoteError> {
        let mut rooms = self.rooms.lock_or_recover();
        if rooms
            .get(room)
            .is_some_and(|r| r.notes.contains_key(&req.key))
        {
            return Err(NoteError::Exists);
        }
        let rev = rooms.get(room).map(|r| r.next_rev).unwrap_or(1);
        let note = Note {
            key: req.key.clone(),
            title: req.title,
            body: req.body,
            can_write: req.can_write,
            updated_by: req.by,
            updated_at: (self.now_ms)(),
            rev,
        };
        self.store
            .upsert_note(room, &note)
            .map_err(|_| NoteError::Storage)?;
        let r = Self::room_mut(&mut rooms, room, &self.default_settings);
        r.next_rev = rev + 1;
        r.notes.insert(req.key, note.clone());
        let _ = r.tx.send(ServerFrame::Note { note: note.clone() });
        Ok(note)
    }

    /// Update an existing note in place. Errors if it doesn't exist (use
    /// `create_note`). Soft permissions: an unassigned writer still succeeds,
    /// but a `NoteWarn` frame is broadcast. `can_write` is only reassigned when
    /// `is_operator` — derived by the caller from the admin token, never from
    /// the request body.
    pub fn update_note(
        &self,
        room: &str,
        key: &str,
        req: UpdateNote,
        is_operator: bool,
    ) -> Result<Note, NoteError> {
        let mut rooms = self.rooms.lock_or_recover();
        let r = rooms.get_mut(room).ok_or(NoteError::NotFound)?;
        // (note: room must already exist; we don't create on update)
        let previous = r.notes.get(key).cloned().ok_or(NoteError::NotFound)?;
        let mut note = previous.clone();
        // Direction 3 (team memory): archive the version being replaced, so
        // "when and by whom did this change" stays answerable forever.

        // Soft-permission check (advisory).
        let allowed = note.can_write.is_empty() || note.can_write.contains(&req.by);
        let warn = if allowed {
            None
        } else {
            Some(ServerFrame::NoteWarn {
                key: key.to_string(),
                by: req.by.clone(),
                can_write: note.can_write.clone(),
            })
        };

        if let Some(t) = req.title {
            note.title = t;
        }
        if let Some(b) = req.body {
            note.body = b;
        }
        // Only the operator may reassign who can write.
        if is_operator {
            if let Some(cw) = req.can_write {
                note.can_write = cw;
            }
        }
        note.updated_by = req.by;
        note.updated_at = (self.now_ms)();
        note.rev = r.next_rev;
        let updated = note;

        self.store
            .replace_note(room, &previous, &updated)
            .map_err(|_| NoteError::Storage)?;
        r.next_rev += 1;
        r.notes.insert(key.to_string(), updated.clone());
        let _ = r.tx.send(ServerFrame::Note {
            note: updated.clone(),
        });
        if let Some(w) = warn {
            let _ = r.tx.send(w);
        }
        Ok(updated)
    }

    /// A note's archived past versions (newest first).
    pub fn note_history(&self, room: &str, key: &str) -> rusqlite::Result<Vec<Note>> {
        self.store.note_history(room, key)
    }

    /// Delete a note by key. Returns whether it existed. Broadcasts a
    /// tombstone note (empty body, key prefixed) is overkill for v0 — clients
    /// simply refetch on the next `note` frame; here we just drop it and push a
    /// control so watchers can refresh.
    pub fn delete_note(&self, room: &str, key: &str) -> rusqlite::Result<bool> {
        let mut rooms = self.rooms.lock_or_recover();
        let Some(r) = rooms.get_mut(room) else {
            return Ok(false);
        };
        if !r.notes.contains_key(key) {
            return Ok(false);
        }
        self.store.delete_note(room, key)?;
        r.notes.remove(key);
        let _ = r.tx.send(ServerFrame::Control {
            cmd: format!("note-deleted:{key}"),
        });
        Ok(true)
    }
}
