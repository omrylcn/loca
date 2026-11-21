//! Opening, archiving and deleting locas.

use super::{
    extensions, identity, loca_name, map_loca_operator_error, master_agent::MasterAgentError,
    ChatMode, CreateLocaError, DeleteReject, GroupError, Hub, IdentityEdit, LocaOperatorAssignment,
    OperatorAssignmentError, RecoverMutex, RequestAuthority, Room, SenderType, ServerFrame,
};

impl Hub {
    /// Seal a room for good: tell everyone and drop it from live memory while
    /// preserving its record. Connected clients get a `room-closed` control,
    /// then their
    /// broadcast channel closes (which ends their sessions). Returns whether
    /// the room existed. The configured home loca may be closed too — it
    /// simply respawns empty on the next join.
    pub fn delete_room(&self, room: &str) -> Result<bool, DeleteReject> {
        {
            let rooms = self.rooms.lock_or_recover();
            match rooms.get(room) {
                Some(r) if !r.settings.archived => return Err(DeleteReject::NotArchived),
                None => return Ok(false),
                _ => {}
            }
        }
        self.store
            .seal_room(room, (self.now_ms)())
            .map_err(|_| DeleteReject::Storage)?;
        let removed = self.rooms.lock_or_recover().remove(room);
        if let Some(r) = removed {
            let _ = r.tx.send(ServerFrame::Control {
                cmd: "room-closed".into(),
            });
        }
        self.deleted.lock_or_recover().insert(room.to_string());
        Ok(true)
    }

    /// Is this loca deleted (tombstoned)? A subscribe/join to it must not
    /// re-create it. The configured home loca is never tombstoned.
    pub fn is_deleted(&self, room: &str) -> bool {
        room != self.home_room.as_str() && self.deleted.lock_or_recover().contains(room)
    }

    /// Is this loca archived (closed, read-only)? Archived is a deliberate
    /// state: the room is kept but nothing new may be written to it.
    pub fn is_archived(&self, room: &str) -> bool {
        self.rooms
            .lock_or_recover()
            .get(room)
            .map(|r| r.settings.archived)
            .unwrap_or(false)
    }

    /// May a domain mutation land in this loca? A sealed (deleted) or archived
    /// loca is read-only — no message, note, task, journal or moderation write.
    /// PRINCIPLES: "archived = read-only, no domain mutation." One predicate
    /// so every write path asks the same question instead of each guessing.
    pub fn is_writable(&self, room: &str) -> bool {
        !self.is_deleted(room) && !self.is_archived(room)
    }

    /// Turn live mode off in any room that has been silent past its timeout.
    /// Called periodically by a background task so a forgotten live room can't
    /// keep waking every agent.
    pub fn expire_live(&self) {
        let now = (self.now_ms)();
        let mut rooms = self.rooms.lock_or_recover();
        for (name, r) in rooms.iter_mut() {
            let t = r.settings.live_timeout_secs as u64;
            if r.settings.live
                && t > 0
                && r.last_msg_ms > 0
                && now.saturating_sub(r.last_msg_ms) > t * 1000
            {
                r.settings.live = false;
                let s = r.settings.clone();
                let _ = r.tx.send(ServerFrame::Settings {
                    settings: self.shown_settings(s.clone()),
                });
                let _ = r.tx.send(ServerFrame::Control {
                    cmd: format!("live-expired:{}", t),
                });
                if let Err(error) = self.store.save_room(name, &r.mode, &s) {
                    r.settings.live = true;
                    tracing::error!(room = %name, %error, "could not persist live expiry");
                } else {
                    tracing::info!(room = %name, "live mode auto-disabled after {}s idle", t);
                }
            }
        }
    }

    pub fn loca_operator(&self, room: &str) -> Option<LocaOperatorAssignment> {
        self.store.loca_operator(room)
    }

    /// Is `principal_id` this loca's appointed operator? Answered from the
    /// in-memory mirror, never from SQLite.
    pub fn is_operator_principal(&self, room: &str, principal_id: &str) -> bool {
        self.operators
            .lock_or_recover()
            .get(room)
            .is_some_and(|current| current == principal_id)
    }

    /// Open a loca. `extensions` is the chosen set (already normalized by the
    /// caller); `None` keeps the legacy set, so an older client that sends
    /// only a name gets the loca it always got. A chosen set is written to the
    /// new loca's Journal.
    #[cfg(test)]
    pub fn create_loca(
        &self,
        room: &str,
        actor: &RequestAuthority,
        extensions: Option<Vec<String>>,
    ) -> Result<Option<LocaOperatorAssignment>, OperatorAssignmentError> {
        self.create_loca_in(room, actor, extensions, None)
            .map_err(|error| match error {
                CreateLocaError::Operator(error) => error,
                CreateLocaError::Group(_)
                | CreateLocaError::Identity(_)
                | CreateLocaError::Name(_)
                | CreateLocaError::MasterAgent(_) => OperatorAssignmentError::AuthorityRequired,
            })
    }

    /// Open a loca, in a group when `group` is given. Only the Master or a
    /// Smaster who is a member of that group opens a group loca, and the
    /// loca is tied to its group in the same commit that creates it.
    #[cfg(test)]
    pub fn create_loca_in(
        &self,
        room: &str,
        actor: &RequestAuthority,
        extensions: Option<Vec<String>>,
        group: Option<&str>,
    ) -> Result<Option<LocaOperatorAssignment>, CreateLocaError> {
        self.create_loca_with(room, actor, extensions, group, &IdentityEdit::default())
    }

    /// Open a loca with its identity (description, seats, icon, colour,
    /// labels) set in the same write. An empty `identity` opens exactly the
    /// loca `create_loca_in` opens; a non-empty one is written to the new
    /// loca's Journal.
    pub fn create_loca_with(
        &self,
        room: &str,
        actor: &RequestAuthority,
        extensions: Option<Vec<String>>,
        group: Option<&str>,
        identity: &IdentityEdit,
    ) -> Result<Option<LocaOperatorAssignment>, CreateLocaError> {
        // A Master Agent opens a loca only on a floor the Master designated,
        // and is given no Operator seat (an explicit Operator is a human).
        let master_agent = Self::is_master_agent(actor);
        if master_agent {
            let floor = group.ok_or(MasterAgentError::FloorNotDesignated);
            floor
                .and_then(|floor| self.master_agent_may_open_in(floor, actor))
                .map_err(CreateLocaError::MasterAgent)?;
        } else if !actor.is_building_admin() {
            return Err(OperatorAssignmentError::AuthorityRequired.into());
        }
        let numbers = self.seat_numbers();
        let identity =
            identity::validated(identity, numbers.ceiling).map_err(CreateLocaError::Identity)?;
        if self.is_deleted(room) || room.starts_with(Self::DM_PREFIX) {
            return Err(OperatorAssignmentError::Conflict.into());
        }
        // Founding a loca in a group takes turns with archiving or deleting
        // that group: the check below must still hold when the loca is bound.
        let group_turn = group.map(|_| self.member_lifecycle.lock_or_recover());
        if let Some(group) = group {
            if self.is_reserved_room(room) || room == self.home_room() {
                return Err(CreateLocaError::Group(GroupError::Excluded));
            }
            if master_agent {
                // Asked again under the turn: the floor may have just closed.
                self.master_agent_may_open_in(group, actor)
                    .map_err(CreateLocaError::MasterAgent)?;
            } else {
                self.may_create_in_group(group, actor)
                    .map_err(CreateLocaError::Group)?;
            }
        }
        let mut settings = self.default_settings.clone();
        settings.extensions = extensions.clone();
        identity::apply(&mut settings, &identity);
        let opened_identity = identity::opened_line(&settings, numbers);
        let mut rooms = self.rooms.lock_or_recover();
        if rooms.contains_key(room) {
            return Err(OperatorAssignmentError::Conflict.into());
        }
        loca_name::check_new(room, rooms.keys().map(String::as_str))
            .map_err(CreateLocaError::Name)?;
        let assignment = if let (true, Some(group)) = (master_agent, group) {
            let opener = actor
                .principal_id
                .as_deref()
                .ok_or(OperatorAssignmentError::AuthorityRequired)?;
            if !self
                .store
                .create_master_agent_loca(room, &settings, opener, group, self.now())
                .map_err(|_| OperatorAssignmentError::Storage)?
            {
                return Err(OperatorAssignmentError::Conflict.into());
            }
            None
        } else if self.store.is_persistent() && !self.admin_open() {
            let founder = actor
                .principal_id
                .as_deref()
                .ok_or(OperatorAssignmentError::AuthorityRequired)?;
            Some(
                self.store
                    .create_room_with_operator(room, &settings, founder, group, self.now())
                    .map_err(map_loca_operator_error)?,
            )
        } else {
            // The deliberately open sandbox has no authenticated founder. It
            // may still use SQLite to preserve its room state across restarts.
            self.store
                .save_room(room, &ChatMode::Free, &settings)
                .map_err(|_| OperatorAssignmentError::Storage)?;
            if group.is_some() {
                self.store
                    .set_room_group(room, group, actor.principal_id.as_deref(), self.now())
                    .map_err(|_| OperatorAssignmentError::Storage)?;
            }
            None
        };
        // Tied to its group before anyone can see the room exists.
        if let Some(group) = group {
            self.bind_new_group_loca(room, group);
        }
        if let Some(assignment) = assignment.as_ref() {
            self.operators
                .lock_or_recover()
                .insert(room.to_string(), assignment.principal_id.clone());
        }
        rooms.insert(room.to_string(), Room::with(ChatMode::Free, settings, 1));
        drop(rooms);
        drop(group_turn);
        if let Some(chosen) = extensions {
            let by = actor
                .display_name
                .clone()
                .unwrap_or_else(|| "master".to_string());
            let kind = actor.kind.unwrap_or(SenderType::User);
            // The loca exists either way; a lost Journal line is logged, not
            // turned into a failed create the caller would retry into 409.
            if let Err(error) =
                self.append_journal(room, by, kind, extensions::opened_line(&chosen))
            {
                tracing::error!(room, %error, "could not journal the opening extensions");
            }
        }
        if let Some(line) = opened_identity {
            let by = actor
                .display_name
                .clone()
                .unwrap_or_else(|| "master".to_string());
            let kind = actor.kind.unwrap_or(SenderType::User);
            if let Err(error) = self.append_journal(room, by, kind, line) {
                tracing::error!(room, %error, "could not journal the opening identity");
            }
        }
        Ok(assignment)
    }

    pub fn loca_operator_history(&self, room: &str) -> Vec<LocaOperatorAssignment> {
        self.store.loca_operator_history(room)
    }
}
