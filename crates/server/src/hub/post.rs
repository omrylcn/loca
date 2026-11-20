//! Posting a message and reacting to one.

use super::*;

impl Hub {
    /// Append a posted message, trim history, and broadcast it. The current
    /// chat mode is enforced here: `is_admin` bypasses gating (so the admin can
    /// always speak, e.g. while paused). Returns `Err(PostReject)` if the mode
    /// forbids this sender. `scope` scopes `op_id` replays
    /// (`Hub::operation_scope`); the separate `sender_principal` is the
    /// verified principal id that wrote the message (`None` when the door
    /// verified nobody), which a rename follows.
    pub fn post(
        &self,
        room: &str,
        body: PostMessage,
        is_admin: bool,
        scope: &OperationScope,
        sender_principal: Option<&str>,
    ) -> Result<Message, PostReject> {
        // A sealed (deleted) loca does not come back on a post. Without this,
        // `room_mut` (get-or-create) would resurrect a tombstoned room the
        // moment anyone with a stale invite posted to it — the room the master
        // shut would silently reopen. A permanent seal cannot be cleared by
        // inviting or posting; create a different loca for new work.
        if self.is_deleted(room) {
            return Err(PostReject::Deleted);
        }
        let mut body = body;
        if let Some(b) = self.dm(room) {
            // A side that left the building leaves the direct message read-only.
            if self.dm_read_only(&b) {
                return Err(PostReject::Archived);
            }
            // Every word in a direct message is said to the other person: it wakes
            // an agent listening for mentions without an `@`, and an `@all`
            // left over from another loca cannot widen it.
            if let Some(peer) = self.dm_peer_of(room, &body.sender) {
                body.target = Some(peer);
            }
        } else if body.sender_type == SenderType::Agent
            && body.target.as_deref() == Some("all")
            && names_people_but_not_all(&body.text)
        {
            // An agent that names people (`@ayse @mehmet ...`) but sent the
            // `all` target would wake every agent at the table. The named
            // people are reached by their `@name` already; `@all` in the text
            // is how an agent really calls everyone.
            body.target = None;
        }
        let op_id = body.op_id.filter(|id| !id.is_empty());
        if let Some(id) = op_id.as_deref() {
            match self.store.message_by_operation(room, &scope.key, id) {
                Ok(Some(message)) => return Ok(message),
                Ok(None) => {}
                Err(_) => return Err(PostReject::Storage),
            }
            // An operation the previous version stored under its REST key
            // answers the re-send only when it is provably this sender's.
            if let Some(legacy) = scope.legacy.as_deref() {
                match self.store.message_by_operation(room, legacy, id) {
                    Ok(Some(message)) => {
                        match Self::legacy_operation_owner(scope, legacy, &message) {
                            LegacyOwner::Sender => return Ok(message),
                            LegacyOwner::Other => {}
                            LegacyOwner::Unknown => return Err(PostReject::OperationUnclear),
                        }
                    }
                    Ok(None) => {}
                    Err(_) => return Err(PostReject::Storage),
                }
            }
        }
        // Resolve cited attachments BEFORE anything persists: each id must be an
        // available upload in THIS room. Rejecting up front means a durable
        // message can never cite a blob the server can't serve, and no
        // cross-room hash guess resolves. The resolved refs ride on the message
        // (durable JSON) and are flipped pending→referenced after the commit.
        let attachments: Vec<protocol::Attachment> = if body.attachments.is_empty() {
            Vec::new()
        } else {
            if body.attachments.len() > MAX_ATTACHMENTS_PER_MESSAGE {
                return Err(PostReject::TooManyAttachments {
                    max: MAX_ATTACHMENTS_PER_MESSAGE,
                });
            }
            let mut resolved: Vec<protocol::Attachment> =
                Vec::with_capacity(body.attachments.len());
            for id in &body.attachments {
                // Dedup: citing the same file twice is one reference, and keeps
                // each (message, sha) unique so the in-tx claim never sees a
                // spurious zero-row insert.
                if resolved.iter().any(|a| &a.id == id) {
                    continue;
                }
                match self.store.resolve_room_attachment(room, id) {
                    Some(a) => resolved.push(a),
                    None => return Err(PostReject::UnknownAttachment),
                }
            }
            resolved
        };
        let now = (self.now_ms)();
        let mut rooms = self.rooms.lock_or_recover();
        let home_tx = (room != self.home_room.as_str())
            .then(|| {
                rooms
                    .get(self.home_room.as_str())
                    .map(|home| home.tx.clone())
            })
            .flatten();
        let r = Self::room_mut(&mut rooms, room, &self.default_settings);
        if let Some(id) = op_id.as_deref() {
            let key = (scope.key.clone(), id.to_string());
            if let Some(message) = r.operations.get(&key) {
                return Ok(message.clone());
            }
        }

        // Archived rooms are read-only for everyone, admin included: closing a
        // room is a deliberate state, not a moderation nudge.
        if r.settings.archived {
            return Err(PostReject::Archived);
        }

        // ---- per-participant moderation (admin bypasses) ----
        if !is_admin {
            if r.banned.contains(&body.sender) {
                return Err(PostReject::Banned);
            }
            if r.muted.contains(&body.sender) {
                return Err(PostReject::Muted);
            }
        }

        // ---- mode enforcement (admin bypasses) ----
        if !is_admin {
            match &r.mode {
                ChatMode::Free => {}
                ChatMode::Paused => return Err(PostReject::Paused),
                ChatMode::Restricted { allow } => {
                    if !allow.contains(&body.sender) {
                        return Err(PostReject::NotAllowed);
                    }
                }
                ChatMode::RoundRobin { order, turn } => match order.get(*turn) {
                    Some(cur) if cur == &body.sender => {}
                    _ => {
                        return Err(PostReject::NotYourTurn {
                            whose: order.get(*turn).cloned(),
                        })
                    }
                },
            }
        }

        // ---- rate limit (sliding window; admin bypasses; 0 = disabled) ----
        let record_rate = !is_admin && r.settings.rate_limit > 0;
        if record_rate {
            let window_ms = r.settings.rate_window_secs as u64 * 1000;
            let cutoff = now.saturating_sub(window_ms);
            let times = r.post_times.entry(body.sender.clone()).or_default();
            while times.front().is_some_and(|&t| t <= cutoff) {
                times.pop_front();
            }
            if times.len() as u32 >= r.settings.rate_limit {
                let retry_ms = times
                    .front()
                    .map(|&t| (t + window_ms).saturating_sub(now))
                    .unwrap_or(window_ms);
                return Err(PostReject::RateLimited {
                    retry_after_secs: retry_ms.div_ceil(1000),
                });
            }
        }

        // A reply addresses the AUTHOR of the message it answers, as a separate
        // recipient alongside any explicit `target` — so a reply that also
        // targets/@mentions someone else wakes BOTH. `msg_addresses` keys wake
        // on `target`/`@name` and never looked at `reply_to`, so a bare reply
        // used to be a silent UI-only link. Resolve the author from the room's
        // in-memory tail first (you usually reply to what you can see), then the
        // durable store — the reply UI allows answering a message older than the
        // 200-message tail, and those must wake their author too; a cross-room
        // or unknown id resolves to nobody (message_owner is room-scoped), and a
        // self-reply never addresses the sender. `target` is left untouched.
        let reply_to_sender = match body.reply_to {
            None => None,
            Some(rid) => {
                let owner = match r.history.iter().rev().find(|m| m.id == rid) {
                    Some(m) => Some(m.sender.clone()),
                    // Only consult the store on a tail miss. A lookup ERROR must
                    // reject the post (503) — never silently drop the wake and
                    // report success. An unknown id is a real absence (Ok(None))
                    // and safely stays recipient-less.
                    None => self
                        .store
                        .message_owner(room, rid)
                        .map_err(|_| PostReject::Storage)?,
                };
                owner.filter(|owner| owner != &body.sender && !owner.is_empty())
            }
        };
        let msg = Message {
            attachments: attachments.clone(),
            kind: body.kind,
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            room: room.to_string(),
            sender: body.sender,
            sender_type: body.sender_type,
            target: body.target.filter(|t| !t.is_empty()),
            text: body.text,
            reply_to: body.reply_to,
            reply_to_sender,
            ts: self.next_condition_generation(now, r.last_msg_ms),
            sender_principal: sender_principal.map(str::to_string),
        };
        // Nothing leaves a direct message or a group loca: a caretaker summons would
        // copy its words into Care, where every rank reads (PRINCIPLES
        // "Birebir", "Groups").
        let caretaker_summons: Vec<CareSignal> =
            if room == self.home_room.as_str() || self.is_dm(room) || self.is_group_loca(room) {
                Vec::new()
            } else {
                self.addressed_caretakers(&msg)
                    .into_iter()
                    .map(|owner| CareSignal {
                        id: format!("summon:{room}:{}:{owner}", msg.id),
                        attention_id: format!("attention:{room}:summon:{}:{owner}", msg.id),
                        // Re-home the envelope onto the caretaker's home loca (where
                        // it is delivered) so signal.room matches the receiving
                        // socket. The true origin travels in source_room; the
                        // attention_id and the ledger stay keyed to the source loca.
                        room: self.home_room.as_ref().clone(),
                        source_room: room.to_string(),
                        reason: CareReason::DirectSummon,
                        audience: AttentionAudience::Person {
                            name: owner.clone(),
                        },
                        owner: Some(owner.clone()),
                        owner_principal_id: None,
                        group: None,
                        target: Some(owner.clone()),
                        participants: vec![msg.sender.clone(), owner],
                        subject: format!("{} directly summoned a caretaker", msg.sender),
                        created_by: msg.sender.clone(),
                        context: vec![msg.clone()],
                        attempt: 1,
                        at: now,
                        escalated: false,
                        state: protocol::ReminderState::Running,
                    })
                    .collect()
            };

        // P0#1: an explicit direct reply re-wakes exactly the waiter it
        // answers. The trigger is deliberately strict and structured — no
        // progress inference: an ordinary turn (`Say`) whose `target` names a
        // participant who is *currently waiting for this very sender*. `target`
        // must be that structured address, never `None`, never the "all"
        // broadcast, and we do NOT parse a free-text `@mention`. The owner is
        // always the waiter, so the sender can never wake itself, and only the
        // addressed waiter is touched (other waiters-on-sender are left alone).
        // Kept mutually exclusive with a caretaker summon: when the same turn
        // already summons the addressed caretaker, that summon does the waking.
        let wait_wake: Option<(WaitState, CareSignal)> = (msg.kind == protocol::MessageKind::Say
            && caretaker_summons.is_empty())
        .then(|| {
            let target = msg.target.as_deref()?;
            if target.eq_ignore_ascii_case("all") || target == msg.sender {
                return None;
            }
            let existing = r.waits.get(target)?;
            if existing.waiting_for != msg.sender {
                return None;
            }
            let new_since = self.next_condition_generation(now, existing.since);
            let updated = WaitState {
                room: room.to_string(),
                waiter: target.to_string(),
                waiting_for: existing.waiting_for.clone(),
                reason: existing.reason.clone(),
                since: new_since,
                last_signal_at: None,
                signal_count: 0,
            };
            let wake = CareSignal {
                id: Self::secure_token("delivery_", 16),
                attention_id: format!("attention:{room}:wait-reply:{target}:{new_since}"),
                room: room.to_string(),
                // Same-room wake: origin == delivery room, so source_room stays empty.
                source_room: String::new(),
                reason: CareReason::WaitReplied,
                audience: AttentionAudience::Person {
                    name: target.to_string(),
                },
                owner: Some(target.to_string()),
                owner_principal_id: None,
                group: None,
                target: Some(target.to_string()),
                participants: vec![msg.sender.clone(), target.to_string()],
                subject: format!("{} replied to {}", msg.sender, target),
                created_by: msg.sender.clone(),
                context: vec![msg.clone()],
                attempt: 1,
                at: now,
                escalated: false,
                state: protocol::ReminderState::Running,
            };
            Some((updated, wake))
        })
        .flatten();

        // (Lead is no longer set by parsing chat. "Speaking must never create a hidden side effect":
        // naming a lead is an explicit act — POST /rooms/:id/lead — not a
        // message the server secretly interprets. See `set_lead`.)

        // Advance the round-robin turn on an accepted non-admin (or admin) post
        // by the current speaker, so the baton actually moves.
        let mut previous_turn = None;
        if let ChatMode::RoundRobin { order, turn } = &mut r.mode {
            if !order.is_empty() && order.get(*turn) == Some(&msg.sender) {
                previous_turn = Some(*turn);
                *turn = (*turn + 1) % order.len();
            }
        }

        // Persist BEFORE broadcast: a message is not "sent" until it is
        // durable. If the write fails we refuse rather than broadcast a line
        // that a restart would forget (PRINCIPLES: "Messages are not lost" — a
        // successful reply means the word landed). Memory-only mode returns Ok.
        let operation = op_id.as_deref().map(|id| (scope.key.as_str(), id));
        let persisted = if let Some((updated_wait, wake)) = wait_wake.as_ref() {
            // The waiter is a member of this room, so the wake is delivered
            // here (no home-room hop). Message + overdue suppression + wait
            // generation advance + durable wake all commit as one.
            self.store.insert_message_with_wait_wake(
                &msg,
                previous_turn.is_some().then_some((&r.mode, &r.settings)),
                operation,
                room,
                &updated_wait.waiter,
                now,
                updated_wait,
                wake,
            )
        } else if !caretaker_summons.is_empty() {
            self.store.insert_message_with_care(
                &msg,
                previous_turn.is_some().then_some((&r.mode, &r.settings)),
                operation,
                self.home_room.as_str(),
                &caretaker_summons,
            )
        } else if previous_turn.is_some() {
            self.store
                .insert_message_with_room(&msg, &r.mode, &r.settings, operation)
        } else {
            self.store.insert_message(&msg, operation)
        };
        if persisted.is_err() {
            if let (Some(old), ChatMode::RoundRobin { turn, .. }) = (previous_turn, &mut r.mode) {
                *turn = old;
            }
            // The in-tx authoritative claim aborts the message when a cited blob
            // was swept between the pre-check and the write. Distinguish that
            // (a caller-input problem → 400) from a genuine storage failure
            // (→ 503) by re-resolving: if any attachment no longer resolves, the
            // vanish race is why the write failed.
            if !attachments.is_empty()
                && attachments
                    .iter()
                    .any(|a| self.store.resolve_room_attachment(room, &a.id).is_none())
            {
                return Err(PostReject::UnknownAttachment);
            }
            return Err(PostReject::Storage);
        }
        // The cited blobs' pending→referenced flip already committed INSIDE the
        // message-insert transaction above (Store::write_attachment_refs), so
        // the message and its references are durable together — nothing to do
        // here. `attachments` was resolved up front purely to reject bad ids
        // and to set msg.attachments before persistence.
        if record_rate {
            r.post_times
                .entry(msg.sender.clone())
                .or_default()
                .push_back(now);
        }
        if let Some(id) = op_id.as_deref() {
            let key = (scope.key.clone(), id.to_string());
            r.operations.insert(key.clone(), msg.clone());
            r.operation_order.push_back(key);
            while r.operation_order.len() > HISTORY_LIMIT {
                if let Some(oldest) = r.operation_order.pop_front() {
                    r.operations.remove(&oldest);
                }
            }
        }
        if previous_turn.is_some() {
            let _ = r.tx.send(ServerFrame::Mode {
                mode: r.mode.clone(),
            });
        }
        // An accepted message is explicit progress for room-silence care.
        // Reset the durable generation in the same transaction as the message
        // (Store), then mirror and broadcast that lifecycle only after commit.
        r.care_marks.remove("silence");
        Self::resolve_condition_attention(r, room, "silence", msg.ts);
        r.last_msg_ms = msg.ts;
        r.history.push(msg.clone());
        // Hot/audit split: memory keeps the tail (context), the DB keeps
        // EVERYTHING (the room's memory — searchable history is direction 3).
        if r.history.len() > HISTORY_LIMIT {
            let overflow = r.history.len() - HISTORY_LIMIT;
            r.history.drain(0..overflow);
        }
        let _ = r.tx.send(ServerFrame::Msg {
            message: msg.clone(),
        });
        // A caretaker sits only in the private maintenance loca, but an exact
        // cross-loca call must survive the caretaker being offline. Persist a
        // one-message, privacy-bounded care envelope in the existing ACKed
        // outbox. Copying the source message into Care chat history would leak
        // room history; a best-effort broadcast used to lose the call.
        if room != self.home_room.as_str() {
            for signal in caretaker_summons {
                let attention = self
                    .store
                    .attention(&signal.attention_id)
                    .unwrap_or_else(|| Attention {
                        id: signal.attention_id.clone(),
                        // The ledger is filed under the source loca (origin),
                        // even though the envelope is delivered in the home loca.
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
                r.attentions.insert(attention.id.clone(), attention.clone());
                let _ = r.tx.send(ServerFrame::Attention { attention });
                self.care_deliveries
                    .lock_or_recover()
                    .insert(signal.id.clone(), signal.attention_id.clone());
                if let Some(home_tx) = home_tx.as_ref() {
                    let _ = home_tx.send(ServerFrame::Care { signal });
                }
            }
        }
        // Mirror the durable wake into memory and broadcast it (twin of the
        // caretaker block above). The waiter, live, gets the Care immediately;
        // offline, `pending_care` replays the same delivery id on reconnect and
        // `replayed_care_ids` dedups it. The wait row stays — only its stale
        // overdue is retired — and the reply's own generation bump resets age.
        if let Some((updated_wait, wake)) = wait_wake {
            let waiter = updated_wait.waiter.clone();
            r.waits.insert(waiter.clone(), updated_wait.clone());
            Self::resolve_wait_attentions(r, room, &waiter, now);
            let attention = self
                .store
                .attention(&wake.attention_id)
                .unwrap_or_else(|| Attention {
                    id: wake.attention_id.clone(),
                    room: wake.room.clone(),
                    reason: wake.reason,
                    subject: wake.subject.clone(),
                    audience: wake.audience.clone(),
                    owner: wake.owner.clone(),
                    group: wake.group.clone(),
                    participants: wake.participants.clone(),
                    created_by: wake.created_by.clone(),
                    created_at: wake.at,
                    attempt: wake.attempt,
                    escalated: wake.escalated,
                    status: AttentionStatus::Open,
                    delivered_at: None,
                    claimed_by: None,
                    claimed_at: None,
                    resolved_at: None,
                });
            r.attentions.insert(attention.id.clone(), attention.clone());
            self.care_deliveries
                .lock_or_recover()
                .insert(wake.id.clone(), wake.attention_id.clone());
            let _ = r.tx.send(ServerFrame::Care { signal: wake });
            let _ = r.tx.send(ServerFrame::Attention { attention });
            let _ = r.tx.send(ServerFrame::Wait {
                waiter,
                wait: Some(updated_wait),
            });
        }
        Ok(msg)
    }

    pub fn reactions(&self, room: &str) -> Vec<protocol::MessageReaction> {
        self.store.message_reactions(room).unwrap_or_default()
    }

    pub fn set_reaction(
        &self,
        room: &str,
        message_id: u64,
        principal: &str,
        reactor: &str,
        emoji: &str,
        active: bool,
    ) -> Result<protocol::MessageReactionEvent, ReactionReject> {
        const ALLOWED: [&str; 4] = ["✓", "✦", "!", "♥"];
        if !ALLOWED.contains(&emoji) {
            return Err(ReactionReject::InvalidEmoji);
        }
        if !self.is_writable(room) {
            return Err(ReactionReject::ReadOnly);
        }
        let owner = self
            .store
            .message_owner(room, message_id)
            .map_err(|_| ReactionReject::Storage)?
            .or_else(|| {
                self.rooms
                    .lock_or_recover()
                    .get(room)
                    .and_then(|r| r.history.iter().find(|m| m.id == message_id))
                    .map(|m| m.sender.clone())
            })
            .ok_or(ReactionReject::NotFound)?;
        if owner == reactor {
            return Err(ReactionReject::OwnMessage);
        }
        let at = (self.now_ms)();
        let reactions = self
            .store
            .set_message_reaction(room, message_id, principal, reactor, emoji, active, at)
            .map_err(|_| ReactionReject::Storage)?;
        let actors = reactions
            .into_iter()
            .find(|r| r.message_id == message_id && r.emoji == emoji)
            .map(|r| r.actors)
            .unwrap_or_else(|| {
                if active {
                    vec![reactor.to_string()]
                } else {
                    Vec::new()
                }
            });
        let event = protocol::MessageReactionEvent {
            message_id,
            emoji: emoji.to_string(),
            actors,
            owner,
            reactor: reactor.to_string(),
            active,
            ts: at,
        };
        if let Some(r) = self.rooms.lock_or_recover().get(room) {
            let _ = r.tx.send(ServerFrame::Reaction {
                reaction: event.clone(),
            });
        }
        Ok(event)
    }
}
