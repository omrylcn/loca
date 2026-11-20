//! Invitations: issuing, reissuing, delivering and ending a loca's invites.

use super::*;

impl Hub {
    /// True if `token` is an invite for THIS loca. An invite opens one loca and no
    /// other — that is the whole point of it (DESIGN §5g).
    /// A presented invite, whichever loca it belongs to: hashed and looked up
    /// by its hash. Nothing else (the hash itself, an `ivid_`) is an invite.
    pub fn invite_by_token(&self, token: &str) -> Option<InviteRecord> {
        if token.is_empty() {
            return None;
        }
        let invite = self
            .invites
            .lock_or_recover()
            .get(&secret_hash(token))
            .cloned()?;
        if !invite.member_id.is_empty() && self.member_suspended(&invite.member_id) {
            return None;
        }
        Some(invite)
    }

    /// Is the invite whose hash is `invite_hash` still live, its member not
    /// suspended? `invite_by_token` for a caller that holds the hash.
    pub(in crate::hub) fn invite_live_by_hash(&self, invite_hash: &str) -> bool {
        let Some(invite) = self.invites.lock_or_recover().get(invite_hash).cloned() else {
            return false;
        };
        invite.member_id.is_empty() || !self.member_suspended(&invite.member_id)
    }

    pub fn invite_for(&self, room: &str, token: Option<&str>) -> Option<InviteRecord> {
        let token = token?;
        let inv = self.invite_by_token(token)?;
        (inv.room == room).then_some(inv)
    }

    /// Resolve the canonical principal carried by an active invite for this
    /// loca. Native listeners commonly authenticate their WebSocket directly
    /// with the invite instead of first minting a session; principal-scoped
    /// Everyone Care must work for both credential forms.
    pub fn principal_id_for_invite(&self, room: &str, token: Option<&str>) -> Option<String> {
        let invite = self.invite_for(room, token)?;
        self.member_principal(&invite.member_id)
    }

    /// Does this member already hold a live invite for this loca?
    /// Does this principal hold a live invite for `room`?
    pub fn principal_holds_invite(&self, room: &str, principal: &str) -> bool {
        let members: Vec<String> = self
            .invites
            .lock_or_recover()
            .values()
            .filter(|i| i.room == room)
            .map(|i| i.member_id.clone())
            .collect();
        members
            .iter()
            .any(|m| self.member_principal(m).as_deref() == Some(principal))
    }

    /// Does the membership `member_id` hold a live invite for `room`?
    pub fn has_active_invite(&self, room: &str, member_id: &str) -> bool {
        self.invites
            .lock_or_recover()
            .values()
            .any(|i| i.room == room && i.member_id == member_id)
    }

    /// Which loca does this invite open, if it is one? The presented invite
    /// is hashed and looked up.
    pub fn invite_room(&self, token: &str) -> Option<String> {
        self.invites
            .lock_or_recover()
            .get(&secret_hash(token))
            .map(|i| i.room.clone())
    }

    /// Is this token a live invite at all — for any loca? Used by the blanket
    /// gate, which cannot know the target loca; the per-loca decision is
    /// `may_enter`.
    pub fn is_invite_token(&self, token: Option<&str>) -> bool {
        token.is_some_and(|t| self.invite_by_token(t).is_some())
    }

    /// Mint an invite token from the operating system CSPRNG. Invites, memberships
    /// and smaster credentials are long-lived bearer authority, so a seeded
    /// hash/clock/counter construction is not an acceptable source of entropy.
    pub fn new_invite_token(&self) -> String {
        Self::secure_token("dv_", 32)
    }

    /// THE one way an invite comes to exist. Both doors — the terminal
    /// (`POST /invites` via invite.sh) and the UI (`+ call` via
    /// `POST /rooms/:id/call`) — land here, so the rules cannot drift apart
    /// again: an invite seats an EXISTING member (it never creates identity),
    /// and one member holds at most one live invite per loca.
    ///
    /// Capacity stays a door rule (`join`), not an invite rule: "a full loca
    /// refuses the eighth even with an invite" is the tested behaviour — more
    /// invites than seats may exist, the seats decide.
    ///
    /// `member_id` is the membership's id (`mbid_`). The answer is the
    /// invite's record and the invite itself, handed over once here.
    pub fn invite_member_to_room(
        &self,
        member_id: &str,
        room: &str,
        issued_by: &str,
    ) -> Result<(InviteRecord, String), InviteError> {
        self.invite_member_to_room_inner(member_id, room, issued_by, false)
    }

    // The only private-pair exception is the internal human-to-agent call.
    // Public HTTP, Lobby call and the SSH console all use the non-exception path.
    pub(in crate::hub) fn invite_member_to_room_inner(
        &self,
        member_id: &str,
        room: &str,
        issued_by: &str,
        private_agent_call: bool,
    ) -> Result<(InviteRecord, String), InviteError> {
        if let Some(pair) = self.dm(room) {
            let allowed = private_agent_call
                && self
                    .member_by_id(member_id)
                    .is_some_and(|m| m.kind == "agent")
                && self
                    .member_principal(member_id)
                    .is_some_and(|p| pair.has(&p));
            if !allowed {
                return Err(InviteError::PrivatePair);
            }
        }
        if self.is_deleted(room) {
            return Err(InviteError::Sealed);
        }
        if !self.room_exists(room)
            && (!self.admin_open() || self.refuse_implicit_loca(room).is_some())
        {
            return Err(InviteError::MissingRoom);
        }
        let Some(member) = self.member_by_id(member_id) else {
            return Err(InviteError::MemberNotFound);
        };
        if room == self.reserved_room.as_str() && !self.caretakers.contains(&member.name) {
            return Err(InviteError::Reserved);
        }
        if self.is_lobby_room(room) {
            return Err(InviteError::Lobby);
        }
        // The group check, one invite per person, no more invites than seats,
        // and the write below share the invite lock: two concurrent calls (or
        // a call racing a group move) cannot each pass a check and then both
        // mint.
        let lifecycle = self.member_lifecycle.lock_or_recover();
        // Nobody outside a group is invited or called into its loca: add them
        // to the group first (PRINCIPLES "Groups").
        if self.is_group_loca(room) {
            let principal = member.principal_id.clone();
            let candidates: Vec<&str> = principal.as_deref().into_iter().collect();
            if self.group_gate(room, &candidates) != Some(true) {
                return Err(InviteError::NotInGroup);
            }
        }
        if self.has_active_invite(room, member_id) {
            return Err(InviteError::AlreadyInvited);
        }
        // Model A: the seat IS the invite (PRINCIPLES: "Invited participant — has
        // a seat in one loca"). A loca seats at most `room_capacity(room)` (its own
        // number, or the Building's), so no more than that many live invites
        // may exist for it — the cap is checked here at mint, not only when
        // someone connects. (The join-time guard stays as defence in depth.)
        let active = self.invite_count(room);
        if active >= self.room_capacity(room) {
            return Err(InviteError::Full);
        }
        let invite = self.new_invite_token();
        let inv = InviteRecord::issued(
            &invite,
            room,
            &member.id,
            // Snapshot for the audit trail — identity lives in the membership.
            &member.name,
            &member.kind,
            self.now(),
            issued_by,
        );
        self.add_invite_locked(inv.clone(), &invite)
            .map_err(|_| InviteError::Storage)?;
        // Wake an already-running agent in the building lobby. Failure only
        // means nobody is listening at this instant; the invite is sent again
        // on their next Lobby connect while its delivery copy is held (until
        // its first use, or 24 hours after issue). Sent under the invite
        // lock, so a re-issue or revoke of this seat cannot slip in between
        // the mint and its delivery: the Lobby bus carries one seat's invites
        // in the order they became live (see `reissue_invite`).
        let _ = self.lobby_tx.send(LobbyEvent::Called {
            member: member.id,
            room: inv.room.clone(),
            token: invite.clone(),
        });
        drop(lifecycle);
        Ok((inv, invite))
    }

    /// Give a member a fresh invite for a seat it already holds. The old invite
    /// ends and the new one is minted in one store transaction, with the same
    /// member, loca, name, kind and issuer; the new one is then delivered
    /// over the member's Lobby line exactly like a call. The seat itself does
    /// not move: the swap happens under the invite lock, so the loca's invite
    /// count never drops and a concurrent call cannot take the place, and a
    /// full loca re-issues as readily as an empty one.
    ///
    /// Re-issuing ends an invite, so it needs the authority that ending it needs
    /// (`may_end_invite`): a Smaster cannot re-issue what the Master issued.
    /// When the store write fails nothing changes, in the store or here.
    pub fn reissue_invite(
        &self,
        member_id: &str,
        room: &str,
        actor: &RequestAuthority,
    ) -> Result<(InviteRecord, String), ReissueError> {
        // The refusals a call gives, in the same order (see
        // `invite_member_to_room_inner`); public routes never re-issue a
        // direct message's invite.
        if self.dm(room).is_some() {
            return Err(ReissueError::Invite(InviteError::PrivatePair));
        }
        if self.is_deleted(room) {
            return Err(ReissueError::Invite(InviteError::Sealed));
        }
        if !self.room_exists(room)
            && (!self.admin_open() || self.refuse_implicit_loca(room).is_some())
        {
            return Err(ReissueError::Invite(InviteError::MissingRoom));
        }
        let Some(member) = self.member_by_id(member_id) else {
            return Err(ReissueError::Invite(InviteError::MemberNotFound));
        };
        if room == self.reserved_room.as_str() && !self.caretakers.contains(&member.name) {
            return Err(ReissueError::Invite(InviteError::Reserved));
        }
        let lifecycle = self.member_lifecycle.lock_or_recover();
        if self.is_group_loca(room) {
            let principal = member.principal_id.clone();
            let candidates: Vec<&str> = principal.as_deref().into_iter().collect();
            if self.group_gate(room, &candidates) != Some(true) {
                return Err(ReissueError::Invite(InviteError::NotInGroup));
            }
        }
        // One member holds at most one invite per loca since the mint rule;
        // rows from before it may hold more, and all of them end here.
        let held: Vec<InviteRecord> = self
            .invites
            .lock_or_recover()
            .values()
            .filter(|i| i.room == room && i.member_id == member.id)
            .cloned()
            .collect();
        // The invite the new one stands in for: the highest issuer, so a
        // re-issue never makes an invite easier to end than the one it replaces.
        let Some(replaced) = held
            .iter()
            .max_by_key(|i| {
                (
                    invite_issuer_rank(&i.issued_by),
                    std::cmp::Reverse(i.issued_at),
                )
            })
            .cloned()
        else {
            return Err(ReissueError::NotHeld);
        };
        if held
            .iter()
            .any(|invite| !self.may_end_invite(actor, invite))
        {
            return Err(ReissueError::Forbidden);
        }
        let invite_key = self.new_invite_token();
        let fresh = InviteRecord::issued(
            &invite_key,
            room,
            &member.id,
            &replaced.name,
            &replaced.kind,
            self.now(),
            &replaced.issued_by,
        );
        let ended: Vec<String> = held.iter().map(|i| i.secret_hash.clone()).collect();
        // As `add_invite_locked` does for a mint: the store refuses a sealed
        // loca too, but a memory-only Building has only this check.
        if self.is_deleted(room) {
            return Err(ReissueError::Invite(InviteError::Sealed));
        }
        if let Err(error) = self
            .store
            .reissue_invite(&ended, &fresh, &invite_key, (self.now_ms)())
        {
            tracing::error!(%error, room, "could not re-issue an invite");
            return Err(ReissueError::Invite(InviteError::Storage));
        }
        // Committed: swap the mirror in one step under the invite lock.
        {
            let mut invites = self.invites.lock_or_recover();
            for hash in &ended {
                invites.remove(hash);
            }
            invites.insert(fresh.secret_hash.clone(), fresh.clone());
        }
        self.hold_delivery(&invite_key);
        #[cfg(test)]
        self.run_reissue_swapped_hook();
        // What the old invite opened ends now: its sessions, then its socket;
        // then the fresh invite goes out. All of it stays under the invite lock,
        // which every other way an invite of this seat begins or ends (a call,
        // another re-issue, release, kick, revoke, suspension, membership
        // revoke, group move) also takes. So no later invite of this seat
        // exists while this clean-up runs (it works by member and loca, not
        // by token, and would end a session taken with one), and the Lobby
        // bus receives this seat's invites in the order they became live: the
        // last `called` a client gets is the invite the server holds. Only
        // in-memory work remains here: two short leaf locks (`sessions`,
        // `rooms`) and two `broadcast::Sender::send`s, which never wait for a
        // receiver.
        for invite in &held {
            self.revoke_sessions_for_invite(&invite.room, &invite.member_id);
        }
        self.recheck_connections(room);
        let _ = self.lobby_tx.send(LobbyEvent::Called {
            member: member.id,
            room: fresh.room.clone(),
            token: invite_key.clone(),
        });
        drop(lifecycle);
        Ok((fresh, invite_key))
    }

    #[cfg(test)]
    pub(in crate::hub) fn run_reissue_swapped_hook(&self) {
        let hook = self.reissue_swapped_hook.lock_or_recover().clone();
        if let Some(hook) = hook {
            hook();
        }
    }

    /// Test-only: set (or clear) the hook `reissue_invite` runs after its swap.
    #[cfg(test)]
    pub fn set_reissue_swapped_hook(&self, hook: Option<TestHook>) {
        *self.reissue_swapped_hook.lock_or_recover() = hook;
    }

    /// Record an invite. The master issues; we only persist and index it.
    #[cfg(test)]
    pub fn add_invite(&self, inv: InviteRecord, invite: &str) -> rusqlite::Result<()> {
        let _lifecycle = self.member_lifecycle.lock_or_recover();
        self.add_invite_locked(inv, invite)
    }

    /// `add_invite` for a caller that already holds the invite lock
    /// (`member_lifecycle`), which is not re-entrant. `invite` is the invite
    /// `inv` was derived from, in hand only at its mint.
    pub(in crate::hub) fn add_invite_locked(
        &self,
        inv: InviteRecord,
        invite: &str,
    ) -> rusqlite::Result<()> {
        if !inv.member_id.is_empty() && self.member_by_id(&inv.member_id).is_none() {
            return Err(rusqlite::Error::InvalidQuery);
        }
        if self.is_deleted(&inv.room) {
            return Err(rusqlite::Error::InvalidParameterName(
                "room is permanently sealed".into(),
            ));
        }
        // Every invite minted here is handed over on the member's Lobby, so
        // it keeps its delivery copy until its first use or deadline.
        self.store.issue_invite(&inv, invite)?;
        self.invites
            .lock_or_recover()
            .insert(inv.secret_hash.clone(), inv);
        self.hold_delivery(invite);
        Ok(())
    }

    /// An invite was minted for delivery: send it again on every Lobby connect
    /// of its member until it is first used or its window closes. This is the
    /// in-memory side of its bounded delivery copy.
    pub(crate) fn hold_delivery(&self, invite: &str) {
        self.deliveries
            .lock_or_recover()
            .insert(secret_hash(invite), invite.to_string());
    }

    /// Is an invite issued at `issued_at` still inside its delivery window at
    /// `now`? The counterpart of the store's `PAST_INVITE_DELIVERY_DEADLINE`.
    pub(in crate::hub) fn delivery_open(issued_at: u64, now: u64) -> bool {
        issued_at > 0 && now < issued_at.saturating_add(crate::store::INVITE_DELIVERY_WINDOW_MS)
    }

    /// The first session or loca socket taken with the invite whose hash is
    /// `invite_hash`: the holder has proved it has it, so its delivery ends
    /// (the copy is cleared and `first_used_at` stamped). Nothing is revoked
    /// or rotated, and an invite with no copy costs no write. A failed write is
    /// logged and the copy stays held, so the next use (or the deadline)
    /// clears it.
    pub fn note_invite_used_by_hash(&self, invite_hash: &str) {
        if !self.deliveries.lock_or_recover().contains_key(invite_hash) {
            return;
        }
        match self
            .store
            .end_invite_delivery_by_hash(invite_hash, (self.now_ms)())
        {
            Ok(_) => {
                self.deliveries.lock_or_recover().remove(invite_hash);
            }
            Err(error) => tracing::error!(
                %error,
                "could not end an invite's delivery copy at its first use; it stays until the next use or its deadline"
            ),
        }
    }

    /// The next invite this Lobby connection is owed, decided now: live, its
    /// copy still held, its window open, and not in `sent` (the hashes of
    /// what this pass already sent). A Lobby pass asks again before every
    /// frame instead of walking a list taken earlier, because sending a frame
    /// waits on the socket: an invite revoked, re-issued or used in the meantime
    /// is then no longer owed and is never sent, and an invite that replaced one
    /// is. The answer is the invite's record and the invite to deliver.
    pub fn next_owed_delivery(
        &self,
        member_id: &str,
        sent: &std::collections::HashSet<String>,
    ) -> Option<(InviteRecord, String)> {
        self.pending_deliveries(member_id)
            .into_iter()
            .find(|(invite, _)| !sent.contains(&invite.secret_hash))
    }

    /// The live invites of the membership `member_id` whose delivery is still
    /// owed: their copy is held and their window open at this moment, each
    /// with the invite. A Lobby connect sends each as an ordinary `called`,
    /// the same invite every time. A copy found past its deadline here is
    /// cleared now (a late delivery attempt), not sent.
    pub fn pending_deliveries(&self, member_id: &str) -> Vec<(InviteRecord, String)> {
        let now = (self.now_ms)();
        let held: Vec<(InviteRecord, String)> = {
            let candidates = self.invites_for_member(member_id);
            let deliveries = self.deliveries.lock_or_recover();
            candidates
                .into_iter()
                .filter_map(|invite| {
                    let invite_key = deliveries.get(&invite.secret_hash)?.clone();
                    Some((invite, invite_key))
                })
                .collect()
        };
        let (due, late): (Vec<_>, Vec<_>) = held
            .into_iter()
            .partition(|(invite, _)| Self::delivery_open(invite.issued_at, now));
        for (invite, _) in late {
            if let Err(error) = self
                .store
                .expire_invite_deliveries(now, Some(&invite.secret_hash))
            {
                tracing::error!(%error, "could not clear an invite delivery copy past its deadline");
            }
            self.deliveries
                .lock_or_recover()
                .remove(&invite.secret_hash);
        }
        due
    }

    /// Clear invite delivery copies whose window has closed or whose invite
    /// has ended, in the store and here. Called by the background sweep, so
    /// a copy nobody asks for again still ends on time. A storage error is
    /// logged as one; the copy is not sent past its deadline either way.
    pub fn expire_invite_deliveries(&self) {
        let now = (self.now_ms)();
        match self.store.expire_invite_deliveries(now, None) {
            Ok(0) => {}
            Ok(cleared) => tracing::info!(
                cleared,
                "invite delivery copies past their window or of ended invites were cleared"
            ),
            Err(error) => tracing::error!(
                %error,
                "could not clear invite delivery copies past their window; the sweep tries again"
            ),
        }
        let held: Vec<String> = self.deliveries.lock_or_recover().keys().cloned().collect();
        let ended: Vec<String> = {
            let invites = self.invites.lock_or_recover();
            held.into_iter()
                .filter(|hash| {
                    !invites
                        .get(hash)
                        .is_some_and(|invite| Self::delivery_open(invite.issued_at, now))
                })
                .collect()
        };
        let mut deliveries = self.deliveries.lock_or_recover();
        for hash in ended {
            deliveries.remove(&hash);
        }
    }

    /// The member's seats: one `(loca, ivid_)` per live invite. Ids only; no
    /// invite leaves here.
    pub fn seats_of(&self, member_id: &str) -> Vec<(String, String)> {
        let mut seats: Vec<(String, String)> = self
            .invites_for_member(member_id)
            .into_iter()
            .map(|invite| (invite.room, invite.id))
            .collect();
        seats.sort();
        seats
    }

    /// End an invite, by its hash. The row survives with `revoked_at` so "who
    /// was let in" stays answerable — revoking is not forgetting.
    ///
    /// Revoking cascades: an invite that dies takes with it any session minted
    /// from it and closes the connection it opened. Otherwise the door would be
    /// shut on paper while a live socket (and a still-valid session token) kept
    /// working — the audit's revoke-does-nothing hole.
    pub fn revoke_invite_by_hash(&self, invite_hash: &str) -> rusqlite::Result<bool> {
        // Under the invite lock, like every other way a seat's invite begins or
        // ends: a revoke that read this invite before a re-issue replaced it
        // would otherwise run its clean-up (by member and loca) after the
        // re-issue, ending the session and socket of the fresh invite.
        let _lifecycle = self.member_lifecycle.lock_or_recover();
        let Some(inv) = self.invites.lock_or_recover().get(invite_hash).cloned() else {
            return Ok(false);
        };
        self.store
            .revoke_invite_by_hash(invite_hash, (self.now_ms)())?;
        self.invites.lock_or_recover().remove(invite_hash);
        // Kill sessions bound to this invite's (loca, member) and close the
        // live connection in that loca for that name.
        self.revoke_sessions_for_invite(&inv.room, &inv.member_id);
        self.close_connections(&inv.room, &inv.name);
        Ok(true)
    }

    /// The management id (`ivid_`) of an invite in hand (a call's answer, a
    /// delivery), with v0.13.0's derivation. A stored invite's id is
    /// `InviteRecord::id`.
    pub fn invite_management_id(&self, token: &str) -> String {
        crate::store::hashed_id("ivid_", &format!("invite:{token}"))
    }

    /// The live invite whose management id (`ivid_`) is `reference`. A raw
    /// invite is not a reference and finds nothing.
    pub fn invite_by_ref(&self, reference: &str) -> Option<InviteRecord> {
        self.invites
            .lock_or_recover()
            .values()
            .find(|invite| invite.id == reference)
            .cloned()
    }

    pub fn revoke_invite_ref(&self, reference: &str) -> rusqlite::Result<bool> {
        match self.invite_by_ref(reference) {
            Some(invite) => self.revoke_invite_by_hash(&invite.secret_hash),
            None => Ok(false),
        }
    }

    /// Drop every session bound to a member (any loca). Used when a MEMBERSHIP
    /// is revoked — the identity itself is gone, so no session proving it may
    /// survive.
    pub(in crate::hub) fn revoke_sessions_for_member(&self, member_id: &str) {
        self.sessions
            .lock_or_recover()
            .retain(|_st, idy| idy.member.as_deref() != Some(member_id));
    }

    /// Drop every session bound to one invite's (loca, member). Used when a
    /// single INVITE is revoked — sessions for that member in OTHER locas (other
    /// invites) survive, only this seat's proof dies.
    pub(in crate::hub) fn revoke_sessions_for_invite(&self, room: &str, member_id: &str) {
        if member_id.is_empty() {
            return;
        }
        self.sessions.lock_or_recover().retain(|_st, idy| {
            !(idy.member.as_deref() == Some(member_id) && idy.loca.as_deref() == Some(room))
        });
    }

    /// Force any live socket in `room` held under `name` to close, by
    /// broadcasting the same `Kicked` frame moderation uses. This is the only
    /// way to reach into an already-open ws connection (authorization is
    /// checked at the handshake, not per-frame), so a cascade that must eject
    /// someone reuses it.
    pub(in crate::hub) fn close_connections(&self, room: &str, name: &str) {
        let rooms = self.rooms.lock_or_recover();
        if let Some(r) = rooms.get(room) {
            let _ = r.tx.send(ServerFrame::Kicked {
                name: name.to_string(),
                banned: false,
            });
        }
    }

    /// Make every live socket in `room` check its credentials now, so the
    /// ones whose invite or session has just ended close. A re-issue is not a
    /// kick: the seat goes on under the fresh invite, and a `kicked` frame
    /// tells a listener to forget the loca's invite, which would race the
    /// Lobby `called` carrying the fresh one and could erase it. The roster
    /// frame is one every client already takes on each join and leave; each
    /// socket re-checks its credentials before it forwards any frame.
    pub(in crate::hub) fn recheck_connections(&self, room: &str) {
        let rooms = self.rooms.lock_or_recover();
        if let Some(r) = rooms.get(room) {
            let _ = r.tx.send(ServerFrame::Members {
                members: r.member_list(&self.profile_cards()),
            });
        }
    }

    /// End every live invite a name holds for one loca. This is how "kick"
    /// and "release" stop the invite — without it, the ejected
    /// name's token would open the door right back (PRINCIPLES: kick stops the
    /// invite; release takes back the seat). The membership is untouched.
    pub fn revoke_invites_for(&self, room: &str, name: &str) -> rusqlite::Result<()> {
        self.store.revoke_invites_for(room, name, (self.now_ms)())?;
        self.revoke_invites_cache_for(room, name);
        Ok(())
    }

    pub(in crate::hub) fn revoke_invites_cache_for(&self, room: &str, name: &str) {
        for inv in self.end_invites_cache_for(room, name) {
            self.close_connections(&inv.room, &inv.name);
        }
    }

    /// End the cached invites (and their sessions) a name holds for one loca,
    /// without closing the connection: for a caller that sends its own, more
    /// specific `Kicked` right after. Ban does — the socket closes on the
    /// first `Kicked` it reads, so a generic one sent here would tell a banned
    /// person they were not banned.
    pub(in crate::hub) fn end_invites_cache_for(
        &self,
        room: &str,
        name: &str,
    ) -> Vec<InviteRecord> {
        let matches: Vec<InviteRecord> = self
            .invites
            .lock_or_recover()
            .values()
            .filter(|i| i.room == room && i.name == name)
            .cloned()
            .collect();
        {
            let mut invites = self.invites.lock_or_recover();
            for inv in &matches {
                invites.remove(&inv.secret_hash);
            }
        }
        for inv in &matches {
            self.revoke_sessions_for_invite(&inv.room, &inv.member_id);
        }
        matches
    }

    /// Every live invite held by one member (its id, as a session names it
    /// too), across all locas — for "who am I" and member views.
    pub fn invites_for_member(&self, member_id: &str) -> Vec<InviteRecord> {
        if member_id.is_empty() {
            return Vec::new();
        }
        self.invites
            .lock_or_recover()
            .values()
            .filter(|i| i.member_id == member_id)
            .cloned()
            .collect()
    }

    /// Live invites for one loca (master view).
    pub fn invites_of(&self, room: &str) -> Vec<InviteRecord> {
        let mut v: Vec<InviteRecord> = self
            .invites
            .lock_or_recover()
            .values()
            .filter(|i| i.room == room)
            .cloned()
            .collect();
        v.sort_by_key(|i| i.issued_at);
        v
    }
}
