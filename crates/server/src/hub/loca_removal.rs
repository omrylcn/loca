//! Deleting a closed loca, with or without its archive first.
//!
//! The Master has two ends for a closed loca. Seal keeps every record
//! (`delete_room`). Delete removes them: messages, notes, tasks, goals,
//! waits, the Journal, the Wiki and the attachment files no other loca
//! cites. Before deleting, the Master may take the loca's archive, a zip of
//! those records: a readable transcript, every table as JSON and the
//! attachment files. Secrets (invites, the vault) are never in it.
//!
//! Like a seal, a delete keeps the name: the loca does not come back
//! empty on a late join or post.

use std::io::Write;

use protocol::ServerFrame;

use super::Hub;
use crate::store::loca_removal::LocaRecords;
use crate::sync::RecoverMutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocaRemovalError {
    NotFound,
    /// Only a closed (or already sealed) loca is deleted.
    NotClosed,
    /// The home loca and Care are never deleted.
    Protected,
    Storage,
}

impl LocaRemovalError {
    pub fn message(self) -> &'static str {
        match self {
            Self::NotFound => "no such loca",
            Self::NotClosed => "close the loca first, then delete it",
            Self::Protected => "this loca cannot be deleted",
            Self::Storage => "could not delete the loca — try again",
        }
    }
}

impl Hub {
    fn protected_from_removal(&self, room: &str) -> bool {
        room == self.home_room() || self.is_reserved_room(room)
    }

    /// The loca's archive as zip bytes. A direct message is not a loca here:
    /// it answers as a name that names nothing, so its existence is not told.
    pub fn loca_archive(&self, room: &str) -> Result<Vec<u8>, LocaRemovalError> {
        if room.starts_with(Self::DM_PREFIX) {
            return Err(LocaRemovalError::NotFound);
        }
        // One lock at a time: `rooms` is never held while `deleted` is taken.
        let sealed = self.is_deleted(room);
        let live = self.rooms.lock_or_recover().contains_key(room);
        if !live && !sealed {
            return Err(LocaRemovalError::NotFound);
        }
        let records = self
            .store
            .loca_records(room)
            .map_err(|_| LocaRemovalError::Storage)?;
        archive_zip(room, (self.now_ms)(), &records, |sha| {
            self.store.attachment_bytes(sha)
        })
        .map_err(|_| LocaRemovalError::Storage)
    }

    /// Delete every record of a closed or sealed loca and keep its name
    /// sealed. Everyone at its table is sent out, as by a seal.
    pub fn delete_loca(&self, room: &str) -> Result<usize, LocaRemovalError> {
        if room.starts_with(Self::DM_PREFIX) {
            return Err(LocaRemovalError::NotFound);
        }
        if self.protected_from_removal(room) {
            return Err(LocaRemovalError::Protected);
        }
        let sealed = self.is_deleted(room);
        let closed = self
            .rooms
            .lock_or_recover()
            .get(room)
            .map(|r| r.settings.archived);
        match closed {
            Some(false) => return Err(LocaRemovalError::NotClosed),
            None if !sealed => return Err(LocaRemovalError::NotFound),
            _ => {}
        }
        let deleted = self
            .store
            .delete_loca_records(room, (self.now_ms)())
            .map_err(|_| LocaRemovalError::Storage)?;
        if let Some(r) = self.rooms.lock_or_recover().remove(room) {
            let _ = r.tx.send(ServerFrame::Control {
                cmd: "room-closed".into(),
            });
        }
        self.deleted.lock_or_recover().insert(room.to_string());
        self.operators.lock_or_recover().remove(room);
        self.invites
            .lock_or_recover()
            .retain(|_, invite| invite.room != room);
        self.sessions
            .lock_or_recover()
            .retain(|_, session| session.loca.as_deref() != Some(room));
        self.unbind_group_loca(room);
        tracing::info!(%room, rows = deleted, "loca deleted with its records");
        Ok(deleted)
    }
}

/// `YYYY-MM-DD HH:MM:SS` UTC for milliseconds since the epoch.
fn utc(ms: u64) -> String {
    let secs = ms / 1000;
    let (days, rest) = ((secs / 86_400) as i64, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        rest / 3600,
        rest / 60 % 60,
        rest % 60
    )
}

/// One readable line per message, oldest first.
fn transcript(room: &str, records: &LocaRecords) -> String {
    let mut out = format!("Loca \"{room}\" — conversation\n\n");
    let messages = records
        .tables
        .iter()
        .find(|(table, _)| *table == "messages")
        .map(|(_, rows)| rows.as_slice())
        .unwrap_or_default();
    let mut ordered: Vec<&serde_json::Value> = messages.iter().collect();
    ordered.sort_by_key(|m| m["id"].as_i64().unwrap_or_default());
    for m in ordered {
        let at = utc(m["ts"].as_u64().unwrap_or_default());
        let sender = m["sender"].as_str().unwrap_or("?");
        let to = match m["target"].as_str() {
            Some(t) if !t.is_empty() && t != "all" => format!(" → {t}"),
            _ => String::new(),
        };
        let text = m["text"].as_str().unwrap_or_default();
        out.push_str(&format!("[{at}] {sender}{to}: {text}\n"));
    }
    out
}

/// A name safe to store in the zip: the attachment's own file name with no
/// directory, after its id so two files with one name never collide.
fn attachment_entry(sha: &str, name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let base: String = base.chars().filter(|c| !c.is_control()).collect();
    let short = &sha[..sha.len().min(12)];
    if base.is_empty() || base == "." || base == ".." {
        format!("attachments/{short}")
    } else {
        format!("attachments/{short}-{base}")
    }
}

fn archive_zip(
    room: &str,
    now: u64,
    records: &LocaRecords,
    bytes_of: impl Fn(&str) -> Option<Vec<u8>>,
) -> zip::result::ZipResult<Vec<u8>> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(&mut buffer);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut missing = Vec::new();
    let mut files = Vec::new();
    for attachment in &records.attachments {
        match bytes_of(&attachment.sha) {
            Some(bytes) => {
                let entry = attachment_entry(&attachment.sha, &attachment.name);
                zip.start_file(entry.as_str(), options)?;
                zip.write_all(&bytes)?;
                files.push(serde_json::json!({ "id": attachment.sha, "name": attachment.name, "file": entry }));
            }
            None => missing.push(attachment.sha.clone()),
        }
    }
    let counts: serde_json::Map<String, serde_json::Value> = records
        .tables
        .iter()
        .map(|(table, rows)| (table.to_string(), rows.len().into()))
        .collect();
    let manifest = serde_json::json!({
        "loca": room,
        "archived_at": utc(now) + " UTC",
        "archived_at_ms": now,
        "server_version": env!("CARGO_PKG_VERSION"),
        "rows": counts,
        "attachments": files,
        "attachments_missing": missing,
        "left_out": crate::store::loca_removal::NOT_ARCHIVED,
    });
    zip.start_file("manifest.json", options)?;
    zip.write_all(
        serde_json::to_string_pretty(&manifest)
            .unwrap_or_default()
            .as_bytes(),
    )?;
    zip.start_file("transcript.txt", options)?;
    zip.write_all(transcript(room, records).as_bytes())?;
    for (table, rows) in &records.tables {
        zip.start_file(format!("data/{table}.json"), options)?;
        zip.write_all(
            serde_json::to_string_pretty(rows)
                .unwrap_or_default()
                .as_bytes(),
        )?;
    }
    zip.finish()?;
    Ok(buffer.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_reads_the_epoch_and_a_leap_day() {
        assert_eq!(utc(0), "1970-01-01 00:00:00");
        assert_eq!(utc(951_782_400_000 + 3_723_000), "2000-02-29 01:02:03");
    }

    #[test]
    fn an_attachment_entry_never_leaves_its_folder() {
        assert_eq!(
            attachment_entry("abcdef0123456789", "../../etc/passwd"),
            "attachments/abcdef012345-passwd"
        );
        assert_eq!(attachment_entry("abc", "a\\b\\..\\"), "attachments/abc");
        assert_eq!(
            attachment_entry("abc", "note.txt"),
            "attachments/abc-note.txt"
        );
    }
}
