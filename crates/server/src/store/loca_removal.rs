//! Deleting a loca's records, and the archive of them the Master may take
//! first.
//!
//! Sealing keeps every record (`seal_room`). Deleting is the Master's other
//! choice for a closed loca: every row the loca owns, in every table of
//! `ROOM_KEYED_TABLES`, is removed, and an attachment file no other loca
//! cites goes with it. The `rooms` row stays, emptied and sealed, so the name
//! is never quietly re-created by a late join or post (as after a seal).

use super::rooms::ROOM_KEYED_TABLES;
use super::{params, Store};

/// Loca tables an archive leaves out: credentials and secrets (invites, the
/// vault), and runtime bookkeeping that means nothing outside the server.
/// Everything else in `ROOM_KEYED_TABLES` is archived as it is stored.
pub(crate) const NOT_ARCHIVED: &[&str] = &[
    "invites",
    "loca_vault",
    "attentions",
    "care_marks",
    "attachment_pending",
    "loca_wiki_batches",
    "loca_wiki_receipts",
    "wiki_native_runtime",
    "wiki_context_selection",
    "wiki_context_receipts",
];

/// One cited attachment: its file id and the name it was sent under.
pub(crate) struct ArchivedAttachment {
    pub sha: String,
    pub name: String,
}

/// A loca's records as the archive holds them.
pub(crate) struct LocaRecords {
    /// `(table, rows)`, each row an object of its stored columns.
    pub tables: Vec<(&'static str, Vec<serde_json::Value>)>,
    pub attachments: Vec<ArchivedAttachment>,
}

fn cell(value: rusqlite::types::ValueRef<'_>) -> serde_json::Value {
    use rusqlite::types::ValueRef;
    match value {
        ValueRef::Null => serde_json::Value::Null,
        ValueRef::Integer(n) => n.into(),
        ValueRef::Real(x) => x.into(),
        ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned().into(),
        // No loca table stores bytes in a row; say so rather than guess.
        ValueRef::Blob(b) => format!("<{} bytes>", b.len()).into(),
    }
}

impl Store {
    /// Every archived row the loca `room` owns, and the attachments its
    /// messages cite. A loca with no rows answers empty tables.
    pub(crate) fn loca_records(&self, room: &str) -> rusqlite::Result<LocaRecords> {
        let Some(c) = self.conn() else {
            return Ok(LocaRecords {
                tables: Vec::new(),
                attachments: Vec::new(),
            });
        };
        let mut tables = Vec::new();
        for table in ROOM_KEYED_TABLES.iter().copied() {
            if NOT_ARCHIVED.contains(&table) {
                continue;
            }
            let mut stmt = c.prepare(&format!("SELECT * FROM {table} WHERE room = ?1"))?;
            let names: Vec<String> = stmt.column_names().map(String::from).collect();
            let rows = stmt.query_map(params![room], |row| {
                let mut object = serde_json::Map::new();
                for (i, name) in names.iter().enumerate() {
                    object.insert(name.clone(), cell(row.get_ref(i)?));
                }
                Ok(serde_json::Value::Object(object))
            })?;
            tables.push((table, rows.collect::<rusqlite::Result<Vec<_>>>()?));
        }
        let attachments = {
            let mut stmt = c.prepare(
                "SELECT sha, MIN(name) FROM attachment_refs WHERE room = ?1 GROUP BY sha ORDER BY sha",
            )?;
            let rows = stmt.query_map(params![room], |row| {
                Ok(ArchivedAttachment {
                    sha: row.get(0)?,
                    name: row.get(1)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        Ok(LocaRecords {
            tables,
            attachments,
        })
    }

    /// The bytes of one attachment file, checked against its id; `None` when
    /// attachments are off or the file is gone.
    pub(crate) fn attachment_bytes(&self, sha: &str) -> Option<Vec<u8>> {
        self.blobs.as_ref()?.read(sha).ok()
    }

    /// Delete every record of the loca `room` in one transaction, leave its
    /// `rooms` row empty and sealed, then collect the attachment files no
    /// other loca cites. Returns how many rows were deleted.
    pub(crate) fn delete_loca_records(&self, room: &str, at: u64) -> rusqlite::Result<usize> {
        let deleted = {
            let Some(mut c) = self.conn() else {
                return Ok(0);
            };
            let tx = c.transaction()?;
            // A care signal from this loca carries copies of its messages,
            // wherever it was delivered: the outbox rows and the attention
            // rows that came from it go first, while their ids still resolve.
            let origin = format!("attention:{room}:");
            let mut deleted = tx.execute(
                "DELETE FROM care_outbox WHERE delivery_room = ?1
                    OR attention_id IN (SELECT id FROM attentions WHERE room = ?1)
                    OR substr(attention_id, 1, length(?2)) = ?2",
                params![room, origin],
            )?;
            deleted += tx.execute(
                "DELETE FROM attentions WHERE substr(id, 1, length(?1)) = ?1",
                params![origin],
            )?;
            // Rows that cite a message first, the messages after them.
            for table in ROOM_KEYED_TABLES
                .iter()
                .filter(|t| !matches!(**t, "rooms" | "messages"))
                .chain(["messages"].iter())
            {
                deleted += tx.execute(
                    &format!("DELETE FROM {table} WHERE room = ?1"),
                    params![room],
                )?;
            }
            tx.execute(
                "INSERT OR IGNORE INTO rooms (room, mode, settings) VALUES (?1, '{\"mode\":\"free\"}', '{}')",
                params![room],
            )?;
            tx.execute(
                "UPDATE rooms SET mode = '{\"mode\":\"free\"}', settings = '{}', \
                 sealed_at = COALESCE(sealed_at, ?2) WHERE room = ?1",
                params![room, at as i64],
            )?;
            tx.commit().inspect_err(
                |e| tracing::error!(error = %e, %room, "delete_loca_records failed"),
            )?;
            deleted
        };
        // Pending uploads of other locas are not touched (no age passes the
        // cutoff); only files nothing cites any more are removed.
        self.sweep_attachments(at, u64::MAX);
        Ok(deleted)
    }
}
