//! Moderation and the loca Operator: bans, mutes, who may moderate whom.

use super::{
    invite_issuer_rank, map_loca_operator_error, BuildingRole, Hub, InviteRecord,
    LocaOperatorAssignment, ModerationError, ModerationRanks, OperatorAssignmentError,
    RecoverMutex, RequestAuthority, ServerFrame,
};

impl Hub {
    /// Whether `name` is banned from `room`.
    pub fn is_banned(&self, room: &str, name: &str) -> bool {
        let rooms = self.rooms.lock_or_recover();
        rooms
            .get(room)
            .map(|r| r.banned.contains(name))
            .unwrap_or(false)
    }

    pub fn mod_state(&self, room: &str) -> protocol::ModState {
        let rooms = self.rooms.lock_or_recover();
        match rooms.get(room) {
            Some(r) => {
                let mut muted: Vec<String> = r.muted.iter().cloned().collect();
                let mut banned: Vec<String> = r.banned.iter().cloned().collect();
                muted.sort();
                banned.sort();
                protocol::ModState { muted, banned }
            }
            None => protocol::ModState::default(),
        }
    }

    /// Every loca's ban list in one read, for the Building view. The caller
    /// filters by its own door: decide reach after this returns, never while
    /// the rooms lock is held (`enter_decision` takes it too).
    pub fn bans_by_room(&self) -> Vec<(String, Vec<String>)> {
        let deleted = self.deleted.lock_or_recover().clone();
        let rooms = self.rooms.lock_or_recover();
        let mut out: Vec<(String, Vec<String>)> = rooms
            .iter()
            .filter(|(room, r)| !r.banned.is_empty() && !deleted.contains(*room))
            .map(|(room, r)| {
                let mut banned: Vec<String> = r.banned.iter().cloned().collect();
                banned.sort();
                (room.clone(), banned)
            })
            .collect();
        out.sort();
        out
    }

    /// Apply a moderation action. Returns the updated mod state. For kick/ban a
    /// `Kicked` frame is broadcast so the targeted client closes its socket.
    pub fn moderate(
        &self,
        room: &str,
        action: protocol::ModAction,
        name: &str,
        actor: &RequestAuthority,
    ) -> Result<protocol::ModState, ModerationError> {
        use protocol::ModAction::*;
        // Admission and moderation cannot race a lower-rank removal of a
        // newly issued superior invite. Checks are repeated under both locks.
        let _lifecycle = self.member_lifecycle.lock_or_recover();
        let mut ranks = self.moderation_ranks.lock_or_recover();
        if !self.may_moderate_with_ranks(room, actor, action, name, &ranks) {
            return Err(ModerationError::Forbidden);
        }
        // A sealed loca is not reopened by a moderation action.
        if self.is_deleted(room) {
            return Ok(self.mod_state(room));
        }
        match action {
            Mute => self.store.set_ban(
                room,
                name,
                "mute",
                (self.now_ms)(),
                actor.moderation_rank(),
                actor.principal_id.as_deref(),
            )?,
            Unmute => self.store.clear_ban(room, name, "mute")?,
            Ban => {
                self.store.set_ban(
                    room,
                    name,
                    "ban",
                    (self.now_ms)(),
                    actor.moderation_rank(),
                    actor.principal_id.as_deref(),
                )?;
                // set_ban persisted the restriction and invite cut-off in one
                // transaction; nothing fallible remains before the mirrors.
                // The connection is closed below by the ban's own `Kicked`.
                self.end_invites_cache_for(room, name);
            }
            Unban => self.store.clear_ban(room, name, "ban")?,
            Kick | Release => self.revoke_invites_for(room, name)?,
        }
        let restriction = match action {
            Mute | Unmute => Some("mute"),
            Ban | Unban => Some("ban"),
            _ => None,
        };
        if let Some(kind) = restriction {
            let key = (room.to_string(), name.to_string(), kind.to_string());
            if matches!(action, Mute | Ban) {
                ranks.insert(key, actor.moderation_rank());
            } else {
                ranks.remove(&key);
            }
        }
        let mut rooms = self.rooms.lock_or_recover();
        let r = Self::room_mut(&mut rooms, room, &self.default_settings);
        match action {
            Mute => {
                r.muted.insert(name.to_string());
            }
            Unmute => {
                r.muted.remove(name);
            }
            Kick => {
                // PRINCIPLES: "kick — connection closes and invitation stops". Kick is
                // not just a disconnect: it ends the invite, otherwise the same
                // token walks right back in. Take the seat AND the invite.
                let _ = r.tx.send(ServerFrame::Kicked {
                    name: name.to_string(),
                    banned: false,
                });
                // Moderation targets a NAME (that is what the operator sees);
                // the seat map is identity-keyed, so drop whichever seat wears
                // that label.
                r.members.retain(|_id, (n, _, _)| n != name);
                let members = r.member_list(&self.profile_cards());
                let _ = r.tx.send(ServerFrame::Members { members });
                drop(rooms);
                return Ok(self.mod_state(room));
            }
            Ban => {
                r.banned.insert(name.to_string());
                r.members.retain(|_id, (n, _, _)| n != name); // ghost or live, they are out
                let _ = r.tx.send(ServerFrame::Kicked {
                    name: name.to_string(),
                    banned: true,
                });
                // Broadcast the new roster, exactly as Kick does. Without this
                // the banned name lingered in every client's list until the
                // next unrelated join/leave — the asymmetry the operator hit.
                let members = r.member_list(&self.profile_cards());
                let _ = r.tx.send(ServerFrame::Members { members });
                // The door shuts, reading included — persist so a restart does
                // not quietly reopen it (PRINCIPLES: "a restart does not kill the room").
                drop(rooms);
                return Ok(self.mod_state(room));
            }
            Unban => {
                r.banned.remove(name);
            }
            Release => {
                // Take back the seat, not the belonging. Their invites for THIS
                // loca end and the connection closes; membership of the
                // building is untouched, so the next call-in is one click.
                // Drop the roster entry directly (Kick/Ban do) so a dead socket
                // leaves no ghost — the whole point of "the work is done."
                let _ = r.tx.send(ServerFrame::Kicked {
                    name: name.to_string(),
                    banned: false,
                });
                r.members.retain(|_id, (n, _, _)| n != name);
                let members = r.member_list(&self.profile_cards());
                let _ = r.tx.send(ServerFrame::Members { members });
                drop(rooms);
                return Ok(self.mod_state(room));
            }
        }
        let mut muted: Vec<String> = r.muted.iter().cloned().collect();
        let mut banned: Vec<String> = r.banned.iter().cloned().collect();
        muted.sort();
        banned.sort();
        let state = protocol::ModState { muted, banned };
        let _ = r.tx.send(ServerFrame::Mod {
            state: state.clone(),
        });
        Ok(state)
    }

    /// May `actor` apply `action` to the participant labelled `target`?
    ///
    /// PRINCIPLES: the upper layer covers the lower one and the lower one
    /// cannot undo the upper one's decision. Loca moderation is for the
    /// table's participants, not for Building ranks: nobody mutes, kicks or
    /// bans the Master, and only the Master does so to a Smaster. Restriction
    /// provenance and invite issuer rank also constrain the action.
    pub fn may_moderate(
        &self,
        room: &str,
        actor: &RequestAuthority,
        action: protocol::ModAction,
        target: &str,
    ) -> bool {
        self.may_moderate_with_ranks(
            room,
            actor,
            action,
            target,
            &self.moderation_ranks.lock_or_recover(),
        )
    }

    pub fn may_end_invite(&self, actor: &RequestAuthority, invite: &InviteRecord) -> bool {
        actor.moderation_rank() >= invite_issuer_rank(&invite.issued_by)
    }

    pub(in crate::hub) fn may_moderate_with_ranks(
        &self,
        room: &str,
        actor: &RequestAuthority,
        action: protocol::ModAction,
        target: &str,
        ranks: &ModerationRanks,
    ) -> bool {
        use protocol::ModAction::*;
        let kind = match action {
            Mute | Unmute => Some("mute"),
            Ban | Unban => Some("ban"),
            _ => None,
        };
        if kind
            .and_then(|kind| ranks.get(&(room.to_string(), target.to_string(), kind.into())))
            .is_some_and(|rank| *rank > actor.moderation_rank())
        {
            return false;
        }
        if matches!(action, Kick | Ban | Release)
            && self.invites.lock_or_recover().values().any(|invite| {
                invite.room == room && invite.name == target && !self.may_end_invite(actor, invite)
            })
        {
            return false;
        }
        if matches!(action, Unmute | Unban) {
            return true;
        }
        match self.building_rank_of(room, target) {
            Some(BuildingRole::Master) => false,
            Some(BuildingRole::Smaster) => actor.is_master(),
            _ => true,
        }
    }

    /// The Building rank wearing the label `name`, as far as the server can
    /// prove it: a persisted profile first, then the live Smaster registry,
    /// then the seat key of whoever sits in `room` under that label (which
    /// also covers memory-only mode, where no profiles are persisted).
    pub(in crate::hub) fn building_rank_of(&self, room: &str, name: &str) -> Option<BuildingRole> {
        if let Some(profile) = self
            .profiles()
            .into_iter()
            .find(|profile| profile.display_name == name)
        {
            return Some(profile.role);
        }
        if self
            .smasters
            .lock_or_recover()
            .values()
            .any(|record| record.name == name)
        {
            return Some(BuildingRole::Smaster);
        }
        let rooms = self.rooms.lock_or_recover();
        let seat = rooms.get(room).and_then(|r| {
            r.members
                .iter()
                .find(|(_, (label, _, _))| label == name)
                .map(|(identity, _)| identity.clone())
        })?;
        if seat == "@master" {
            Some(BuildingRole::Master)
        } else if seat.starts_with("sm:") {
            Some(BuildingRole::Smaster)
        } else {
            None
        }
    }

    pub fn appoint_loca_operator(
        &self,
        room: &str,
        target_principal_id: &str,
        actor: &RequestAuthority,
    ) -> Result<LocaOperatorAssignment, OperatorAssignmentError> {
        if !actor.is_building_admin() {
            return Err(OperatorAssignmentError::AuthorityRequired);
        }
        let actor_id = actor
            .principal_id
            .as_deref()
            .ok_or(OperatorAssignmentError::AuthorityRequired)?;
        // Held across the write so two appointments cannot leave the mirror
        // naming the one the database replaced.
        let mut operators = self.operators.lock_or_recover();
        let assignment = self
            .store
            .appoint_loca_operator(room, target_principal_id, actor_id, self.now())
            .map_err(map_loca_operator_error)?;
        operators.insert(room.to_string(), assignment.principal_id.clone());
        Ok(assignment)
    }

    pub fn revoke_loca_operator(
        &self,
        room: &str,
        actor: &RequestAuthority,
    ) -> Result<LocaOperatorAssignment, OperatorAssignmentError> {
        if !actor.is_building_admin() {
            return Err(OperatorAssignmentError::AuthorityRequired);
        }
        let actor_id = actor
            .principal_id
            .as_deref()
            .ok_or(OperatorAssignmentError::AuthorityRequired)?;
        let current = self
            .store
            .loca_operator(room)
            .ok_or(OperatorAssignmentError::NotFound)?;
        if actor.building_role == Some(BuildingRole::Smaster)
            && (current.appointed_by_role != BuildingRole::Smaster
                || current.appointed_by_principal_id != actor_id)
        {
            return Err(OperatorAssignmentError::MasterProtected);
        }
        let mut operators = self.operators.lock_or_recover();
        let revoked = self
            .store
            .revoke_loca_operator(room, &current.principal_id, self.now())
            .map_err(map_loca_operator_error)?;
        operators.remove(room);
        Ok(revoked)
    }

    /// End an appointment because a rule requires it (the operator left the
    /// loca's group), not by anyone's choice: no actor's appointment rights
    /// are involved. The record keeps the revocation.
    pub(crate) fn end_loca_operator_by_rule(
        &self,
        room: &str,
    ) -> Result<(), OperatorAssignmentError> {
        let Some(current) = self.store.loca_operator(room) else {
            return Ok(());
        };
        let mut operators = self.operators.lock_or_recover();
        self.store
            .revoke_loca_operator(room, &current.principal_id, self.now())
            .map_err(map_loca_operator_error)?;
        operators.remove(room);
        Ok(())
    }

    /// Operator authority in one loca: Building authority everywhere, or the
    /// principal explicitly appointed to this loca. In-memory/dev stores keep
    /// the old name list for compatibility; persistent stores migrate it at
    /// boot and never grant authority from a display label.
    pub fn is_loca_operator(
        &self,
        room: &str,
        admin_token: Option<&str>,
        session_token: Option<&str>,
        name: &str,
    ) -> bool {
        let authority = self.resolve_authority(admin_token, session_token);
        if authority.is_building_admin() {
            return true;
        }
        if self.admin_token.is_empty() {
            return true; // dev-open, consistent with the other admin gates
        }
        // The Lobby has no Operator seat: only a rank moderates it. An
        // appointment recorded under the id `lobby` by a loca of that name
        // before the Lobby existed must not reach the whole Building.
        if self.is_lobby_room(room) {
            return false;
        }
        let scoped = self
            .session_identity(session_token)
            .filter(|s| s.loca.as_deref() == Some(room));
        let principal = scoped
            .as_ref()
            .and_then(|s| s.principal_id.as_deref())
            .or(authority.principal_id.as_deref());
        if let Some(principal_id) = principal {
            let current = self.operators.lock_or_recover().get(room).cloned();
            if let Some(current) = current {
                return current == principal_id;
            }
        }
        if self.store.is_persistent() {
            return false;
        }
        let rooms = self.rooms.lock_or_recover();
        rooms
            .get(room)
            .map(|r| r.settings.operators.iter().any(|o| o == name))
            .unwrap_or(false)
    }

    // ---- tasks: declared work (the guest object) ----
}
