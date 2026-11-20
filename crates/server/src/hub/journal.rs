//! The Journal: the append-only record of what landed in a loca.

use super::*;

impl Hub {
    /// Everything recorded in this loca's journal, oldest first.
    pub fn journal(&self, room: &str) -> Vec<protocol::JournalEntry> {
        let rooms = self.rooms.lock_or_recover();
        rooms
            .get(room)
            .map(|r| r.journal.clone())
            .unwrap_or_default()
    }

    /// Record a piece of finished work. Nobody assigns this and nothing closes
    /// it — the writer is simply saying what they did, and it stands.
    pub fn append_journal(
        &self,
        room: &str,
        by: String,
        by_type: SenderType,
        text: String,
    ) -> rusqlite::Result<protocol::JournalEntry> {
        let now = (self.now_ms)();
        let mut rooms = self.rooms.lock_or_recover();
        let id = rooms.get(room).map(|r| r.next_journal_id).unwrap_or(1);
        let entry = protocol::JournalEntry {
            id,
            room: room.to_string(),
            by,
            by_type,
            text,
            at: now,
        };
        self.store.append_journal(&entry)?;
        let r = Self::room_mut(&mut rooms, room, &self.default_settings);
        r.next_journal_id = id + 1;
        r.journal.push(entry.clone());
        // Written always; pushed to open sockets only while the Journal is on
        // in this loca (a line the server writes itself, such as a group
        // move, can land while it is off).
        if r.settings.extension_enabled("journal") {
            let _ = r.tx.send(ServerFrame::Journal {
                entry: entry.clone(),
            });
        }
        Ok(entry)
    }

    // ---- notes (living, keyed project state) ----
}
