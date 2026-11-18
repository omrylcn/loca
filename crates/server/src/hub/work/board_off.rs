//! Turning the Loca Board off while some of its tasks are not done.
//!
//! The server asks first (`BoardOff::Ask`, with the counts); the caller says
//! what becomes of them: keep them as they are (they come back with the
//! board), mark them done (the Operator's veto, no review), or remove them.
//! Nothing is ever deleted. The tasks are closed one by one through the
//! ordinary task rules before the board goes, so their goals, reminders and
//! frames follow as for any task closed by the Operator.

use protocol::{TaskActor, TaskStatus, UpdateTask};

use super::super::{ExtensionEdit, Hub};
use super::TaskError;
use crate::sync::RecoverMutex;

/// What becomes of the board's unfinished tasks when it is turned off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenTasks {
    Keep,
    Done,
    Remove,
}

/// The board's unfinished tasks, by stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub struct Unfinished {
    pub open: usize,
    pub taken: usize,
    pub review: usize,
}

impl Unfinished {
    pub fn total(&self) -> usize {
        self.open + self.taken + self.review
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardOff {
    /// Unfinished tasks, and no choice given: ask.
    Ask(Unfinished),
    Storage,
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        "task"
    } else {
        "tasks"
    }
}

impl Hub {
    /// Settle the board's unfinished tasks before `edit` turns it off, as
    /// `choice` says. `Ok(None)` when the edit leaves the board as it is or
    /// nothing is unfinished; otherwise the words the Journal line adds.
    pub fn settle_board_off(
        &self,
        room: &str,
        edit: ExtensionEdit<'_>,
        choice: Option<OpenTasks>,
        by: &str,
    ) -> Result<Option<String>, BoardOff> {
        let turns_off = self.extension_enabled(room, "board")
            && match edit {
                ExtensionEdit::Remove(id) => id.trim() == "board",
                ExtensionEdit::Replace(wanted) => !wanted.iter().any(|id| id.trim() == "board"),
                ExtensionEdit::Add(_) => false,
            };
        if !turns_off {
            return Ok(None);
        }
        let (unfinished, ids) = {
            let rooms = self.rooms.lock_or_recover();
            let mut counts = Unfinished::default();
            let mut ids = Vec::new();
            for task in rooms
                .get(room)
                .map(|r| r.tasks.values())
                .into_iter()
                .flatten()
            {
                match task.status {
                    TaskStatus::Open => counts.open += 1,
                    TaskStatus::Taken => counts.taken += 1,
                    TaskStatus::Review => counts.review += 1,
                    TaskStatus::Done | TaskStatus::Cancelled => continue,
                }
                ids.push(task.id);
            }
            ids.sort_unstable();
            (counts, ids)
        };
        let total = unfinished.total();
        if total == 0 {
            return Ok(None);
        }
        let (status, note) = match choice {
            None => return Err(BoardOff::Ask(unfinished)),
            Some(OpenTasks::Keep) => {
                return Ok(Some(format!("{total} unfinished {} kept", plural(total))))
            }
            Some(OpenTasks::Done) => (TaskStatus::Done, "marked done"),
            Some(OpenTasks::Remove) => (TaskStatus::Cancelled, "removed"),
        };
        for id in ids {
            let closed = self.update_task_as(
                room,
                id,
                UpdateTask {
                    status: Some(status),
                    assigned_to: None,
                    note: None,
                    reason: None,
                    by: by.to_string(),
                },
                TaskActor::Operator,
                None,
            );
            match closed {
                Ok(_) | Err(TaskError::NotFound) => {}
                Err(TaskError::Storage) => return Err(BoardOff::Storage),
                // An Operator may close any task; nothing else can happen.
                Err(_) => return Err(BoardOff::Storage),
            }
        }
        Ok(Some(format!("{total} {} {note}", plural(total))))
    }
}
