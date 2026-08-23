//! Bounded, condition-based Reminder scheduling and Care publication.

use std::collections::HashMap;

use protocol::{
    Attention, AttentionAudience, AttentionStatus, CareReason, CareSignal, GoalStatus,
    ReminderRecipient, ReminderState, ServerFrame, Task, TaskStatus,
};

use super::super::{CareDraft, CareMark, Hub, Room};
use crate::sync::RecoverMutex;

impl Hub {
    pub(in crate::hub) fn goal_caretaker_subject(subject: &mut String, escalated: bool) {
        subject.push_str(if escalated {
            " · Lead remains unavailable · operator review needed"
        } else {
            " · Lead unavailable · loca-care holding continuity"
        });
    }

    pub(in crate::hub) fn reminder_state(
        owner: Option<&str>,
        attempt: u32,
        escalated: bool,
    ) -> ReminderState {
        if owner == Some("loca-care") {
            ReminderState::Stalled
        } else if escalated || attempt > 1 {
            ReminderState::Overdue
        } else {
            ReminderState::Running
        }
    }

    pub(in crate::hub) fn make_care_signal(
        &self,
        room_name: &str,
        room: &Room,
        draft: CareDraft,
    ) -> CareSignal {
        let context_len = room.settings.care_context_messages as usize;
        let start = room.history.len().saturating_sub(context_len);
        let state = Self::reminder_state(draft.owner.as_deref(), draft.attempt, draft.escalated);
        let mut signal = CareSignal {
            // One id per delivery attempt. Condition identity belongs to
            // attention_id; timestamps/reasons are not unique enough for an
            // outbox primary key when two conditions fire in the same ms.
            id: Self::secure_token("delivery_", 16),
            attention_id: format!("attention:{room_name}:{}", draft.attention_key),
            room: room_name.to_string(),
            source_room: String::new(),
            reason: draft.reason,
            audience: match &room.settings.care_recipient {
                ReminderRecipient::Lead => AttentionAudience::Lead,
                ReminderRecipient::All => {
                    let mut names: Vec<String> = room
                        .members
                        .values()
                        .filter(|(_, _, count)| *count > 0)
                        .map(|(name, _, _)| name.clone())
                        .collect();
                    names.sort();
                    names.dedup();
                    AttentionAudience::Group { names }
                }
                ReminderRecipient::Person { name } => {
                    AttentionAudience::Person { name: name.clone() }
                }
            },
            owner: draft.owner,
            target: draft.target,
            participants: draft.participants,
            subject: draft.subject,
            created_by: "care".to_string(),
            context: room.history[start..].to_vec(),
            attempt: draft.attempt,
            at: draft.at,
            escalated: draft.escalated,
            state,
        };
        self.rehome_caretaker_signal(&mut signal, room_name);
        signal
    }
    fn publish_care_signal(
        &self,
        rooms: &mut HashMap<String, Room>,
        source_room: &str,
        signal: CareSignal,
        already_persisted: bool,
    ) {
        let stable_attention_id = if signal.attention_id.is_empty() {
            signal.id.as_str()
        } else {
            signal.attention_id.as_str()
        };
        let already_resolved = rooms
            .get(source_room)
            .and_then(|room| room.attentions.get(stable_attention_id))
            .is_some_and(|attention| attention.status == AttentionStatus::Resolved)
            || self
                .store
                .attention(stable_attention_id)
                .is_some_and(|attention| attention.status == AttentionStatus::Resolved);
        if already_resolved {
            return;
        }
        // The envelope is re-homed at construction, so its room already IS the
        // delivery room. Deriving delivery_room from signal.room (rather than a
        // decoupled routing key) keeps care_outbox.delivery_room == signal.room,
        // which is what guarantees a socket only ever replays care for its room.
        let delivery_room = signal.room.as_str();
        if signal.owner.is_some() {
            if !already_persisted {
                if let Err(error) = self.store.enqueue_care(delivery_room, &signal) {
                    tracing::error!(
                        %error,
                        room = %source_room,
                        signal_id = %signal.id,
                        "care signal was not published because durable enqueue failed"
                    );
                    return;
                }
            }
            self.care_deliveries
                .lock_or_recover()
                .insert(signal.id.clone(), signal.attention_id.clone());
        }
        let attention_id = if signal.attention_id.is_empty() {
            signal.id.clone()
        } else {
            signal.attention_id.clone()
        };
        let attention = self
            .store
            .attention(&attention_id)
            .unwrap_or_else(|| Attention {
                id: attention_id,
                // Ledger is keyed to the origin loca, not the delivery room.
                room: signal.origin_room().to_string(),
                reason: signal.reason,
                subject: signal.subject.clone(),
                audience: signal.audience.clone(),
                owner: signal.owner.clone(),
                participants: signal.participants.clone(),
                created_by: signal.created_by.clone(),
                created_at: signal.at,
                attempt: signal.attempt,
                escalated: signal.escalated,
                status: AttentionStatus::Open,
                delivered_at: None,
                claimed_by: None,
                claimed_at: None,
                resolved_at: None,
            });
        let frame = ServerFrame::Care {
            signal: signal.clone(),
        };
        if let Some(source) = rooms.get_mut(source_room) {
            // The browser/operator sees the state in its own room. Filtered
            // agent streams accept it only for the one named owner.
            let _ = source.tx.send(frame.clone());
            source
                .attentions
                .insert(attention.id.clone(), attention.clone());
            let _ = source.tx.send(ServerFrame::Attention { attention });
        }
        if signal.owner.as_deref() == Some("loca-care") && source_room != self.home_room.as_str() {
            let home = Self::room_mut(rooms, self.home_room.as_str(), &self.default_settings);
            let _ = home.tx.send(frame);
        }
    }
    /// Produce at most one care signal per room per sweep. Explicit waits have
    /// priority; operator-enabled task/goal/silence reminders follow. This
    /// bounds wake storms while preserving counters across restarts.
    pub fn tick_care(&self) {
        let now = (self.now_ms)();
        let mut rooms = self.rooms.lock_or_recover();
        let room_names: Vec<String> = rooms.keys().cloned().collect();
        let mut pending = Vec::new();
        for room_name in room_names {
            if rooms
                .get(&room_name)
                .is_some_and(|room| room.settings.archived)
            {
                continue;
            }
            let owner = self.care_owner(&rooms, &room_name);
            if owner.is_none() {
                // No live lead or loca-care runtime: leave counters untouched.
                // The explicit wait remains persisted and the next sweep will
                // deliver it once one attention owner is actually present.
                continue;
            }
            let Some(room) = rooms.get_mut(&room_name) else {
                continue;
            };
            let wait_after_ms = room.settings.care_wait_secs as u64 * 1_000;
            let cooldown_ms = room.settings.care_cooldown_secs as u64 * 1_000;
            let max_attempts = room.settings.care_max_attempts;
            let mut room_signal: Option<(CareSignal, bool)> = None;
            let cycles = Self::wait_cycles(room);
            let cycle_members: std::collections::HashSet<String> =
                cycles.iter().flatten().cloned().collect();
            for cycle in &cycles {
                let baseline = cycle
                    .iter()
                    .filter_map(|name| room.waits.get(name).map(|wait| wait.since))
                    .max()
                    .unwrap_or(now);
                let key = format!("wait-cycle:{}:{baseline}", cycle.join("+"));
                let mark = room.care_marks.get(&key).copied().unwrap_or(CareMark {
                    last_signal_at: 0,
                    signal_count: 0,
                });
                if mark.signal_count <= max_attempts
                    && !(mark.last_signal_at > 0
                        && now.saturating_sub(mark.last_signal_at) < cooldown_ms)
                {
                    let next_mark = CareMark {
                        last_signal_at: now,
                        signal_count: mark.signal_count.saturating_add(1),
                    };
                    let signal = self.make_care_signal(
                        &room_name,
                        room,
                        CareDraft {
                            attention_key: key.clone(),
                            owner: owner.clone(),
                            reason: CareReason::WaitCycle,
                            target: None,
                            participants: cycle.clone(),
                            subject: format!("wait cycle: {}", cycle.join(" → ")),
                            attempt: next_mark.signal_count,
                            at: now,
                            escalated: next_mark.signal_count > max_attempts,
                        },
                    );
                    let already_owned_or_done = room
                        .attentions
                        .get(&signal.attention_id)
                        .is_some_and(|attention| attention.status != AttentionStatus::Open)
                        || self
                            .store
                            .attention(&signal.attention_id)
                            .is_some_and(|attention| attention.status != AttentionStatus::Open);
                    if !already_owned_or_done {
                        // Re-homed at construction: the envelope room IS the
                        // delivery room, so a socket never replays a foreign room.
                        let delivery_room = signal.room.as_str();
                        match self.store.enqueue_care_with_mark(
                            &room_name,
                            &key,
                            next_mark.last_signal_at,
                            next_mark.signal_count,
                            delivery_room,
                            &signal,
                        ) {
                            Ok(()) => {
                                room.care_marks.insert(key, next_mark);
                                room_signal = Some((signal, true));
                                break;
                            }
                            Err(error) => {
                                tracing::error!(%error, room = %room_name, "wait-cycle attention persistence failed");
                            }
                        }
                    }
                }
            }
            let mut names: Vec<String> = if room_signal.is_none() {
                room.waits
                    .keys()
                    .filter(|name| !cycle_members.contains(*name))
                    .cloned()
                    .collect()
            } else {
                Vec::new()
            };
            names.sort();
            for name in names {
                let Some(wait) = room.waits.get(&name).cloned() else {
                    continue;
                };
                if now.saturating_sub(wait.since) < wait_after_ms {
                    continue;
                }
                if wait
                    .last_signal_at
                    .is_some_and(|last| now.saturating_sub(last) < cooldown_ms)
                {
                    continue;
                }
                // N bounded nudges, then one escalation. Never loop forever.
                if wait.signal_count > max_attempts {
                    continue;
                }
                let mut updated = wait;
                updated.signal_count = updated.signal_count.saturating_add(1);
                updated.last_signal_at = Some(now);
                let escalated = updated.signal_count > max_attempts;
                let target = updated.waiting_for.clone();
                let subject = format!(
                    "{} waits for {}: {}",
                    updated.waiter, updated.waiting_for, updated.reason
                );
                let wait_since = updated.since;
                let attempt = updated.signal_count;
                let participants = vec![updated.waiter.clone(), updated.waiting_for.clone()];
                let signal = self.make_care_signal(
                    &room_name,
                    room,
                    CareDraft {
                        attention_key: format!("wait:{name}:{wait_since}"),
                        owner: owner.clone(),
                        reason: CareReason::WaitOverdue,
                        target: Some(target),
                        participants,
                        subject,
                        attempt,
                        at: now,
                        escalated,
                    },
                );
                let already_owned_or_done = room
                    .attentions
                    .get(&signal.attention_id)
                    .is_some_and(|attention| attention.status != AttentionStatus::Open)
                    || self
                        .store
                        .attention(&signal.attention_id)
                        .is_some_and(|attention| attention.status != AttentionStatus::Open);
                if already_owned_or_done {
                    continue;
                }
                // Re-homed at construction: envelope room == delivery room.
                let delivery_room = signal.room.as_str();
                if let Err(error) = self.store.enqueue_care_with_waits(
                    std::slice::from_ref(&updated),
                    delivery_room,
                    &signal,
                ) {
                    tracing::error!(%error, room = %room_name, waiter = %name, "care wait + attention persistence failed");
                    continue;
                }
                room.waits.insert(name.clone(), updated);
                room_signal = Some((signal, true));
                break;
            }
            if room_signal.is_none() {
                let mut candidates = Vec::new();
                if room.settings.care_task_secs > 0 {
                    let mut tasks: Vec<Task> = room
                        .tasks
                        .values()
                        .filter(|task| matches!(task.status, TaskStatus::Open | TaskStatus::Taken))
                        .cloned()
                        .collect();
                    tasks.sort_by_key(|task| task.id);
                    for task in tasks {
                        let baseline = task.progress_at.max(task.created_at);
                        if now.saturating_sub(baseline)
                            >= room.settings.care_task_secs as u64 * 1_000
                        {
                            candidates.push((
                                format!("task:{}", task.id),
                                format!("task:{}:{baseline}", task.id),
                                CareReason::TaskReminder,
                                task.assigned_to.clone(),
                                task.assigned_to.clone().into_iter().collect::<Vec<_>>(),
                                format!(
                                    "task #{} is still {}: {}",
                                    task.id,
                                    match task.status {
                                        TaskStatus::Taken => "taken",
                                        _ => "open",
                                    },
                                    task.title
                                ),
                            ));
                        }
                    }
                }
                if let Some(goal) = room
                    .goals
                    .values()
                    .find(|goal| goal.status == GoalStatus::Active)
                {
                    let stale_after_secs = goal
                        .stale_after_secs
                        .unwrap_or(room.settings.care_goal_secs);
                    if stale_after_secs > 0 {
                        let baseline = goal.progress_at.max(goal.created_at);
                        if now.saturating_sub(baseline) >= stale_after_secs as u64 * 1_000 {
                            candidates.push((
                                format!("goal:{}", goal.id),
                                format!("goal:{}:{baseline}", goal.id),
                                CareReason::GoalReminder,
                                None,
                                Vec::new(),
                                format!("Goal: {}", goal.outcome),
                            ));
                        }
                    }
                }
                if room.settings.care_silence_secs > 0
                    && room.last_msg_ms > 0
                    && now.saturating_sub(room.last_msg_ms)
                        >= room.settings.care_silence_secs as u64 * 1_000
                {
                    candidates.push((
                        "silence".to_string(),
                        format!("silence:{}", room.last_msg_ms),
                        CareReason::RoomSilence,
                        None,
                        Vec::new(),
                        "operator-enabled room silence check".to_string(),
                    ));
                }

                for (key, attention_key, reason, target, participants, mut subject) in candidates {
                    let mark = room.care_marks.get(&key).copied().unwrap_or(CareMark {
                        last_signal_at: 0,
                        signal_count: 0,
                    });
                    if mark.signal_count > max_attempts
                        || (mark.last_signal_at > 0
                            && now.saturating_sub(mark.last_signal_at) < cooldown_ms)
                    {
                        continue;
                    }
                    let next_mark = CareMark {
                        last_signal_at: now,
                        signal_count: mark.signal_count.saturating_add(1),
                    };
                    let attempt = next_mark.signal_count;
                    let escalated = attempt > max_attempts;
                    if reason == CareReason::GoalReminder && owner.as_deref() == Some("loca-care") {
                        Self::goal_caretaker_subject(&mut subject, escalated);
                    }
                    let signal = self.make_care_signal(
                        &room_name,
                        room,
                        CareDraft {
                            attention_key,
                            owner: owner.clone(),
                            reason,
                            target,
                            participants,
                            subject,
                            attempt,
                            at: now,
                            escalated,
                        },
                    );
                    let already_owned_or_done = room
                        .attentions
                        .get(&signal.attention_id)
                        .is_some_and(|attention| attention.status != AttentionStatus::Open)
                        || self
                            .store
                            .attention(&signal.attention_id)
                            .is_some_and(|attention| attention.status != AttentionStatus::Open);
                    if already_owned_or_done {
                        continue;
                    }
                    // Re-homed at construction: envelope room == delivery room.
                    let delivery_room = signal.room.as_str();
                    if let Err(error) = self.store.enqueue_care_with_mark(
                        &room_name,
                        &key,
                        next_mark.last_signal_at,
                        next_mark.signal_count,
                        delivery_room,
                        &signal,
                    ) {
                        tracing::error!(%error, room = %room_name, signal_key = %key, "care mark + attention persistence failed");
                        continue;
                    }
                    room.care_marks.insert(key, next_mark);
                    room_signal = Some((signal, true));
                    break;
                }
            }
            if let Some((signal, persisted)) = room_signal {
                pending.push((room_name.clone(), signal, persisted));
            }
        }
        for (room, signal, persisted) in pending {
            self.publish_care_signal(&mut rooms, &room, signal, persisted);
        }
    }
}
