//! The Lobby room, the reserved home loca, and who waits in the Lobby.

use super::*;

impl Hub {
    /// Is `token` still the live invite for `room` of the membership
    /// `member_id`? The Lobby forwards a `called` only while it is, so an
    /// invite that ended between its delivery on the bus and the moment a
    /// Lobby socket forwards it (a call, then a revoke or re-issue of the
    /// same seat) never reaches the client after the invite that replaced it.
    pub fn call_is_current(&self, member_id: &str, room: &str, token: &str) -> bool {
        self.invites
            .lock_or_recover()
            .get(&secret_hash(token))
            .is_some_and(|invite| invite.member_id == member_id && invite.room == room)
    }

    /// Subscribe before taking the seat list and the pending deliveries, so a
    /// call racing the handshake is observed either there or on the bus
    /// (possibly both; clients de-duplicate by loca/token).
    pub fn subscribe_lobby(&self) -> broadcast::Receiver<LobbyEvent> {
        self.lobby_tx.subscribe()
    }

    pub fn home_room(&self) -> &str {
        &self.home_room
    }

    pub fn is_reserved_room(&self, room: &str) -> bool {
        !self.reserved_room.is_empty() && room == self.reserved_room.as_str()
    }

    /// The Lobby room: where everyone in the Building lands first and may
    /// talk. Every Building member reaches it by membership alone; it takes
    /// no invitations, has no seat limit, carries no extensions, and is never
    /// deleted, closed or moved into a group (PRINCIPLES "Lobby").
    pub fn is_lobby_room(&self, room: &str) -> bool {
        room == LOBBY_ROOM
    }

    /// Who may enter the Lobby room: the ranks, and any Building member
    /// signed in with a Building-level credential (their own key, profile
    /// credential or the building key). A session taken with a loca invite
    /// stays in its loca; a banned name stays out.
    pub(in crate::hub) fn lobby_door(
        &self,
        credential_authority: &RequestAuthority,
        admin_tok: Option<&str>,
        invite_tok: Option<&str>,
        session: Option<&SessionIdentity>,
        name_hint: Option<&str>,
    ) -> EnterDecision {
        if credential_authority.is_building_admin() && admin_tok.is_some() {
            return EnterDecision::Allowed;
        }
        if session.is_some_and(|s| s.admin) {
            return EnterDecision::Allowed;
        }
        if let Some(name) = self.door_name(invite_tok, session, credential_authority, name_hint) {
            if self.is_banned(LOBBY_ROOM, &name) {
                return EnterDecision::Banned;
            }
            // A caretaker tends loca itself and is not a member of the group:
            // the Lobby is not its room (PRINCIPLES "Lobby").
            if self.is_caretaker(&name) {
                return EnterDecision::Denied;
            }
        }
        let building_session = session.is_some_and(|s| s.loca.is_none());
        if building_session {
            return EnterDecision::Allowed;
        }
        // A member's own key presented as the room credential (an agent's
        // listener and REST reads do this): a key, not an invite.
        if session.is_none()
            && invite_tok.is_some_and(|token| {
                !self.store.credential_is_revoked(token)
                    && self.member_for_credential(Some(token)).is_some()
            })
        {
            return EnterDecision::Allowed;
        }
        EnterDecision::Denied
    }

    /// Who calls everyone in the Lobby room. The Lobby holds the whole
    /// Building, so a member's `@all` there would wake every agent of the
    /// Building: only a rank (`rank`: the post carries Building-admin
    /// authority) calls everyone there. A rank's call is stored as `target:
    /// all`, so every client and a reconnecting agent's REST backfill see it;
    /// anyone else's `target: all` is dropped and their `@all` stays words.
    /// The mentions filter counts only `target: all` in the Lobby
    /// (`msg_addresses`). Every other room is left as it is.
    pub fn settle_lobby_call(&self, room: &str, body: &mut PostMessage, rank: bool) {
        if !self.is_lobby_room(room) {
            return;
        }
        let calls_all = body.target.as_deref() == Some("all") || text_calls_all(&body.text);
        if !calls_all {
            return;
        }
        if rank {
            if body.target.is_none() {
                body.target = Some("all".to_string());
            }
        } else if body.target.as_deref() == Some("all") {
            body.target = None;
        }
    }

    pub fn is_caretaker(&self, name: &str) -> bool {
        self.caretakers.contains(name)
    }

    pub fn caretaker_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.caretakers.iter().cloned().collect();
        names.sort();
        names
    }

    /// Count a Lobby socket of the membership `member_id` (its `mbid_`).
    pub fn lobby_join(&self, member_id: &str) {
        let mut online = self.lobby_online.lock_or_recover();
        *online.entry(member_id.to_string()).or_insert(0) += 1;
    }

    pub fn lobby_leave(&self, member_id: &str) {
        let mut online = self.lobby_online.lock_or_recover();
        if let Some(count) = online.get_mut(member_id) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                online.remove(member_id);
            }
        }
    }

    pub(in crate::hub) fn lobby_is_online(&self, member_id: &str) -> bool {
        self.lobby_online
            .lock_or_recover()
            .get(member_id)
            .copied()
            .unwrap_or(0)
            > 0
    }
}
