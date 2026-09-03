//! Durable Attention lifecycle and delivery acknowledgement coordination.

use protocol::{Attention, AttentionStatus, CareReason, CareSignal, CreateAttention, ServerFrame};

use super::super::{AttentionError, Hub, Room};
use crate::sync::RecoverMutex;

impl Hub {
    pub(in crate::hub) fn resolve_condition_attention(
        room: &mut Room,
        room_name: &str,
        key: &str,
        at: u64,
    ) {
        let id = format!("attention:{room_name}:{key}");
        let ids: Vec<String> = room
            .attentions
            .keys()
            .filter(|candidate| **candidate == id || candidate.starts_with(&format!("{id}:")))
            .cloned()
            .collect();
        for id in ids {
            let Some(attention) = room.attentions.get_mut(&id) else {
                continue;
            };
            if attention.status == AttentionStatus::Resolved {
                continue;
            }
            attention.status = AttentionStatus::Resolved;
            attention.resolved_at = Some(at);
            let _ = room.tx.send(ServerFrame::Attention {
                attention: attention.clone(),
            });
        }
    }
    pub(in crate::hub) fn resolve_wait_attentions(
        room: &mut Room,
        room_name: &str,
        waiter: &str,
        at: u64,
    ) {
        let overdue_prefix = format!("attention:{room_name}:wait:{waiter}");
        let ids: Vec<String> = room
            .attentions
            .iter()
            .filter(|(id, attention)| {
                (**id == overdue_prefix || id.starts_with(&format!("{overdue_prefix}:")))
                    || (attention.reason == CareReason::WaitCycle
                        && attention.participants.iter().any(|name| name == waiter))
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            let Some(attention) = room.attentions.get_mut(&id) else {
                continue;
            };
            if attention.status == AttentionStatus::Resolved {
                continue;
            }
            attention.status = AttentionStatus::Resolved;
            attention.resolved_at = Some(at);
            if attention.reason == CareReason::WaitCycle {
                if let Some(signal_key) = id.strip_prefix(&format!("attention:{room_name}:")) {
                    room.care_marks.remove(signal_key);
                }
            }
            let _ = room.tx.send(ServerFrame::Attention {
                attention: attention.clone(),
            });
        }
    }
    /// Name-based replay, superseded for live delivery by `pending_care_scoped`
    /// (which is principal-aware). Retained as a test helper for the Lead/Person
    /// and wait paths that key on display name.
    #[cfg(test)]
    pub fn pending_care(&self, delivery_room: &str, owner: &str) -> Vec<CareSignal> {
        self.store.pending_care(delivery_room, owner)
    }
    /// Reconnect replay for one session, scoped to its authenticated principal. A
    /// principal-required reminder (Everyone per-member) matches only this
    /// session's principal in the store, and is additionally dropped here when the
    /// recipient's davet/membership was revoked after the snapshot — the
    /// delivery-time re-check. The outbox row is left untouched (not fake-ACKed),
    /// so the audit truth stands; it is simply not replayed.
    pub fn pending_care_scoped(
        &self,
        delivery_room: &str,
        owner: &str,
        principal_id: Option<&str>,
    ) -> Vec<CareSignal> {
        self.store
            .pending_care_scoped(delivery_room, owner, principal_id)
            .into_iter()
            .filter(|signal| match signal.owner_principal_id.as_deref() {
                Some(pid) => self.everyone_recipient_active(delivery_room, pid),
                None => true,
            })
            .collect()
    }
    /// Acknowledge a care delivery. `principal_id` is the ACKing session's
    /// authenticated canonical principal (None for a legacy/principal-less
    /// session): an Everyone per-member row (owner_principal_id set) is ACK'd
    /// only by its own principal — a shared display name can never ACK another
    /// principal's delivery. The name pre-check below stays for the legacy/hot
    /// path; the store UPDATE is the authoritative principal gate.
    pub fn ack_care(
        &self,
        signal_id: &str,
        owner: &str,
        principal_id: Option<&str>,
    ) -> rusqlite::Result<bool> {
        let now = (self.now_ms)();
        let hot_attention_id = self
            .care_deliveries
            .lock_or_recover()
            .get(signal_id)
            .cloned();
        let Some(attention_id) = hot_attention_id
            .clone()
            .or_else(|| self.store.attention_id_for_delivery(signal_id))
        else {
            return Ok(false);
        };
        let memory_attention = self
            .rooms
            .lock_or_recover()
            .values()
            .find_map(|room| room.attentions.get(&attention_id).cloned());
        let attention = memory_attention
            .clone()
            .or_else(|| self.store.attention(&attention_id));
        if attention
            .as_ref()
            .and_then(|attention| attention.owner.as_deref())
            != Some(owner)
        {
            return Ok(false);
        }

        let mut acked = self
            .store
            .ack_care_scoped(signal_id, owner, principal_id, now)?;
        if !self.store.is_persistent()
            && hot_attention_id.is_some()
            && memory_attention
                .as_ref()
                .is_some_and(|attention| attention.delivered_at.is_none())
        {
            acked = true;
        }
        if acked {
            self.care_deliveries.lock_or_recover().remove(signal_id);
            let mut rooms = self.rooms.lock_or_recover();
            for room in rooms.values_mut() {
                if let Some(attention) = room.attentions.get_mut(&attention_id) {
                    attention.delivered_at.get_or_insert(now);
                    let _ = room.tx.send(ServerFrame::Attention {
                        attention: attention.clone(),
                    });
                    break;
                }
            }
        }
        Ok(acked)
    }
    pub fn attentions(&self, room: &str) -> Vec<Attention> {
        let rooms = self.rooms.lock_or_recover();
        let mut out: Vec<_> = rooms
            .get(room)
            .map(|room| room.attentions.values().cloned().collect())
            .unwrap_or_default();
        out.sort_by_key(|attention| attention.created_at);
        out
    }
    /// Create one explicit durable attention. It is not a task and does not
    /// change goal progress; it only names who must notice the existing state.
    pub fn create_attention(
        &self,
        room_name: &str,
        req: CreateAttention,
    ) -> Result<Attention, AttentionError> {
        let now = (self.now_ms)();
        let mut rooms = self.rooms.lock_or_recover();
        let room = rooms.get(room_name).ok_or(AttentionError::NotFound)?;
        let (owner, participants) = self
            .resolve_attention_owner(room, &req.audience)
            .ok_or(AttentionError::NoRecipient)?;
        let context_len = room.settings.care_context_messages as usize;
        let start = room.history.len().saturating_sub(context_len);
        let signal = CareSignal {
            id: Self::secure_token("delivery_", 16),
            attention_id: Self::secure_token("att_", 16),
            room: room_name.to_string(),
            source_room: String::new(),
            reason: CareReason::Manual,
            audience: req.audience,
            owner: Some(owner.clone()),
            owner_principal_id: None,
            group: None,
            target: Some(owner),
            participants,
            subject: req.subject,
            created_by: req.by,
            context: room.history[start..].to_vec(),
            attempt: 1,
            at: now,
            escalated: false,
            state: protocol::ReminderState::Running,
        };
        self.store
            .enqueue_care(room_name, &signal)
            .map_err(|_| AttentionError::Storage)?;
        self.care_deliveries
            .lock_or_recover()
            .insert(signal.id.clone(), signal.attention_id.clone());
        let attention = self
            .store
            .attention(&signal.attention_id)
            .unwrap_or_else(|| Attention {
                id: signal.attention_id.clone(),
                room: signal.origin_room().to_string(),
                reason: signal.reason,
                subject: signal.subject.clone(),
                audience: signal.audience.clone(),
                owner: signal.owner.clone(),
                group: signal.group.clone(),
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
        let room = rooms.get_mut(room_name).ok_or(AttentionError::NotFound)?;
        room.attentions
            .insert(attention.id.clone(), attention.clone());
        let _ = room.tx.send(ServerFrame::Attention {
            attention: attention.clone(),
        });
        let _ = room.tx.send(ServerFrame::Care { signal });
        Ok(attention)
    }
    pub fn claim_attention(
        &self,
        room_name: &str,
        id: &str,
        actor: &str,
    ) -> Result<Attention, AttentionError> {
        let now = (self.now_ms)();
        let mut rooms = self.rooms.lock_or_recover();
        let room = rooms.get_mut(room_name).ok_or(AttentionError::NotFound)?;
        let attention = room
            .attentions
            .get(id)
            .cloned()
            .ok_or(AttentionError::NotFound)?;
        if attention.owner.as_deref() != Some(actor) {
            return Err(AttentionError::Forbidden);
        }
        if attention.status != AttentionStatus::Open {
            return Err(AttentionError::Conflict);
        }
        if !self
            .store
            .claim_attention(id, actor, now)
            .map_err(|_| AttentionError::Storage)?
        {
            return Err(AttentionError::Conflict);
        }
        // Invariant: this room stays mutably locked and `current` was cloned
        // from this exact key above; no intervening code can remove it.
        let attention = room.attentions.get_mut(id).unwrap();
        attention.status = AttentionStatus::Claimed;
        attention.claimed_by = Some(actor.to_string());
        attention.claimed_at = Some(now);
        let out = attention.clone();
        let _ = room.tx.send(ServerFrame::Attention {
            attention: out.clone(),
        });
        Ok(out)
    }
    pub fn resolve_attention(
        &self,
        room_name: &str,
        id: &str,
        actor: &str,
        is_operator: bool,
    ) -> Result<Attention, AttentionError> {
        let now = (self.now_ms)();
        let mut rooms = self.rooms.lock_or_recover();
        let room = rooms.get_mut(room_name).ok_or(AttentionError::NotFound)?;
        let current = room
            .attentions
            .get(id)
            .cloned()
            .ok_or(AttentionError::NotFound)?;
        if !is_operator
            && current.owner.as_deref() != Some(actor)
            && current.claimed_by.as_deref() != Some(actor)
        {
            return Err(AttentionError::Forbidden);
        }
        if current.status == AttentionStatus::Resolved {
            return Err(AttentionError::Conflict);
        }
        if !self
            .store
            .resolve_attention(id, now)
            .map_err(|_| AttentionError::Storage)?
        {
            return Err(AttentionError::Conflict);
        }
        // Invariant: this room stays mutably locked and `current` was cloned
        // from this exact key above; no intervening code can remove it.
        let attention = room.attentions.get_mut(id).unwrap();
        attention.status = AttentionStatus::Resolved;
        attention.resolved_at = Some(now);
        let out = attention.clone();
        let _ = room.tx.send(ServerFrame::Attention {
            attention: out.clone(),
        });
        Ok(out)
    }
}
