//! The Task board (the `board` extension): Focus tasks with a review step.
//!
//! While the board is on, the holder's "done" is a request for review, and
//! the Lead (never on its own task) or the Operator says go (`review` →
//! `done`) or no-go with a reason (`review` → `taken`). Removing a task and
//! reopening one stay the Operator's. With the board off every rule is the
//! house rule of `update_task_as`. The board owns no route; it changes the
//! rules of Focus's task routes, and needs Focus and a Lead.

use protocol::{Message, TaskStatus};

use super::super::{ExtensionError, Hub, Room, HISTORY_LIMIT};
use super::TaskError;
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
    // The Lead never reviews its own task; that review is the Operator's.
    let reviewer = who.operator || (who.lead && !who.mine);
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

impl Hub {
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
        id: u64,
        kind: Transition,
        by_label: &str,
        holder: Option<&str>,
    ) {
        match kind {
            Transition::Plain => {}
            Transition::ReviewAsked => {
                let lead = r.settings.lead.clone().unwrap_or_else(|| "all".into());
                let text = format!("{by_label} asks @{lead} to review task #{id}.");
                self.announce_task_line(r, room, id, &lead, text);
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

/// The Task board needs Focus and a Lead. `enabled` is the set a change asks
/// for, `before` the set it replaces. Removing Focus takes the board with it
/// in the same change (as removing Live ends live mode); turning the board
/// on without Focus, or without a Lead, is refused.
pub(in crate::hub) fn settle_board(
    before: &[String],
    mut enabled: Vec<String>,
    has_lead: bool,
) -> Result<Vec<String>, ExtensionError> {
    let on = |set: &[String], id: &str| set.iter().any(|have| have == id);
    if !on(&enabled, "board") {
        return Ok(enabled);
    }
    if !on(&enabled, "focus") {
        if on(before, "board") && on(before, "focus") {
            enabled.retain(|id| id != "board");
            return Ok(enabled);
        }
        return Err(ExtensionError::BoardNeedsFocus);
    }
    if !on(before, "board") && !has_lead {
        return Err(ExtensionError::BoardNeedsLead);
    }
    Ok(enabled)
}
