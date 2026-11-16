use super::super::*;
use crate::sync::RecoverMutex;

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
    pub fn create_task(
        &self,
        room: &str,
        req: CreateTask,
        by_principal: Option<&str>,
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
            created_by: req.by,
            from_message: req.from_message,
            assigned_to: req.assigned_to,
            assigned_principal,
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
            self.announce_assignment(r, room, id, &t.created_by, assignee);
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
    pub fn update_task(
        &self,
        room: &str,
        id: u64,
        req: UpdateTask,
        is_operator: bool,
        by_principal: Option<&str>,
    ) -> Result<Task, TaskError> {
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
            if !is_operator {
                // An unassigned task may be self-claimed by whoever takes it;
                // stealing someone else's assignment needs operator authority.
                if out.assigned_to.is_some() && !is_assignee(&out) {
                    return Err(TaskError::Forbidden);
                }
                if assignee != req.by {
                    return Err(TaskError::Forbidden);
                }
            }
            if out.assigned_to.as_deref() != Some(assignee.as_str()) {
                progressed = true;
                out.assigned_principal = new_principal;
                if assignee != req.by {
                    newly_assigned = Some(assignee.clone());
                }
            }
            out.assigned_to = Some(assignee);
        }
        if let Some(st) = req.status {
            let mine = is_assignee(&out);
            let allowed = match st {
                TaskStatus::Taken | TaskStatus::Done => is_operator || mine,
                TaskStatus::Cancelled | TaskStatus::Open => is_operator,
            };
            if !allowed {
                return Err(TaskError::Forbidden);
            }
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
            self.announce_assignment(r, room, id, &req.by, assignee);
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
        let msg = Message {
            attachments: Vec::new(),
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            room: room.to_string(),
            sender: "loca".into(),
            sender_type: SenderType::User,
            target: Some(assignee.to_string()),
            text: format!("{by} assigned task #{id} to @{assignee}."),
            reply_to: None,
            reply_to_sender: None,
            sender_principal: None,
            kind: protocol::MessageKind::Announce,
            ts: (self.now_ms)(),
        };
        // The task change has already committed; its announcement must not
        // depend on a second write (the `announce_join_request` rule). A
        // failed write is logged and the line is still shown and delivered.
        if let Err(error) = self.store.insert_message(&msg, None) {
            tracing::error!(%error, room, task_id = id, "could not persist a task assignment line");
        }
        r.history.push(msg.clone());
        if r.history.len() > HISTORY_LIMIT {
            let overflow = r.history.len() - HISTORY_LIMIT;
            r.history.drain(0..overflow);
        }
        let _ = r.tx.send(ServerFrame::Msg { message: msg });
    }
}
