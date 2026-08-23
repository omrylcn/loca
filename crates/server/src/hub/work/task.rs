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
    pub fn create_task(&self, room: &str, req: CreateTask) -> rusqlite::Result<Task> {
        let now = (self.now_ms)();
        let mut rooms = self.rooms.lock_or_recover();
        let id = rooms.get(room).map(|r| r.next_task_id).unwrap_or(1);
        let t = Task {
            id,
            room: room.to_string(),
            title: req.title,
            created_by: req.by,
            from_message: req.from_message,
            assigned_to: req.assigned_to,
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
        Ok(t)
    }
    /// Update a task. Rules of the house:
    /// - the assigned agent may TAKE it and mark it DONE (agent finishes),
    /// - only an operator may cancel, reopen or reassign (operator contests).
    ///
    /// `is_operator` is resolved at the route layer.
    pub fn update_task(
        &self,
        room: &str,
        id: u64,
        req: UpdateTask,
        is_operator: bool,
    ) -> Result<Task, TaskError> {
        let now = (self.now_ms)();
        let mut rooms = self.rooms.lock_or_recover();
        let r = rooms.get_mut(room).ok_or(TaskError::NotFound)?;
        let mut out = r.tasks.get(&id).cloned().ok_or(TaskError::NotFound)?;
        let mut progressed = false;

        if let Some(assignee) = req.assigned_to.clone() {
            if !is_operator {
                // An unassigned task may be self-claimed by whoever takes it;
                // stealing someone else's assignment needs operator authority.
                if out.assigned_to.is_some() && out.assigned_to.as_deref() != Some(req.by.as_str())
                {
                    return Err(TaskError::Forbidden);
                }
                if assignee != req.by {
                    return Err(TaskError::Forbidden);
                }
            }
            progressed |= out.assigned_to.as_deref() != Some(assignee.as_str());
            out.assigned_to = Some(assignee);
        }
        if let Some(st) = req.status {
            let mine = out.assigned_to.as_deref() == Some(req.by.as_str());
            let allowed = match st {
                TaskStatus::Taken | TaskStatus::Done => is_operator || mine,
                TaskStatus::Cancelled | TaskStatus::Open => is_operator,
            };
            if !allowed {
                return Err(TaskError::Forbidden);
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
}
