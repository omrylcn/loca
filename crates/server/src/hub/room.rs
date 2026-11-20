//! A loca's live state: joining and leaving, its mode, settings, Lead and typing.

use super::*;

impl Hub {
    /// Check room existence without implicitly provisioning it.
    pub fn room_exists(&self, room: &str) -> bool {
        self.rooms.lock_or_recover().contains_key(room)
    }

    /// Internal sandbox/state helper; external closed-Building entry is gated.
    pub(in crate::hub) fn room_mut<'a>(
        rooms: &'a mut HashMap<String, Room>,
        name: &str,
        defaults: &RoomSettings,
    ) -> &'a mut Room {
        rooms
            .entry(name.to_string())
            .or_insert_with(|| Room::with(ChatMode::Free, defaults.clone(), 1))
    }

    /// A subscription to a room's live frames, creating the room if needed.
    /// Returns the receiver plus the current history to replay to the joiner.
    pub fn subscribe(&self, room: &str) -> (broadcast::Receiver<ServerFrame>, Vec<Message>) {
        // Subscribing to a deleted loca must NOT bring it back. A watch=1
        // listener touches subscribe before join, so this was the main way a
        // removed room reappeared. Give it a throwaway channel and empty
        // history instead of resurrecting the room.
        if self.is_deleted(room) {
            let (tx, rx) = broadcast::channel(1);
            drop(tx);
            return (rx, Vec::new());
        }
        let mut rooms = self.rooms.lock_or_recover();
        let r = Self::room_mut(&mut rooms, room, &self.default_settings);
        (r.tx.subscribe(), r.history.clone())
    }

    /// Seats per loca. Every person — human or agent — takes one; watchers
    /// take none.
    pub fn loca_capacity(&self) -> usize {
        self.loca_capacity
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Configure seats per loca, clamped to the bounds above. Seats already
    /// taken are never revoked by lowering it; only new seats are refused.
    pub fn set_loca_capacity(&self, seats: usize) -> usize {
        let seats = seats.clamp(Self::LOCA_CAPACITY_MIN, Self::LOCA_CAPACITY_MAX);
        self.loca_capacity
            .store(seats, std::sync::atomic::Ordering::Relaxed);
        seats
    }

    /// Notifications one sender may send in
    /// [`notifications::NOTIFICATION_SEND_WINDOW_MS`].
    pub fn notification_send_limit(&self) -> u32 {
        self.notification_send_limit
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Configure the sending limit's count (at least 1; the window is
    /// fixed). Returns the value in force.
    pub fn set_notification_send_limit(&self, max: u32) -> u32 {
        let max = max.max(1);
        self.notification_send_limit
            .store(max, std::sync::atomic::Ordering::Relaxed);
        max
    }

    /// Seat a member. Returns false when the loca is full — the caller must
    /// refuse the connection.
    ///
    /// One key = one seat: `identity` is the seat key (derived from the
    /// credential at the door), `name` is only the label the seat wears. The
    /// same identity reconnecting still occupies its old seat; only a NEW
    /// identity can overflow the loca. Agent reconnects and identity renames
    /// replace the previous holder. Same-name human readers coexist. This is
    /// what makes two operator screens ONE person instead of two seats.
    #[must_use]
    pub fn join(
        &self,
        room: &str,
        identity: &str,
        name: &str,
        kind: SenderType,
        session: u64,
    ) -> bool {
        // A join to a deleted loca does not resurrect it — the master must open
        // it again first (which clears the tombstone). Refuse the seat.
        if self.is_deleted(room) {
            return false;
        }
        let mut rooms = self.rooms.lock_or_recover();
        let r = Self::room_mut(&mut rooms, room, &self.default_settings);
        // Capacity counts identities, not names: an identity rejoining under
        // any name evicts itself, never overflows.
        // The Lobby room holds the whole Building: it has no seat limit.
        let seats = if room == LOBBY_ROOM {
            usize::MAX
        } else {
            identity::seats_of(&r.settings, self.seat_numbers())
        };
        if !r.members.contains_key(identity) && r.members.len() >= seats {
            return false;
        }
        let additional_human_reader =
            r.members
                .get(identity)
                .is_some_and(|(existing_name, existing_kind, _)| {
                    existing_name == name
                        && *existing_kind == SenderType::User
                        && kind == SenderType::User
                });
        if additional_human_reader {
            // A laptop and a phone are two readers of the same human seat.
            // Keep both sockets alive; leave() removes the roster entry only
            // after the final reader disconnects.
            if let Some(entry) = r.members.get_mut(identity) {
                entry.2.insert(session);
            }
        } else if r.members.contains_key(identity) {
            // Tell older connections on this identity to close; `session` is
            // the new agent/renamed holder, so it recognises the frame as its
            // own and stays. Other human readers never take this path.
            // The frame carries the identity because the old connection may be
            // sitting under a DIFFERENT display name — name alone would miss it.
            let _ = r.tx.send(ServerFrame::Evicted {
                name: name.to_string(),
                identity: identity.to_string(),
                session,
            });
            // The seat's connections are now exactly this one: the evicted
            // session is gone whether or not its reader ever runs leave(). A
            // dead reader never reaches leave(), so keeping it here would
            // stick the seat in the roster forever (the ghost the operator
            // kept seeing); and an evicted reader that does run leave(), late,
            // is no longer listed, so it cannot free this connection's seat.
            if let Some(entry) = r.members.get_mut(identity) {
                entry.0 = name.to_string();
                entry.1 = kind;
                entry.2 = std::collections::HashSet::from([session]);
            }
        } else {
            r.members.insert(
                identity.to_string(),
                (
                    name.to_string(),
                    kind,
                    std::collections::HashSet::from([session]),
                ),
            );
        }
        let members = r.member_list(&self.profile_cards());
        let _ = r.tx.send(ServerFrame::Members { members });
        true
    }

    /// Deregister one connection (`session`, as given to `join`) on
    /// disconnect; frees the seat when none of its connections is left. A
    /// connection a newer one evicted is no longer listed, so its leave, in
    /// whatever order it comes, does not touch the seat.
    pub fn leave(&self, room: &str, identity: &str, session: u64) {
        let mut rooms = self.rooms.lock_or_recover();
        let Some(r) = rooms.get_mut(room) else { return };
        if let Some(entry) = r.members.get_mut(identity) {
            entry.2.remove(&session);
            if entry.2.is_empty() {
                r.members.remove(identity);
                r.seat_principals.remove(identity);
            }
        }
        let members = r.member_list(&self.profile_cards());
        let _ = r.tx.send(ServerFrame::Members { members });
    }

    /// Name (or clear) this loca's lead — an explicit operator action, not a
    /// chat side effect. Mutates settings, persists, broadcasts the new
    /// settings, AND emits an announcement so everyone in the loca learns of it
    /// "in the same breath" (PRINCIPLES: the action is explicit; the room sees
    /// its result). Naming targets the announcement at the new lead: ordinary
    /// clients still see the public announcement, while a mentions-only agent
    /// receives an immediate direct wake. Ending the title remains an untargeted
    /// room announcement. Returns the settings after the change.
    pub fn set_lead(
        &self,
        room: &str,
        lead: Option<String>,
        by: &str,
    ) -> Result<RoomSettings, LeadError> {
        if self.is_deleted(room) {
            return Ok(self.settings(room));
        }
        // The Lead's task authority follows this principal, not the name.
        let lead_principal = lead.as_deref().and_then(|name| self.principal_named(name));
        let mut rooms = self.rooms.lock_or_recover();
        let (mode, mut settings) = rooms
            .get(room)
            .map(|r| (r.mode.clone(), r.settings.clone()))
            .unwrap_or_else(|| (ChatMode::Free, self.default_settings.clone()));
        if lead.is_none()
            && rooms.get(room).is_some_and(|room| {
                room.goals
                    .values()
                    .any(|goal| goal.status == GoalStatus::Active)
            })
        {
            return Err(LeadError::ActiveGoal);
        }
        if lead.is_none() && settings.extension_enabled("board") {
            return Err(LeadError::BoardOn);
        }
        settings.lead = lead.clone();
        settings.lead_principal = lead_principal;
        let text = match &lead {
            Some(name) => format!("{by} set @lead {name}."),
            None => format!("{by} set @lead none."),
        };
        let msg = Message {
            attachments: Vec::new(),
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            room: room.to_string(),
            sender: "loca".into(),
            sender_type: SenderType::User,
            // Announcements do not normally wake filtered agents. A lead
            // assignment is also a direct summons to the person who must now
            // watch the whole room, so address that one announcement to them.
            target: lead.clone().or_else(|| Some("all".into())),
            text,
            reply_to: None,
            reply_to_sender: None,
            sender_principal: None,
            kind: protocol::MessageKind::Announce,
            ts: (self.now_ms)(),
        };
        self.store
            .insert_message_with_room(&msg, &mode, &settings, None)
            .map_err(|_| LeadError::Storage)?;
        let r = Self::room_mut(&mut rooms, room, &self.default_settings);
        r.settings = settings.clone();
        r.history.push(msg.clone());
        let settings = self.shown_settings(settings);
        let _ = r.tx.send(ServerFrame::Settings {
            settings: settings.clone(),
        });
        let _ = r.tx.send(ServerFrame::Msg { message: msg });
        Ok(settings)
    }

    /// Set the chat mode (admin action) and broadcast it. Normalizes an
    /// out-of-range round-robin `turn` back to 0.
    pub fn set_mode(&self, room: &str, mut mode: ChatMode) -> rusqlite::Result<ChatMode> {
        // A sealed loca is not reopened by a mode change — only a deliberate
        // reopen clears the tombstone (see `post`/`join`/`subscribe`).
        if self.is_deleted(room) {
            return Ok(self.mode(room));
        }
        if let ChatMode::RoundRobin { order, turn } = &mut mode {
            if order.is_empty() || *turn >= order.len() {
                *turn = 0;
            }
        }
        let mut rooms = self.rooms.lock_or_recover();
        let r = Self::room_mut(&mut rooms, room, &self.default_settings);
        self.store.save_room(room, &mode, &r.settings)?;
        r.mode = mode.clone();
        let _ = r.tx.send(ServerFrame::Mode { mode: mode.clone() });
        Ok(mode)
    }

    pub fn mode(&self, room: &str) -> ChatMode {
        let rooms = self.rooms.lock_or_recover();
        rooms.get(room).map(|r| r.mode.clone()).unwrap_or_default()
    }

    /// Whether the room is in "live" (WhatsApp) mode right now.
    pub fn is_live(&self, room: &str) -> bool {
        let rooms = self.rooms.lock_or_recover();
        rooms.get(room).map(|r| r.settings.live).unwrap_or(false)
    }

    /// Whether `name` currently wears this loca's lead title.
    ///
    /// A mentions-filtered lead still receives the whole room: seeing the
    /// conversation is the role, not a temporary live-mode side effect.
    pub fn is_lead(&self, room: &str, name: &str) -> bool {
        let rooms = self.rooms.lock_or_recover();
        rooms.get(room).and_then(|r| r.settings.lead.as_deref()) == Some(name)
    }

    /// Current admin-tunable settings for a room.
    /// A loca's own number of seats reads as the number in force (see
    /// `identity::shown`).
    pub fn settings(&self, room: &str) -> RoomSettings {
        let settings = {
            let rooms = self.rooms.lock_or_recover();
            rooms
                .get(room)
                .map(|r| r.settings.clone())
                .unwrap_or_else(|| self.default_settings.clone())
        };
        self.shown_settings(settings)
    }

    /// Update settings (admin action), broadcast, and persist.
    #[allow(clippy::too_many_arguments)]
    pub fn set_settings(
        &self,
        room: &str,
        rate_limit: Option<u32>,
        window: Option<u32>,
        live: Option<bool>,
        archived: Option<bool>,
        live_timeout: Option<u32>,
        operators: Option<Vec<String>>,
        turn_max_messages: Option<u32>,
        turn_idle_ms: Option<u32>,
        turn_max_wait_ms: Option<u32>,
        care_wait_secs: Option<u32>,
        care_cooldown_secs: Option<u32>,
        care_max_attempts: Option<u32>,
        care_context_messages: Option<u32>,
        care_recipient: Option<ReminderRecipient>,
        care_goal_secs: Option<u32>,
        care_task_secs: Option<u32>,
        care_silence_secs: Option<u32>,
    ) -> rusqlite::Result<RoomSettings> {
        // A sealed loca does not come back on a settings change.
        if self.is_deleted(room) {
            return Err(rusqlite::Error::InvalidParameterName(
                "room is permanently sealed".into(),
            ));
        }
        let mut rooms = self.rooms.lock_or_recover();
        let r = Self::room_mut(&mut rooms, room, &self.default_settings);
        let mut s = r.settings.clone();
        let was_archived = r.settings.archived;
        if let Some(rl) = rate_limit {
            s.rate_limit = rl;
        }
        if let Some(w) = window {
            s.rate_window_secs = w.max(1);
        }
        if let Some(l) = live {
            s.live = l;
        }
        if let Some(a) = archived {
            s.archived = a;
        }
        if let Some(t) = live_timeout {
            s.live_timeout_secs = t;
        }
        if let Some(ops) = operators {
            s.operators = ops;
        }
        if let Some(max) = turn_max_messages {
            s.turn_max_messages = max.clamp(1, 16);
        }
        if let Some(idle) = turn_idle_ms {
            s.turn_idle_ms = idle.clamp(100, 30_000);
        }
        if let Some(max_wait) = turn_max_wait_ms {
            s.turn_max_wait_ms = max_wait.clamp(100, 60_000);
        }
        // A hard maximum below the quiet window would make the advertised
        // quiet-window contract impossible. Keep the pair internally valid.
        s.turn_max_wait_ms = s.turn_max_wait_ms.max(s.turn_idle_ms);
        if let Some(value) = care_wait_secs {
            s.care_wait_secs = value;
        }
        if let Some(value) = care_cooldown_secs {
            s.care_cooldown_secs = value;
        }
        if let Some(value) = care_max_attempts {
            s.care_max_attempts = value.clamp(1, 10);
        }
        if let Some(value) = care_context_messages {
            s.care_context_messages = value.clamp(0, 20);
        }
        if let Some(value) = care_recipient {
            s.care_recipient = value;
        }
        if let Some(value) = care_goal_secs {
            s.care_goal_secs = value;
        }
        if let Some(value) = care_task_secs {
            s.care_task_secs = value;
        }
        if let Some(value) = care_silence_secs {
            s.care_silence_secs = value;
        }
        self.store.save_room(room, &r.mode, &s)?;
        if !was_archived && s.archived {
            r.care_marks.clear();
        }
        if live == Some(true) {
            r.last_msg_ms = (self.now_ms)();
        }
        r.settings = s.clone();
        let s = self.shown_settings(s);
        let _ = r.tx.send(ServerFrame::Settings {
            settings: s.clone(),
        });
        Ok(s)
    }

    /// Broadcast a control frame (e.g. operator `/stop`) to a room.
    pub fn control(&self, room: &str, cmd: &str) {
        let rooms = self.rooms.lock_or_recover();
        if let Some(r) = rooms.get(room) {
            let _ = r.tx.send(ServerFrame::Control {
                cmd: cmd.to_string(),
            });
        }
    }

    /// Rebroadcast an ephemeral typing signal (not persisted).
    pub fn typing(&self, room: &str, name: &str, on: bool) {
        let rooms = self.rooms.lock_or_recover();
        if let Some(r) = rooms.get(room) {
            let _ = r.tx.send(ServerFrame::Typing {
                name: name.to_string(),
                on,
            });
        }
    }

    // ---- per-participant moderation (admin-only, enforced at post/join) ----
}
