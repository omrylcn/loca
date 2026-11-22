use super::super::{CreateTask, Hub, Room, ServerFrame, Task, TaskError, TaskStatus, UpdateTask};
use super::board;
use crate::sync::RecoverMutex;
use protocol::TaskActor;

/// The longest review note or no-go reason, in characters.
const TASK_NOTE_MAX: usize = 2000;

impl Hub {
    pub fn tasks(&self, room: &str) -> Vec<Task> {
        let rooms = self.rooms.lock_or_recover();
        let mut list: Vec<Task> = rooms
            .get(room)
            .map(|r| r.tasks.values().cloned().collect())
            .unwrap_or_default();
        list.sort_by_key(|t| t.id);
        list
    }
    /// Declare a task (operator's signature required at the route layer).
    /// `by_principal` is the principal the server verified for `req.by`.
    #[cfg(test)]
    pub fn create_task(
        &self,
        room: &str,
        req: CreateTask,
        by_principal: Option<&str>,
    ) -> rusqlite::Result<Task> {
        self.create_task_as(room, req, by_principal, TaskActor::Operator)
    }
    /// Declare a task with the authority the route layer resolved: an
    /// operator's, or the Lead's. The task records which, so the Lead's
    /// bounds hold later.
    pub fn create_task_as(
        &self,
        room: &str,
        req: CreateTask,
        by_principal: Option<&str>,
        actor: TaskActor,
    ) -> rusqlite::Result<Task> {
        let now = (self.now_ms)();
        let assigned_principal = req
            .assigned_to
            .as_deref()
            .and_then(|assignee| self.assignee_principal(assignee, &req.by, by_principal));
        let mut rooms = self.rooms.lock_or_recover();
        let id = rooms.get(room).map(|r| r.next_task_id).unwrap_or(1);
        let t = Task {
            id,
            room: room.to_string(),
            title: req.title,
            assigned_by: req.assigned_to.as_ref().map(|_| req.by.clone()),
            created_by: req.by,
            from_message: req.from_message,
            assigned_as: req.assigned_to.as_ref().map(|_| actor),
            assigned_to: req.assigned_to,
            assigned_principal,
            review_note: None,
            review_reason: None,
            reviews: Vec::new(),
            created_as: Some(actor),
            created_principal: by_principal.map(str::to_string),
            status: TaskStatus::Open,
            created_at: now,
            progress_at: now,
            closed_at: None,
        };
        self.store.upsert_task(&t)?;
        let r = Self::room_mut(&mut rooms, room, &self.default_settings);
        r.next_task_id = id + 1;
        r.tasks.insert(id, t.clone());
        let _ = r.tx.send(ServerFrame::Task { task: t.clone() });
        if let Some(assignee) = t.assigned_to.as_deref().filter(|who| *who != t.created_by) {
            self.announce_assignment(r, room, id, &assigner_label(&t.created_by, actor), assignee);
        }
        Ok(t)
    }
    /// Update a task. Rules of the house:
    /// - the assigned agent may TAKE it and mark it DONE (agent finishes),
    /// - only an operator may cancel, reopen or reassign (operator contests).
    ///
    /// `is_operator` and `by_principal` (the principal the server verified
    /// for `req.by`) are resolved at the route layer. Where the task records
    /// the principal it was given to, that principal is its assignee and the
    /// name is only its label: another person who now wears the name is not.
    #[cfg(test)]
    pub fn update_task(
        &self,
        room: &str,
        id: u64,
        req: UpdateTask,
        is_operator: bool,
        by_principal: Option<&str>,
    ) -> Result<Task, TaskError> {
        let actor = if is_operator {
            TaskActor::Operator
        } else {
            TaskActor::Participant
        };
        self.update_task_as(room, id, req, actor, by_principal)
    }
    /// Update a task with the authority the route layer resolved. On top of
    /// the house rules above, the Lead (checked by principal) may assign or
    /// reassign an unfinished task unless the current assignment is an
    /// operator's, and may cancel an untaken task it declared itself.
    pub fn update_task_as(
        &self,
        room: &str,
        id: u64,
        req: UpdateTask,
        actor: TaskActor,
        by_principal: Option<&str>,
    ) -> Result<Task, TaskError> {
        let is_operator = actor == TaskActor::Operator;
        let is_lead = actor == TaskActor::Lead;
        let note = req
            .note
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string);
        let reason = req
            .reason
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string);
        if [&note, &reason].iter().any(|text| {
            text.as_ref()
                .is_some_and(|t| t.chars().count() > TASK_NOTE_MAX)
        }) {
            return Err(TaskError::TooLong);
        }
        let now = (self.now_ms)();
        let new_principal = req
            .assigned_to
            .as_deref()
            .and_then(|assignee| self.assignee_principal(assignee, &req.by, by_principal));
        let mut rooms = self.rooms.lock_or_recover();
        let r = rooms.get_mut(room).ok_or(TaskError::NotFound)?;
        let mut out = r.tasks.get(&id).cloned().ok_or(TaskError::NotFound)?;
        let mut progressed = false;
        let mut newly_assigned = None;
        let is_assignee = |task: &Task| match task.assigned_principal.as_deref() {
            Some(assigned) => by_principal == Some(assigned),
            None => task.assigned_to.as_deref() == Some(req.by.as_str()),
        };

        if let Some(assignee) = req.assigned_to.clone() {
            let made_as = if is_operator {
                TaskActor::Operator
            } else if is_lead && Self::lead_may_assign(&out) {
                TaskActor::Lead
            } else {
                // An unassigned task may be self-claimed by whoever takes it;
                // stealing someone else's assignment needs operator authority.
                if out.assigned_to.is_some() && !is_assignee(&out) {
                    return Err(TaskError::Forbidden);
                }
                if assignee != req.by {
                    return Err(TaskError::Forbidden);
                }
                TaskActor::Participant
            };
            if out.assigned_to.as_deref() != Some(assignee.as_str()) {
                progressed = true;
                out.assigned_principal = new_principal;
                out.assigned_by = Some(req.by.clone());
                out.assigned_as = Some(made_as);
                if assignee != req.by {
                    newly_assigned = Some(assignee.clone());
                }
            }
            out.assigned_to = Some(assignee);
        }
        let mut moved = board::Transition::Plain;
        if let Some(asked) = req.status {
            let mine = is_assignee(&out);
            let asker = board::Asker {
                board: r.settings.extension_enabled("board"),
                operator: is_operator,
                lead: is_lead,
                mine,
                reviewer: self.asks_as_reviewer(&out, &req.by, by_principal),
                lead_declared_untaken: is_lead && self.lead_may_cancel(&out, &req.by, by_principal),
                has_reason: reason.is_some(),
            };
            let (st, kind) = board::transition(out.status, asked, asker)?;
            moved = kind;
            match kind {
                board::Transition::ReviewAsked => {
                    out.review_note = note.clone();
                    out.review_reason = None;
                }
                board::Transition::NoGo => out.review_reason = reason.clone(),
                board::Transition::Plain | board::Transition::Go => {}
            }
            board::record_review(
                &mut out,
                kind,
                &req.by,
                now,
                note.as_deref(),
                reason.as_deref(),
            );
            // The assignee's own act binds a task given by name alone (before
            // the principal was recorded, or to a name nobody wore yet) to the
            // principal who takes it.
            if mine && out.assigned_principal.is_none() {
                out.assigned_principal = by_principal.map(str::to_string);
            }
            let status_changed = out.status != st;
            progressed |= status_changed;
            out.status = st;
            if status_changed {
                out.closed_at = match st {
                    TaskStatus::Done | TaskStatus::Cancelled => Some(now),
                    _ => None,
                };
            }
        }
        if progressed {
            out.progress_at = self.next_condition_generation(now, out.progress_at);
        }
        let linked_goal = progressed
            .then(|| self.linked_goal_after_task_progress(r, &out, out.progress_at))
            .flatten();
        self.store
            .upsert_task_with_goal_progress(&out, linked_goal.as_ref(), progressed)
            .map_err(|_| TaskError::Storage)?;
        if progressed {
            let care_key = format!("task:{id}");
            r.care_marks.remove(&care_key);
            Self::resolve_condition_attention(r, room, &care_key, out.progress_at);
        }
        r.tasks.insert(id, out.clone());
        let _ = r.tx.send(ServerFrame::Task { task: out.clone() });
        if let Some(assignee) = newly_assigned.as_deref() {
            let made_as = out.assigned_as.unwrap_or(TaskActor::Operator);
            self.announce_assignment(r, room, id, &assigner_label(&req.by, made_as), assignee);
        }
        if moved != board::Transition::Plain {
            self.announce_board_move(r, room, &out, moved, &assigner_label(&req.by, actor));
        }
        if let Some(goal) = linked_goal {
            r.care_marks.remove(&format!("goal:{}", goal.id));
            Self::resolve_condition_attention(
                r,
                room,
                &format!("goal:{}", goal.id),
                goal.progress_at,
            );
            r.goals.insert(goal.id, goal.clone());
            let _ = r.tx.send(ServerFrame::Goal { goal });
        }
        Ok(out)
    }

    /// The principal a task given to `assignee` belongs to: the caller's own
    /// verified principal when they name themselves, otherwise the one active
    /// principal wearing that name. None when nobody (or more than one
    /// person) can be verified; the task then stays a name alone.
    fn assignee_principal(
        &self,
        assignee: &str,
        by: &str,
        by_principal: Option<&str>,
    ) -> Option<String> {
        if assignee == by {
            return by_principal.map(str::to_string);
        }
        self.principal_named(assignee)
    }

    /// Say in chat that `by` gave task `id` to `assignee`, addressed to the
    /// assignee alone (the `set_lead` pattern): the one line that wakes a
    /// mentions-filtered agent, visible to the whole room. The title is left
    /// out on purpose, so an `@name` written in it cannot wake anybody else;
    /// the assignee reads it from the task list.
    fn announce_assignment(&self, r: &mut Room, room: &str, id: u64, by: &str, assignee: &str) {
        let text = format!("{by} assigned task #{id} to @{assignee}.");
        self.announce_task_line(r, room, id, assignee, text);
    }
}

/// Who the assignment line names as the assigner: the Lead says so, so the
/// room can tell a Lead's decision from an operator's.
fn assigner_label(by: &str, actor: TaskActor) -> String {
    match actor {
        TaskActor::Lead => format!("Lead {by}"),
        TaskActor::Operator | TaskActor::Participant => by.to_string(),
    }
}
