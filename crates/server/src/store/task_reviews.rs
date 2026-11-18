//! A task's review history (the Loca Board): every request for review, go
//! and no-go, with who, when, and the note or reason. Rows are only
//! inserted, in the same transaction as the task they belong to, and read
//! back with it. A new table only: a database from an earlier version opens
//! unchanged (its tasks simply have no history), and an earlier binary opens
//! a database that has it and leaves the table alone.
use super::*;
use protocol::{TaskReview, TaskReviewKind};

pub(super) const SCHEMA: &str = r#"
    CREATE TABLE IF NOT EXISTS task_reviews (
        room TEXT NOT NULL,
        task_id INTEGER NOT NULL,
        seq INTEGER NOT NULL,
        kind TEXT NOT NULL,
        by_name TEXT NOT NULL,
        at INTEGER NOT NULL,
        text TEXT,
        PRIMARY KEY (room, task_id, seq)
    );
"#;

fn kind_text(kind: TaskReviewKind) -> &'static str {
    match kind {
        TaskReviewKind::Asked => "asked",
        TaskReviewKind::Go => "go",
        TaskReviewKind::Nogo => "nogo",
    }
}

fn kind_of(text: &str) -> Option<TaskReviewKind> {
    match text {
        "asked" => Some(TaskReviewKind::Asked),
        "go" => Some(TaskReviewKind::Go),
        "nogo" => Some(TaskReviewKind::Nogo),
        _ => None,
    }
}

impl Store {
    /// Write the task's history rows not yet stored (they are numbered by
    /// their place in the history, so an existing row is never rewritten).
    pub(super) fn write_task_reviews(c: &Connection, task: &Task) -> rusqlite::Result<()> {
        for (seq, review) in task.reviews.iter().enumerate() {
            c.execute(
                "INSERT OR IGNORE INTO task_reviews (room, task_id, seq, kind, by_name, at, text)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    task.room,
                    task.id,
                    seq as i64,
                    kind_text(review.kind),
                    review.by,
                    review.at,
                    review.text
                ],
            )?;
        }
        Ok(())
    }

    /// Give each of `room`'s tasks its history, oldest first.
    pub(super) fn attach_task_reviews(c: &Connection, room: &str, tasks: &mut [Task]) {
        let Ok(mut stmt) = c.prepare(
            "SELECT task_id, kind, by_name, at, text FROM task_reviews
             WHERE room = ?1 ORDER BY task_id, seq",
        ) else {
            return;
        };
        let Ok(rows) = stmt.query_map(params![room], |r| {
            Ok((
                r.get::<_, u64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, u64>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        }) else {
            return;
        };
        for (task_id, kind, by, at, text) in rows.flatten() {
            let (Some(kind), Some(task)) =
                (kind_of(&kind), tasks.iter_mut().find(|t| t.id == task_id))
            else {
                continue;
            };
            task.reviews.push(TaskReview { kind, by, at, text });
        }
    }
}
