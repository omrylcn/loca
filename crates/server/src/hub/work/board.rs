//! The Loca Board (the `board` extension): the loca's tasks, with a review
//! step.
//!
//! The board owns the task routes (`protocol::EXTENSIONS`), so with it off
//! they are refused and every stored task is kept. The holder's "done" is a
//! request for review, and whoever gave the task to its holder (or, when the
//! holder took it, whoever declared it), never the holder, or the Operator
//! says go (`review` → `done`) or no-go with a reason (`review` → `taken`).
//! Removing a task and reopening one stay the Operator's. `transition` keeps
//! the house rule for a board that is off, which only a caller inside the
//! hub can reach. The board needs a Lead, not Focus.

use protocol::{Message, Task, TaskReview, TaskReviewKind, TaskStatus};

use super::super::{ExtensionError, Hub, Room, HISTORY_LIMIT};
use super::TaskError;
use crate::sync::RecoverMutex;
use std::sync::atomic::Ordering;

/// What kind of move a status change is, for the line it posts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Transition {
    /// A change with no line of its own.
    Plain,
    /// The holder asks for review (board on).
    ReviewAsked,
    /// The reviewer accepts: `review` → `done`.
    Go,
    /// The reviewer sends it back, with a reason: `review` → `taken`.
    NoGo,
}

/// Who is asking, as far as a status change cares.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Asker {
    /// The board is on in this loca.
    pub board: bool,
    /// An operator: the loca's Operator, the Master or a Smaster.
    pub operator: bool,
    /// The loca's Lead, checked by principal.
    pub lead: bool,
    /// The task's holder.
    pub mine: bool,
    /// The task's reviewer (`Hub::task_reviewer`): who gave it to its
    /// holder, or who declared it.
    pub reviewer: bool,
    /// The Lead declared this task itself and nobody has taken it.
    pub lead_declared_untaken: bool,
    /// The request carries a non-empty reason.
    pub has_reason: bool,
}

/// The status a request moves a task to, and what kind of move it is; or
/// why it may not. With the board off this is exactly the house rule (plus
/// the Lead's cancel of its own untaken declaration).
pub(super) fn transition(
    from: TaskStatus,
    asked: TaskStatus,
    who: Asker,
) -> Result<(TaskStatus, Transition), TaskError> {
    use TaskStatus::{Cancelled, Done, Open, Review, Taken};
    let allow = |ok: bool, to: TaskStatus, kind: Transition| {
        if ok {
            Ok((to, kind))
        } else {
            Err(TaskError::Forbidden)
        }
    };
    let cancels = who.operator || (who.lead && who.lead_declared_untaken);
    if !who.board {
        return match asked {
            Taken | Done => allow(who.operator || who.mine, asked, Transition::Plain),
            Cancelled => allow(cancels, asked, Transition::Plain),
            Open => allow(who.operator, asked, Transition::Plain),
            Review => Err(TaskError::BoardOff),
        };
    }
    // Whoever gave the task reviews it, never its holder; the Operator
    // always may.
    let reviewer = who.operator || (who.reviewer && !who.mine);
    match (from, asked) {
        (Review, Done) => allow(reviewer, Done, Transition::Go),
        (Review, Taken) if reviewer && !who.has_reason => Err(TaskError::ReasonRequired),
        (Review, Taken) => allow(reviewer, Taken, Transition::NoGo),
        // The holder's "done" is a request for review.
        (Open | Taken, Done | Review) if who.mine && !who.operator => {
            Ok((Review, Transition::ReviewAsked))
        }
        (Open | Taken, Review) => allow(who.operator, Review, Transition::ReviewAsked),
        (_, Review) => Err(TaskError::Forbidden),
        // Finishing without a review, and taking back finished work, are
        // the Operator's veto.
        (_, Done) => allow(who.operator, Done, Transition::Plain),
        (Open | Taken, Taken) => allow(who.operator || who.mine, Taken, Transition::Plain),
        (_, Taken) => allow(who.operator, Taken, Transition::Plain),
        (_, Cancelled) => allow(cancels, Cancelled, Transition::Plain),
        (_, Open) => allow(who.operator, Open, Transition::Plain),
    }
}

/// Add a review move to the task's history (read-only once written): a
/// request with the holder's note, a go, or a no-go with its reason.
pub(super) fn record_review(
    task: &mut Task,
    kind: Transition,
    by: &str,
    at: u64,
    note: Option<&str>,
    reason: Option<&str>,
) {
    let (kind, text) = match kind {
        Transition::Plain => return,
        Transition::ReviewAsked => (TaskReviewKind::Asked, note),
        Transition::Go => (TaskReviewKind::Go, None),
        Transition::NoGo => (TaskReviewKind::Nogo, reason),
    };
    task.reviews.push(TaskReview {
        kind,
        by: by.to_string(),
        at,
        text: text.map(str::to_string),
    });
}

impl Hub {
    /// One task of `room` by its number.
    pub fn task(&self, room: &str, id: u64) -> Option<protocol::Task> {
        let rooms = self.rooms.lock_or_recover();
        rooms.get(room).and_then(|r| r.tasks.get(&id).cloned())
    }

    /// How many tasks `room` holds, on the board or kept while it is off.
    /// A count only: what they say is the board's to show.
    pub fn stored_task_count(&self, room: &str) -> usize {
        let rooms = self.rooms.lock_or_recover();
        rooms.get(room).map_or(0, |r| r.tasks.len())
    }

    /// Post one server line about task `id` in the loca, addressed to
    /// `target` (`all` wakes nobody in an announcement). The task change has
    /// already committed; its line must not depend on a second write (the
    /// `announce_join_request` rule): a failed write is logged and the line
    /// is still shown and delivered.
    pub(super) fn announce_task_line(
        &self,
        r: &mut Room,
        room: &str,
        id: u64,
        target: &str,
        text: String,
    ) {
        let msg = Message {
            attachments: Vec::new(),
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            room: room.to_string(),
            sender: "loca".into(),
            sender_type: protocol::SenderType::User,
            target: Some(target.to_string()),
            text,
            reply_to: None,
            reply_to_sender: None,
            sender_principal: None,
            kind: protocol::MessageKind::Announce,
            ts: (self.now_ms)(),
        };
        if let Err(error) = self.store.insert_message(&msg, None) {
            tracing::error!(%error, room, task_id = id, "could not persist a task line");
        }
        r.history.push(msg.clone());
        if r.history.len() > HISTORY_LIMIT {
            let overflow = r.history.len() - HISTORY_LIMIT;
            r.history.drain(0..overflow);
        }
        let _ = r.tx.send(protocol::ServerFrame::Msg { message: msg });
    }

    /// The line a board move posts, if any. The title and the reason are
    /// left out on purpose, so an `@name` written in them wakes nobody; they
    /// are read from the task.
    pub(super) fn announce_board_move(
        &self,
        r: &mut Room,
        room: &str,
        task: &Task,
        kind: Transition,
        by_label: &str,
    ) {
        let id = task.id;
        let holder = task.assigned_to.as_deref();
        let reviewer = task_reviewer(task);
        match kind {
            Transition::Plain => {}
            Transition::ReviewAsked => {
                let reviewer = reviewer.map(str::to_string).unwrap_or_else(|| "all".into());
                let text = format!("{by_label} asks @{reviewer} to review task #{id}.");
                self.announce_task_line(r, room, id, &reviewer, text);
            }
            Transition::Go => {
                let text = format!("{by_label}: task #{id} is done.");
                self.announce_task_line(r, room, id, "all", text);
            }
            Transition::NoGo => {
                let holder = holder.unwrap_or("all");
                let text = format!(
                    "{by_label}: task #{id} is not done yet, back to @{holder} (the reason is on the task)."
                );
                self.announce_task_line(r, room, id, holder, text);
            }
        }
    }
}

/// The Loca Board needs a Lead. `enabled` is the set a change
/// asks for, `before` the set it replaces: turning the board on without a
/// Lead is refused. It does not need Focus, and removing Focus leaves it.
pub(in crate::hub) fn settle_board(
    before: &[String],
    enabled: Vec<String>,
    has_lead: bool,
) -> Result<Vec<String>, ExtensionError> {
    let on = |set: &[String], id: &str| set.iter().any(|have| have == id);
    if on(&enabled, "board") && !on(before, "board") && !has_lead {
        return Err(ExtensionError::BoardNeedsLead);
    }
    Ok(enabled)
}

/// Whose review a task waits for: who gave it to its holder, or, where the
/// holder took it themselves (or nobody holds it), who declared it. Never
/// the holder: then only the Operator reviews it.
pub(super) fn task_reviewer(task: &Task) -> Option<&str> {
    let holder = task.assigned_to.as_deref();
    let reviewer = match task.assigned_by.as_deref() {
        Some(by) if Some(by) != holder => by,
        _ => task.created_by.as_str(),
    };
    (Some(reviewer) != holder).then_some(reviewer)
}

impl Hub {
    /// Whether the one asking is the task's reviewer (`task_reviewer`),
    /// checked by principal: the declarer's recorded one, else the one the
    /// name stands for; by name only where names stand for principals.
    pub(super) fn asks_as_reviewer(
        &self,
        task: &Task,
        by: &str,
        by_principal: Option<&str>,
    ) -> bool {
        let Some(name) = task_reviewer(task) else {
            return false;
        };
        let recorded = (name == task.created_by && task.assigned_by.as_deref() != Some(name))
            .then(|| task.created_principal.clone())
            .flatten();
        match recorded.or_else(|| self.principal_named(name)) {
            Some(principal) => by_principal == Some(principal.as_str()),
            None => self.names_stand_for_principals() && name == by,
        }
    }
}
