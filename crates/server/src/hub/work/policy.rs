use super::super::*;

impl Hub {
    pub(in crate::hub) fn normalize_goal_tasks(
        room: &Room,
        completion: GoalCompletion,
        task_ids: &mut Vec<u64>,
    ) -> Result<(), GoalError> {
        task_ids.sort_unstable();
        task_ids.dedup();
        if completion == GoalCompletion::Manual {
            task_ids.clear();
            return Ok(());
        }
        if task_ids.is_empty() || task_ids.iter().any(|id| !room.tasks.contains_key(id)) {
            return Err(GoalError::InvalidTasks);
        }
        Ok(())
    }
    pub(in crate::hub) fn all_goal_tasks_done(room: &Room, goal: &Goal) -> bool {
        goal.completion == GoalCompletion::AllTasks
            && !goal.task_ids.is_empty()
            && goal.task_ids.iter().all(|id| {
                room.tasks
                    .get(id)
                    .map(|task| task.status == TaskStatus::Done)
                    .unwrap_or(false)
            })
    }
    pub(in crate::hub) fn linked_goal_after_task_progress(
        &self,
        room: &Room,
        task: &Task,
        now: u64,
    ) -> Option<Goal> {
        let id = room.goals.values().find_map(|goal| {
            (goal.status == GoalStatus::Active && goal.task_ids.contains(&task.id))
                .then_some(goal.id)
        })?;
        let mut goal = room.goals.get(&id).cloned().expect("goal id came from map");
        goal.progress_at = self.next_condition_generation(now, goal.progress_at);
        let all_done = goal.task_ids.iter().all(|id| {
            if *id == task.id {
                task.status == TaskStatus::Done
            } else {
                room.tasks
                    .get(id)
                    .is_some_and(|linked| linked.status == TaskStatus::Done)
            }
        });
        if all_done {
            goal.status = GoalStatus::Achieved;
            goal.closed_at = Some(now);
        }
        Some(goal)
    }
}
