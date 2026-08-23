use super::*;

impl Store {
    pub fn upsert_task(&self, t: &Task) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else { return Ok(()) };
        Self::write_task(&c, t)
    }
    fn write_task(c: &Connection, t: &Task) -> rusqlite::Result<()> {
        let st = match t.status {
            TaskStatus::Open => "open",
            TaskStatus::Taken => "taken",
            TaskStatus::Done => "done",
            TaskStatus::Cancelled => "cancelled",
        };
        c.execute(
            "INSERT OR REPLACE INTO tasks (id, room, title, created_by, from_message, assigned_to, status, created_at, progress_at, closed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![t.id, t.room, t.title, t.created_by, t.from_message, t.assigned_to, st, t.created_at, t.progress_at, t.closed_at],
        )
        .inspect_err(|e| tracing::error!(error = %e, "upsert_task failed"))
        .map(|_| ())
    }
    pub fn upsert_goal(&self, goal: &Goal) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else { return Ok(()) };
        Self::write_goal(&c, goal)
    }
    /// Persist a direct goal transition and reset its reminder in the same
    /// transaction. A failed reminder reset must roll the goal write back.
    pub fn upsert_goal_with_care_reset(
        &self,
        goal: &Goal,
        clear_care: bool,
    ) -> rusqlite::Result<()> {
        let Some(mut c) = self.conn() else {
            return Ok(());
        };
        let tx = c.transaction()?;
        Self::write_goal(&tx, goal)?;
        if clear_care {
            let attention_id = format!("attention:{}:goal:{}", goal.room, goal.id);
            tx.execute(
                "DELETE FROM care_marks WHERE room = ?1 AND signal_key = ?2",
                params![goal.room, format!("goal:{}", goal.id)],
            )?;
            tx.execute(
                "UPDATE attentions SET resolved_at = COALESCE(resolved_at, ?2)
                 WHERE id = ?1 OR substr(id, 1, length(?1) + 1) = ?1 || ':'",
                params![attention_id, goal.progress_at],
            )?;
            tx.execute(
                "UPDATE care_outbox SET acked_at = COALESCE(acked_at, ?2)
                 WHERE attention_id = ?1 OR substr(attention_id, 1, length(?1) + 1) = ?1 || ':'",
                params![attention_id, goal.progress_at],
            )?;
        }
        tx.commit().inspect_err(|e| {
            tracing::error!(error = %e, room = %goal.room, goal_id = goal.id, "atomic goal progress failed")
        })
    }
    fn write_goal(c: &Connection, goal: &Goal) -> rusqlite::Result<()> {
        let completion = match goal.completion {
            GoalCompletion::Manual => "manual",
            GoalCompletion::AllTasks => "all_tasks",
        };
        let status = match goal.status {
            GoalStatus::Active => "active",
            GoalStatus::Achieved => "achieved",
            GoalStatus::Cancelled => "cancelled",
        };
        let task_ids = serde_json::to_string(&goal.task_ids).unwrap_or_else(|_| "[]".into());
        c.execute(
            "INSERT OR REPLACE INTO goals
             (id, room, outcome, checkpoint, stale_after_secs, created_by, completion, task_ids, status, created_at, progress_at, closed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                goal.id,
                goal.room,
                goal.outcome,
                goal.checkpoint,
                goal.stale_after_secs,
                goal.created_by,
                completion,
                task_ids,
                status,
                goal.created_at,
                goal.progress_at,
                goal.closed_at
            ],
        )
        .inspect_err(|e| tracing::error!(error = %e, "upsert_goal failed"))
        .map(|_| ())
    }
    /// Commit a task transition, its linked goal progress, and the reminder
    /// resets as one state change. A 503 must never leave the task advanced
    /// while its goal and care clock remain stale.
    pub fn upsert_task_with_goal_progress(
        &self,
        task: &Task,
        goal: Option<&Goal>,
        clear_task_care: bool,
    ) -> rusqlite::Result<()> {
        let Some(mut c) = self.conn() else {
            return Ok(());
        };
        let tx = c.transaction()?;
        Self::write_task(&tx, task)?;
        if let Some(goal) = goal {
            Self::write_goal(&tx, goal)?;
        }
        if clear_task_care {
            let attention_id = format!("attention:{}:task:{}", task.room, task.id);
            tx.execute(
                "DELETE FROM care_marks WHERE room = ?1 AND signal_key = ?2",
                params![task.room, format!("task:{}", task.id)],
            )?;
            tx.execute(
                "UPDATE attentions SET resolved_at = COALESCE(resolved_at, ?2)
                 WHERE id = ?1 OR substr(id, 1, length(?1) + 1) = ?1 || ':'",
                params![attention_id, task.progress_at],
            )?;
            tx.execute(
                "UPDATE care_outbox SET acked_at = COALESCE(acked_at, ?2)
                 WHERE attention_id = ?1 OR substr(attention_id, 1, length(?1) + 1) = ?1 || ':'",
                params![attention_id, task.progress_at],
            )?;
        }
        if let Some(goal) = goal {
            let attention_id = format!("attention:{}:goal:{}", goal.room, goal.id);
            tx.execute(
                "DELETE FROM care_marks WHERE room = ?1 AND signal_key = ?2",
                params![goal.room, format!("goal:{}", goal.id)],
            )?;
            tx.execute(
                "UPDATE attentions SET resolved_at = COALESCE(resolved_at, ?2)
                 WHERE id = ?1 OR substr(id, 1, length(?1) + 1) = ?1 || ':'",
                params![attention_id, goal.progress_at],
            )?;
            tx.execute(
                "UPDATE care_outbox SET acked_at = COALESCE(acked_at, ?2)
                 WHERE attention_id = ?1 OR substr(attention_id, 1, length(?1) + 1) = ?1 || ':'",
                params![attention_id, goal.progress_at],
            )?;
        }
        tx.commit()
            .inspect_err(|e| tracing::error!(error = %e, room = %task.room, task_id = task.id, "atomic task/goal progress failed"))
    }
    pub fn load_tasks(&self, room: &str) -> Vec<Task> {
        let Some(c) = self.conn() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if let Ok(mut stmt) = c.prepare(
            "SELECT id, title, created_by, from_message, assigned_to, status, created_at, progress_at, closed_at
             FROM tasks WHERE room = ?1 ORDER BY id",
        ) {
            if let Ok(rows) = stmt.query_map(params![room], |r| {
                let st: String = r.get(5)?;
                Ok(Task {
                    id: r.get(0)?,
                    room: room.to_string(),
                    title: r.get(1)?,
                    created_by: r.get(2)?,
                    from_message: r.get(3)?,
                    assigned_to: r.get(4)?,
                    status: match st.as_str() {
                        "taken" => TaskStatus::Taken,
                        "done" => TaskStatus::Done,
                        "cancelled" => TaskStatus::Cancelled,
                        _ => TaskStatus::Open,
                    },
                    created_at: r.get(6)?,
                    progress_at: r.get(7)?,
                    closed_at: r.get(8)?,
                })
            }) {
                out.extend(rows.flatten());
            }
        }
        out
    }
    pub fn load_goals(&self, room: &str) -> Vec<Goal> {
        let Some(c) = self.conn() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if let Ok(mut stmt) = c.prepare(
            "SELECT id, outcome, checkpoint, stale_after_secs, created_by, completion, task_ids, status, created_at, progress_at, closed_at
             FROM goals WHERE room = ?1 ORDER BY id",
        ) {
            if let Ok(rows) = stmt.query_map(params![room], |r| {
                let completion: String = r.get(5)?;
                let task_ids: String = r.get(6)?;
                let status: String = r.get(7)?;
                Ok(Goal {
                    id: r.get(0)?,
                    room: room.to_string(),
                    outcome: r.get(1)?,
                    checkpoint: r.get(2)?,
                    stale_after_secs: r.get(3)?,
                    created_by: r.get(4)?,
                    completion: if completion == "all_tasks" {
                        GoalCompletion::AllTasks
                    } else {
                        GoalCompletion::Manual
                    },
                    task_ids: serde_json::from_str(&task_ids).unwrap_or_default(),
                    status: match status.as_str() {
                        "achieved" => GoalStatus::Achieved,
                        "cancelled" => GoalStatus::Cancelled,
                        _ => GoalStatus::Active,
                    },
                    created_at: r.get(8)?,
                    progress_at: r.get(9)?,
                    closed_at: r.get(10)?,
                })
            }) {
                out.extend(rows.flatten());
            }
        }
        out
    }
    pub(in crate::store) fn write_wait(c: &Connection, wait: &WaitState) -> rusqlite::Result<()> {
        c.execute(
            "INSERT OR REPLACE INTO waits
             (room, waiter, waiting_for, reason, since, last_signal_at, signal_count)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                wait.room,
                wait.waiter,
                wait.waiting_for,
                wait.reason,
                wait.since,
                wait.last_signal_at,
                wait.signal_count
            ],
        )
        .inspect_err(|e| tracing::error!(error = %e, "upsert_wait failed"))
        .map(|_| ())
    }
    /// Replace a declared dependency and retire the prior generation's care
    /// state atomically. A new edge is progress; its old reminder must not
    /// remain claimable after the replacement succeeds.
    pub fn replace_wait_with_care(&self, wait: &WaitState, at: u64) -> rusqlite::Result<()> {
        let Some(mut c) = self.conn() else {
            return Ok(());
        };
        let tx = c.transaction()?;
        Self::write_wait(&tx, wait)?;
        tx.execute(
            "DELETE FROM care_marks WHERE room = ?1 AND signal_key = ?2",
            params![wait.room, format!("wait:{}", wait.waiter)],
        )?;
        Self::resolve_wait_attentions(&tx, &wait.room, &wait.waiter, at)?;
        tx.commit()
            .inspect_err(|e| tracing::error!(error = %e, "replace wait failed"))
    }
    pub fn delete_wait(&self, room: &str, waiter: &str, at: u64) -> rusqlite::Result<()> {
        let Some(mut c) = self.conn() else {
            return Ok(());
        };
        let tx = c.transaction()?;
        tx.execute(
            "DELETE FROM waits WHERE room = ?1 AND waiter = ?2",
            params![room, waiter],
        )?;
        tx.execute(
            "DELETE FROM care_marks WHERE room = ?1 AND signal_key = ?2",
            params![room, format!("wait:{waiter}")],
        )?;
        Self::resolve_wait_attentions(&tx, room, waiter, at)?;
        tx.commit()
            .inspect_err(|e| tracing::error!(error = %e, "delete_wait failed"))
    }
    pub fn load_waits(&self, room: &str) -> Vec<WaitState> {
        let Some(c) = self.conn() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if let Ok(mut stmt) = c.prepare(
            "SELECT waiter, waiting_for, reason, since, last_signal_at, signal_count
             FROM waits WHERE room = ?1 ORDER BY waiter",
        ) {
            if let Ok(rows) = stmt.query_map(params![room], |row| {
                Ok(WaitState {
                    room: room.to_string(),
                    waiter: row.get(0)?,
                    waiting_for: row.get(1)?,
                    reason: row.get(2)?,
                    since: row.get(3)?,
                    last_signal_at: row.get(4)?,
                    signal_count: row.get(5)?,
                })
            }) {
                out.extend(rows.flatten());
            }
        }
        out
    }
}
