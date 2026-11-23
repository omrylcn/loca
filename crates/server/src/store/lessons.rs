//! Lessons proposed for a loca's Vault (`protocol::LessonProposal`): one row
//! per proposal, with who proposed it, where it came from and what became of
//! it. Rows are inserted, then decided once; nothing is deleted. The text is
//! stored as given and never read for meaning.
use super::{params, OptionalExtension, Store};
use protocol::{LessonProposal, LessonSource, LessonStatus};

/// A new table only, so a database from an earlier version opens unchanged:
/// none of its locas has a proposed lesson.
pub(super) const SCHEMA: &str = r#"
    CREATE TABLE IF NOT EXISTS loca_lessons (
        room TEXT NOT NULL,
        id INTEGER NOT NULL,
        text TEXT NOT NULL,
        source TEXT NOT NULL,
        task INTEGER,
        proposed_by TEXT NOT NULL,
        proposed_at INTEGER NOT NULL,
        status TEXT NOT NULL,
        decided_by TEXT,
        decided_at INTEGER,
        vault_rev INTEGER,
        PRIMARY KEY (room, id)
    );
"#;

fn source_str(source: LessonSource) -> &'static str {
    match source {
        LessonSource::Nogo => "nogo",
        LessonSource::Reopen => "reopen",
        LessonSource::Person => "person",
        LessonSource::Agent => "agent",
    }
}

fn source_of(text: &str) -> LessonSource {
    match text {
        "nogo" => LessonSource::Nogo,
        "reopen" => LessonSource::Reopen,
        "agent" => LessonSource::Agent,
        _ => LessonSource::Person,
    }
}

fn status_str(status: LessonStatus) -> &'static str {
    match status {
        LessonStatus::Proposed => "proposed",
        LessonStatus::Accepted => "accepted",
        LessonStatus::Dismissed => "dismissed",
    }
}

fn status_of(text: &str) -> LessonStatus {
    match text {
        "accepted" => LessonStatus::Accepted,
        "dismissed" => LessonStatus::Dismissed,
        _ => LessonStatus::Proposed,
    }
}

const COLUMNS: &str = "id, text, source, task, proposed_by, proposed_at, status, \
                       decided_by, decided_at, vault_rev";

fn lesson_of(room: &str, row: &rusqlite::Row<'_>) -> rusqlite::Result<LessonProposal> {
    Ok(LessonProposal {
        id: row.get(0)?,
        room: room.to_string(),
        text: row.get(1)?,
        source: source_of(&row.get::<_, String>(2)?),
        task: row.get(3)?,
        proposed_by: row.get(4)?,
        proposed_at: row.get(5)?,
        status: status_of(&row.get::<_, String>(6)?),
        decided_by: row.get(7)?,
        decided_at: row.get(8)?,
        vault_rev: row.get(9)?,
    })
}

/// What a decision writes onto a proposal.
pub struct LessonDecision<'a> {
    pub status: LessonStatus,
    /// The words as accepted (an accepter may edit them).
    pub text: &'a str,
    pub by: &'a str,
    pub at: u64,
    pub vault_rev: Option<u64>,
}

impl Store {
    /// Up to `limit` lessons of `room`: the proposed ones first, then the
    /// decided ones, newest first within each.
    pub fn lessons(&self, room: &str, limit: usize) -> rusqlite::Result<Vec<LessonProposal>> {
        let Some(c) = self.conn() else {
            return Ok(Vec::new());
        };
        let mut statement = c.prepare(&format!(
            "SELECT {COLUMNS} FROM loca_lessons WHERE room = ?1
             ORDER BY status = 'proposed' DESC, id DESC LIMIT ?2"
        ))?;
        let rows = statement.query_map(params![room, limit as i64], |row| lesson_of(room, row))?;
        rows.collect()
    }

    /// One lesson of `room`.
    pub fn lesson(&self, room: &str, id: u64) -> rusqlite::Result<Option<LessonProposal>> {
        let Some(c) = self.conn() else {
            return Ok(None);
        };
        c.query_row(
            &format!("SELECT {COLUMNS} FROM loca_lessons WHERE room = ?1 AND id = ?2"),
            params![room, id],
            |row| lesson_of(room, row),
        )
        .optional()
    }

    /// How many lessons of `room` wait for a decision, and whether one with
    /// the same text, source and task already does.
    pub fn proposed_lessons(
        &self,
        room: &str,
        text: &str,
        source: LessonSource,
        task: Option<u64>,
    ) -> rusqlite::Result<(u64, Option<u64>)> {
        let Some(c) = self.conn() else {
            return Ok((0, None));
        };
        let waiting: u64 = c.query_row(
            "SELECT COUNT(*) FROM loca_lessons WHERE room = ?1 AND status = 'proposed'",
            params![room],
            |row| row.get(0),
        )?;
        let same: Option<u64> = c
            .query_row(
                "SELECT id FROM loca_lessons WHERE room = ?1 AND status = 'proposed'
                   AND text = ?2 AND source = ?3 AND task IS ?4 LIMIT 1",
                params![room, text, source_str(source), task],
                |row| row.get(0),
            )
            .optional()?;
        Ok((waiting, same))
    }

    /// File a new proposal under the next id of its loca and return it.
    pub fn insert_lesson(
        &self,
        room: &str,
        text: &str,
        source: LessonSource,
        task: Option<u64>,
        by: &str,
        at: u64,
    ) -> rusqlite::Result<LessonProposal> {
        let Some(mut c) = self.conn() else {
            return Err(rusqlite::Error::InvalidQuery);
        };
        let tx = c.transaction()?;
        let id: u64 = tx.query_row(
            "SELECT COALESCE(MAX(id), 0) + 1 FROM loca_lessons WHERE room = ?1",
            params![room],
            |row| row.get(0),
        )?;
        tx.execute(
            "INSERT INTO loca_lessons (room, id, text, source, task, proposed_by, proposed_at, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'proposed')",
            params![room, id, text, source_str(source), task, by, at],
        )?;
        tx.commit()?;
        Ok(LessonProposal {
            id,
            room: room.to_string(),
            text: text.to_string(),
            source,
            task,
            proposed_by: by.to_string(),
            proposed_at: at,
            status: LessonStatus::Proposed,
            decided_by: None,
            decided_at: None,
            vault_rev: None,
        })
    }

    /// Decide a proposal once. `Ok(false)` when it was no longer proposed
    /// (someone decided it first).
    pub fn decide_lesson(
        &self,
        room: &str,
        id: u64,
        decision: &LessonDecision<'_>,
    ) -> rusqlite::Result<bool> {
        let Some(c) = self.conn() else {
            return Ok(false);
        };
        let changed = c.execute(
            "UPDATE loca_lessons SET status = ?3, text = ?4, decided_by = ?5, decided_at = ?6,
                    vault_rev = ?7
             WHERE room = ?1 AND id = ?2 AND status = 'proposed'",
            params![
                room,
                id,
                status_str(decision.status),
                decision.text,
                decision.by,
                decision.at,
                decision.vault_rev
            ],
        )?;
        Ok(changed == 1)
    }
}
