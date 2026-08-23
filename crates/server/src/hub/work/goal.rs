use super::super::*;
use crate::sync::RecoverMutex;

impl Hub {
    pub fn goals(&self, room: &str) -> Vec<Goal> {
        let rooms = self.rooms.lock_or_recover();
        let mut list: Vec<Goal> = rooms
            .get(room)
            .map(|r| r.goals.values().cloned().collect())
            .unwrap_or_default();
        list.sort_by_key(|goal| goal.id);
        list
    }
    /// Create the loca's one active outcome. This is called only after the
    /// route proves operator authority; conversation never reaches it.
    pub fn create_goal(&self, room: &str, req: CreateGoal) -> Result<Goal, GoalError> {
        let now = (self.now_ms)();
        let mut rooms = self.rooms.lock_or_recover();
        let r = Self::room_mut(&mut rooms, room, &self.default_settings);
        if r.settings.lead.is_none() {
            return Err(GoalError::LeadRequired);
        }
        if r.goals
            .values()
            .any(|goal| goal.status == GoalStatus::Active)
        {
            return Err(GoalError::ActiveExists);
        }
        let mut task_ids = req.task_ids;
        Self::normalize_goal_tasks(r, req.completion, &mut task_ids)?;
        let id = r.next_goal_id;
        let mut goal = Goal {
            id,
            room: room.to_string(),
            outcome: req.outcome,
            checkpoint: req.checkpoint,
            stale_after_secs: req.stale_after_secs,
            created_by: req.by,
            completion: req.completion,
            task_ids,
            status: GoalStatus::Active,
            created_at: now,
            progress_at: now,
            closed_at: None,
        };
        if Self::all_goal_tasks_done(r, &goal) {
            goal.status = GoalStatus::Achieved;
            goal.closed_at = Some(now);
        }
        self.store
            .upsert_goal(&goal)
            .map_err(|_| GoalError::Storage)?;
        r.next_goal_id += 1;
        r.goals.insert(id, goal.clone());
        let _ = r.tx.send(ServerFrame::Goal { goal: goal.clone() });
        Ok(goal)
    }
    /// Change/close/reopen a goal. Only an operator reaches this method.
    pub fn update_goal(&self, room: &str, id: u64, req: UpdateGoal) -> Result<Goal, GoalError> {
        let now = (self.now_ms)();
        let mut rooms = self.rooms.lock_or_recover();
        let r = rooms.get_mut(room).ok_or(GoalError::NotFound)?;
        let mut goal = r.goals.get(&id).cloned().ok_or(GoalError::NotFound)?;
        let previous = (
            goal.outcome.clone(),
            goal.checkpoint.clone(),
            goal.stale_after_secs,
            goal.completion,
            goal.task_ids.clone(),
            goal.status,
        );
        if let Some(outcome) = req.outcome {
            goal.outcome = outcome;
        }
        if let Some(checkpoint) = req.checkpoint {
            goal.checkpoint = (!checkpoint.is_empty()).then_some(checkpoint);
        }
        if let Some(stale_after_secs) = req.stale_after_secs {
            goal.stale_after_secs = stale_after_secs;
        }
        if let Some(completion) = req.completion {
            goal.completion = completion;
        }
        if let Some(task_ids) = req.task_ids {
            goal.task_ids = task_ids;
        }
        Self::normalize_goal_tasks(r, goal.completion, &mut goal.task_ids)?;
        if let Some(status) = req.status {
            if status == GoalStatus::Active
                && r.goals
                    .values()
                    .any(|other| other.id != id && other.status == GoalStatus::Active)
            {
                return Err(GoalError::ActiveExists);
            }
            let status_changed = goal.status != status;
            goal.status = status;
            if status_changed {
                goal.closed_at = match status {
                    GoalStatus::Active => None,
                    GoalStatus::Achieved | GoalStatus::Cancelled => Some(now),
                };
            }
        }
        if goal.status == GoalStatus::Active && r.settings.lead.is_none() {
            return Err(GoalError::LeadRequired);
        }
        if goal.status == GoalStatus::Active && Self::all_goal_tasks_done(r, &goal) {
            goal.status = GoalStatus::Achieved;
            goal.closed_at = Some(now);
        }
        // Compare canonical state only after task ids have been sorted and
        // deduplicated. Reordering [1,2] as [2,1] is a no-op, not progress.
        let progressed = previous
            != (
                goal.outcome.clone(),
                goal.checkpoint.clone(),
                goal.stale_after_secs,
                goal.completion,
                goal.task_ids.clone(),
                goal.status,
            );
        if progressed {
            goal.progress_at = self.next_condition_generation(now, goal.progress_at);
        }
        self.store
            .upsert_goal_with_care_reset(&goal, progressed)
            .map_err(|_| GoalError::Storage)?;
        if progressed {
            let care_key = format!("goal:{id}");
            r.care_marks.remove(&care_key);
            Self::resolve_condition_attention(r, room, &care_key, goal.progress_at);
        }
        r.goals.insert(id, goal.clone());
        let _ = r.tx.send(ServerFrame::Goal { goal: goal.clone() });
        Ok(goal)
    }
}
