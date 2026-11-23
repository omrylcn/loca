//! Lessons for a loca's Vault (`protocol::LessonProposal`): what went wrong
//! once, proposed as a line, and written into the Vault only when those who
//! run the loca accept it. A reviewer's no-go reason and a reopened task
//! propose one by themselves; a person or an agent may propose one too. An
//! accepted lesson becomes a line under the Vault's `## Lessons` heading, a
//! new Vault revision like any other, so agents read it with the rest of the
//! rules. Nothing is learned without acceptance, and the server never reads
//! a lesson for meaning.

use protocol::{LessonProposal, LessonSource, LessonStatus, SenderType, ServerFrame};

use super::{Hub, Room, VaultError};
use crate::store::LessonDecision;
use crate::sync::RecoverMutex;

/// The longest lesson, in characters: one line a model reads in a glance.
pub const LESSON_MAX_CHARS: usize = 500;
/// The most proposals one loca keeps waiting at once.
pub const LESSONS_WAITING_MAX: u64 = 100;
/// How many lessons `GET /rooms/:id/vault/lessons` answers.
pub const LESSONS_LIST_LIMIT: usize = 200;
/// The Vault heading accepted lessons are written under.
pub const LESSONS_HEADING: &str = "## Lessons";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LessonError {
    /// The Vault is off in this loca (or it is a direct message).
    Off,
    Empty,
    TooLong,
    /// A character that hides itself or breaks a line.
    Invalid,
    TooMany,
    NotFound,
    /// Someone accepted or dismissed it first.
    Decided,
    /// The Vault has no room for one more line.
    VaultFull,
    NotWritable,
    NoStorage,
    Storage,
}

impl LessonError {
    pub fn message(&self) -> String {
        match self {
            Self::Off => "lessons live in the Vault, and the Vault is off in this loca".into(),
            Self::Empty => "a lesson needs words".into(),
            Self::TooLong => format!("a lesson is one line: at most {LESSON_MAX_CHARS} characters"),
            Self::Invalid => {
                "a lesson must be plain text, with no control or invisible formatting characters"
                    .into()
            }
            Self::TooMany => format!(
                "{LESSONS_WAITING_MAX} lessons already wait for a decision here; accept or \
                 dismiss some first"
            ),
            Self::NotFound => "no such lesson in this loca".into(),
            Self::Decided => "this lesson was already accepted or dismissed".into(),
            Self::VaultFull => {
                "the Vault has no room for this lesson; shorten or remove older rules first".into()
            }
            Self::NotWritable => "this loca is closed — read-only".into(),
            Self::NoStorage => "lessons need persistent storage in this deployment".into(),
            Self::Storage => "could not save the lesson — try again".into(),
        }
    }
}

/// A lesson as it is kept: one line, its whitespace (line breaks included)
/// folded to single spaces, checked for length and hidden characters.
pub fn lesson_text(text: &str) -> Result<String, LessonError> {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.is_empty() {
        return Err(LessonError::Empty);
    }
    if line.chars().count() > LESSON_MAX_CHARS {
        return Err(LessonError::TooLong);
    }
    if line.chars().any(super::identity::hidden_or_breaking) {
        return Err(LessonError::Invalid);
    }
    Ok(line)
}

/// The Vault line an accepted lesson becomes: the words, then where they
/// came from, so a reader can trace it.
pub fn vault_line(lesson: &LessonProposal, text: &str) -> String {
    let from = match (lesson.source, lesson.task) {
        (LessonSource::Nogo, Some(task)) => format!("from the no-go on task #{task}"),
        (LessonSource::Reopen, Some(task)) => format!("from reopened task #{task}"),
        _ => format!("proposed by {}", lesson.proposed_by),
    };
    format!("- {text} ({from})")
}

/// `body` with `line` added at the end of its `## Lessons` section (made at
/// the end when there is none), or `None` when the line is already there.
pub fn vault_with_lesson(body: &str, line: &str) -> Option<String> {
    if body.lines().any(|existing| existing.trim() == line.trim()) {
        return None;
    }
    let lines: Vec<&str> = body.lines().collect();
    let Some(heading) = lines.iter().position(|l| l.trim() == LESSONS_HEADING) else {
        let base = body.trim_end();
        return Some(if base.is_empty() {
            format!("{LESSONS_HEADING}\n\n{line}\n")
        } else {
            format!("{base}\n\n{LESSONS_HEADING}\n\n{line}\n")
        });
    };
    let end = lines[heading + 1..]
        .iter()
        .position(|l| l.starts_with("## "))
        .map_or(lines.len(), |i| heading + 1 + i);
    let mut at = end;
    while at > heading + 1 && lines[at - 1].trim().is_empty() {
        at -= 1;
    }
    let mut out: Vec<&str> = Vec::with_capacity(lines.len() + 2);
    out.extend_from_slice(&lines[..at]);
    if at == heading + 1 {
        out.push("");
    }
    out.push(line);
    if at < lines.len() {
        out.push("");
        out.extend(lines[at..].iter().skip_while(|l| l.trim().is_empty()));
    }
    let mut joined = out.join("\n");
    joined.push('\n');
    Some(joined)
}

impl Hub {
    /// Up to [`LESSONS_LIST_LIMIT`] lessons of `room`, waiting ones first.
    pub fn lessons(&self, room: &str) -> rusqlite::Result<Vec<LessonProposal>> {
        self.store.lessons(room, LESSONS_LIST_LIMIT)
    }

    fn lessons_allowed(&self, room: &str) -> Result<(), LessonError> {
        if self.is_dm(room) || room.starts_with(Self::DM_PREFIX) {
            return Err(LessonError::Off);
        }
        if !self.store.is_persistent() {
            return Err(LessonError::NoStorage);
        }
        if !self.is_writable(room) {
            return Err(LessonError::NotWritable);
        }
        Ok(())
    }

    /// Propose a lesson in `room` as `by`. The caller checked that `by` may
    /// write in the loca.
    pub fn propose_lesson(
        &self,
        room: &str,
        text: &str,
        source: LessonSource,
        by: &str,
    ) -> Result<LessonProposal, LessonError> {
        self.lessons_allowed(room)?;
        let mut rooms = self.rooms.lock_or_recover();
        let r = Self::room_mut(&mut rooms, room, &self.default_settings);
        self.propose_lesson_in(r, room, text, source, None, by)
    }

    /// Propose a lesson in the loca `r` the caller holds. A proposal that
    /// already waits with the same words, source and task is answered
    /// instead of filed twice.
    pub(super) fn propose_lesson_in(
        &self,
        r: &mut Room,
        room: &str,
        text: &str,
        source: LessonSource,
        task: Option<u64>,
        by: &str,
    ) -> Result<LessonProposal, LessonError> {
        if !r.settings.extension_enabled("vault") {
            return Err(LessonError::Off);
        }
        let text = lesson_text(text)?;
        let (waiting, same) = self
            .store
            .proposed_lessons(room, &text, source, task)
            .map_err(|_| LessonError::Storage)?;
        if let Some(id) = same {
            return self
                .store
                .lesson(room, id)
                .map_err(|_| LessonError::Storage)?
                .ok_or(LessonError::Storage);
        }
        if waiting >= LESSONS_WAITING_MAX {
            return Err(LessonError::TooMany);
        }
        let lesson = self
            .store
            .insert_lesson(room, &text, source, task, by, (self.now_ms)())
            .map_err(|_| LessonError::Storage)?;
        let _ = r.tx.send(ServerFrame::Lesson {
            lesson: lesson.clone(),
        });
        Ok(lesson)
    }

    /// A proposal the task flow makes by itself (a no-go reason, a reopened
    /// task). It is advice, so a loca without the Vault, or a full queue,
    /// simply has none.
    pub(super) fn propose_task_lesson(
        &self,
        r: &mut Room,
        room: &str,
        text: &str,
        source: LessonSource,
        task: u64,
        by: &str,
    ) {
        if !self.store.is_persistent() {
            return;
        }
        match self.propose_lesson_in(r, room, text, source, Some(task), by) {
            Ok(_) | Err(LessonError::Off) => {}
            Err(error) => {
                tracing::warn!(room, task, error = %error.message(), "no lesson proposed");
            }
        }
    }

    /// Accept (`accept_text` given: the words as accepted) or dismiss lesson
    /// `id` of `room`. The caller checked that `by` runs the loca. An
    /// accepted lesson is written into the Vault first, then marked; a
    /// lesson whose line is already in the Vault is not written twice.
    pub fn decide_lesson(
        &self,
        room: &str,
        id: u64,
        accept_text: Option<&str>,
        by: &str,
        by_type: SenderType,
        by_principal: Option<&str>,
    ) -> Result<LessonProposal, LessonError> {
        self.lessons_allowed(room)?;
        if !self
            .rooms
            .lock_or_recover()
            .get(room)
            .is_some_and(|r| r.settings.extension_enabled("vault"))
        {
            return Err(LessonError::Off);
        }
        let mut lesson = self
            .store
            .lesson(room, id)
            .map_err(|_| LessonError::Storage)?
            .ok_or(LessonError::NotFound)?;
        if lesson.status != LessonStatus::Proposed {
            return Err(LessonError::Decided);
        }
        let now = (self.now_ms)();
        let (status, text, vault_rev) = match accept_text {
            Some(asked) => {
                let text = lesson_text(asked)?;
                let line = vault_line(&lesson, &text);
                let rev = self.write_lesson_line(room, &line, by, by_type, by_principal)?;
                (LessonStatus::Accepted, text, Some(rev))
            }
            None => (LessonStatus::Dismissed, lesson.text.clone(), None),
        };
        let decided = self
            .store
            .decide_lesson(
                room,
                id,
                &LessonDecision {
                    status,
                    text: &text,
                    by,
                    at: now,
                    vault_rev,
                },
            )
            .map_err(|_| LessonError::Storage)?;
        if !decided {
            return Err(LessonError::Decided);
        }
        lesson.status = status;
        lesson.text = text;
        lesson.decided_by = Some(by.to_string());
        lesson.decided_at = Some(now);
        lesson.vault_rev = vault_rev;
        if let Some(r) = self.rooms.lock_or_recover().get(room) {
            let _ = r.tx.send(ServerFrame::Lesson {
                lesson: lesson.clone(),
            });
        }
        Ok(lesson)
    }

    /// Add `line` to the Vault's Lessons section as a new revision and
    /// answer the revision that holds it. One retry when someone wrote the
    /// Vault in between.
    fn write_lesson_line(
        &self,
        room: &str,
        line: &str,
        by: &str,
        by_type: SenderType,
        by_principal: Option<&str>,
    ) -> Result<u64, LessonError> {
        for attempt in 0..2 {
            let doc = self.vault(room).map_err(|_| LessonError::Storage)?;
            let Some(body) = vault_with_lesson(&doc.body, line) else {
                return Ok(doc.rev);
            };
            match self.write_vault(room, &body, doc.rev, by, by_type, by_principal) {
                Ok(written) => return Ok(written.rev),
                Err(VaultError::Stale { .. }) if attempt == 0 => continue,
                Err(VaultError::TooLong) => return Err(LessonError::VaultFull),
                Err(VaultError::NotWritable) => return Err(LessonError::NotWritable),
                Err(VaultError::Dm) => return Err(LessonError::Off),
                Err(VaultError::NoStorage) => return Err(LessonError::NoStorage),
                Err(_) => return Err(LessonError::Storage),
            }
        }
        Err(LessonError::Storage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lesson_is_one_plain_line() {
        assert_eq!(
            lesson_text("  run the tests\n before   review "),
            Ok("run the tests before review".into())
        );
        assert_eq!(lesson_text(" \n "), Err(LessonError::Empty));
        assert_eq!(
            lesson_text(&"a".repeat(LESSON_MAX_CHARS + 1)),
            Err(LessonError::TooLong)
        );
        assert_eq!(lesson_text("zero\u{200B}width"), Err(LessonError::Invalid));
    }

    #[test]
    fn an_accepted_lesson_goes_under_the_lessons_heading_once() {
        let first = vault_with_lesson("", "- a (x)").unwrap();
        assert_eq!(first, "## Lessons\n\n- a (x)\n");
        let rules = "# Rules\n\n- ask before deploy\n";
        let with = vault_with_lesson(rules, "- a (x)").unwrap();
        assert_eq!(
            with,
            "# Rules\n\n- ask before deploy\n\n## Lessons\n\n- a (x)\n"
        );
        let second = vault_with_lesson(&with, "- b (y)").unwrap();
        assert_eq!(
            second,
            "# Rules\n\n- ask before deploy\n\n## Lessons\n\n- a (x)\n- b (y)\n"
        );
        assert_eq!(vault_with_lesson(&second, "- b (y)"), None);
        // A section in the middle keeps what follows it.
        let middle = "## Lessons\n\n- a (x)\n\n## Deploy\n\n- tag first\n";
        assert_eq!(
            vault_with_lesson(middle, "- b (y)").unwrap(),
            "## Lessons\n\n- a (x)\n- b (y)\n\n## Deploy\n\n- tag first\n"
        );
        // An empty section gets its first line after a blank one.
        assert_eq!(
            vault_with_lesson("## Lessons\n", "- a (x)").unwrap(),
            "## Lessons\n\n- a (x)\n"
        );
    }

    #[test]
    fn the_vault_line_says_where_the_lesson_came_from() {
        let mut lesson = LessonProposal {
            id: 1,
            room: "r".into(),
            text: "t".into(),
            source: LessonSource::Nogo,
            task: Some(7),
            proposed_by: "ada".into(),
            proposed_at: 1,
            status: LessonStatus::Proposed,
            decided_by: None,
            decided_at: None,
            vault_rev: None,
        };
        assert_eq!(
            vault_line(&lesson, "run it"),
            "- run it (from the no-go on task #7)"
        );
        lesson.source = LessonSource::Reopen;
        assert_eq!(
            vault_line(&lesson, "run it"),
            "- run it (from reopened task #7)"
        );
        lesson.source = LessonSource::Agent;
        lesson.task = None;
        assert_eq!(vault_line(&lesson, "run it"), "- run it (proposed by ada)");
    }
}
