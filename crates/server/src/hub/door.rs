//! The door: who may enter which loca, and which identity a request acts as.

use super::{
    secret_hash, BuildingRole, EnterDecision, Hub, LegacyOwner, Message, OperationScope,
    PrivateDoor, RequestAuthority, SenderType, SessionIdentity,
};

impl Hub {
    /// May this room credential enter `room`? An active per-device credential
    /// inherits the transitional building-key access of its durable Member
    /// record; loca-specific invites remain separate, scoped bearers.
    /// That indirection is important: revoking the original legacy bearer must
    /// close the door, while another credential of the same principal keeps
    /// the principal's inherited building-key access.
    pub fn may_enter(&self, room: &str, token: Option<&str>) -> bool {
        self.invite_for(room, token).is_some() || self.reaches_building(token)
    }

    /// Does this room credential reach every ordinary loca through the
    /// building key (`may_enter` without an invite)?
    pub fn reaches_building(&self, token: Option<&str>) -> bool {
        let Some(token) = token else {
            return self.is_member(None);
        };
        if self.store.credential_is_revoked(token) {
            return false;
        }
        if let Some(member) = self.member_for_credential(Some(token)) {
            // What the building key's rule answers for the membership's own
            // key, asked by that key's hash: an installation upgraded with
            // ROOM_TOKEN set to a member's key still reaches the building
            // with every credential of that member.
            return self.is_member_key_hash(&member.secret_hash);
        }
        // Memory-only/dev mode and an as-yet unmigrated shared ROOM_TOKEN keep
        // their compatibility behavior. A known revoked credential never
        // reaches this fallback.
        self.is_member(Some(token))
    }

    /// THE single door decision. Every gate — the blanket middleware, the REST
    /// loca check, and the WS handshake — calls this and only this. Before, each
    /// had its own copy of the rules and fixing one broke another; this is the
    /// one place the rules live.
    ///
    /// Inputs are the three credentials a caller can present (HTTP headers on
    /// REST, WebSocket subprotocol credentials on WS):
    ///   - `admin_tok`: the admin key (`x-admin-token`) — master/smaster
    ///   - `invite_tok`: an invite or the building key (`x-room-token`)
    ///   - `session`:   an identity taken earlier (`x-session-token`)
    ///
    /// A `name_for_ban` hint lets the WS door pass the query `?name=` when there
    /// is no session yet; REST passes `None` and the ban is judged from whatever
    /// identity the credentials resolve to.
    pub fn enter_decision(
        &self,
        room: &str,
        admin_tok: Option<&str>,
        invite_tok: Option<&str>,
        session: Option<&SessionIdentity>,
        name_hint: Option<&str>,
    ) -> EnterDecision {
        // Rank first, and rank ignores bans: the master is never banned from
        // their own building. master + smaster, resolved in ONE place — by the
        // raw admin key OR a live admin session (the browser holds the session,
        // not the key). Without the session branch the door turned the master
        // away in invite mode: the admin session's loca is None, so it matched no
        // loca and every room read/WS handshake 401'd.
        let credential_authority = self.resolve_authority(admin_tok, None);
        // A Master Agent's door is decided here, before every other rule: it
        // enters only a loca it opened itself (`hub/master_agent.rs`).
        if let Some(session) = session.filter(|s| s.building_role == BuildingRole::MasterAgent) {
            return self.master_agent_door(room, session.principal_id.as_deref());
        }
        if Self::is_master_agent(&credential_authority) {
            return self.master_agent_door(room, credential_authority.principal_id.as_deref());
        }
        if session.is_some_and(|s| s.loca.as_deref().is_some_and(|scope| scope != room)) {
            return EnterDecision::Denied;
        }
        if self.is_lobby_room(room) {
            return self.lobby_door(
                &credential_authority,
                admin_tok,
                invite_tok,
                session,
                name_hint,
            );
        }
        // A direct message and a group loca come BEFORE the rank bypass below: no
        // Smaster reaches either by rank (see `private_door`).
        match self.private_door(
            room,
            &credential_authority,
            admin_tok,
            invite_tok,
            session,
            name_hint,
        ) {
            PrivateDoor::Private(decision) => return decision,
            PrivateDoor::Reserved => return EnterDecision::Denied,
            PrivateDoor::Ordinary => {}
        }
        if credential_authority.is_building_admin() && admin_tok.is_some() {
            return EnterDecision::Allowed;
        }
        if session.map(|s| s.admin).unwrap_or(false) {
            return EnterDecision::Allowed;
        }

        let ban_name = self.door_name(invite_tok, session, &credential_authority, name_hint);
        if let Some(n) = &ban_name {
            if self.is_banned(room, n) {
                return EnterDecision::Banned;
            }
        }

        let principal_at_door = session
            .and_then(|identity| identity.principal_id.as_deref())
            .or(credential_authority.principal_id.as_deref());
        if principal_at_door
            .is_some_and(|principal_id| self.is_operator_principal(room, principal_id))
        {
            return EnterDecision::Allowed;
        }

        // An invite for THIS loca, or the building key (is_member) — may_enter
        // already folds both. An invite for another loca opens nothing.
        if self.may_enter(room, invite_tok) {
            return EnterDecision::Allowed;
        }

        // A person signed in with their own key holds a live invite for this
        // loca: the invite is their seat, whichever credential they came with.
        if let Some(principal) = session.and_then(|s| s.principal_id.as_deref()) {
            if self.principal_holds_invite(room, principal) {
                return EnterDecision::Allowed;
            }
        }

        // A session opens the loca it was minted for.
        if let Some(s) = session {
            match &s.loca {
                Some(l) if l == room => return EnterDecision::Allowed,
                Some(_) => {}
                // A session with no loca was taken with a Building-level
                // credential. Only one taken with the building key itself
                // (`building_key`) reaches the whole building, exactly as the
                // key does: the browser takes a session once and stops sending
                // the key. A session taken with a member's own key or profile
                // credential is that member signed in — it got everything it
                // is entitled to from the rules above and nothing from the key.
                // In invite-only mode (no ROOM_TOKEN) there is no building key,
                // so a loca-less session reaches nothing through it.
                None if s.building_key && !self.room_token.is_empty() => {
                    return EnterDecision::Allowed
                }
                None => {}
            }
        }

        EnterDecision::Denied
    }

    /// Whose name is at this door? A session names itself; otherwise an invite
    /// carries a name; otherwise the caller's ?name= hint. This is the name a
    /// ban is checked against.
    pub(in crate::hub) fn door_name(
        &self,
        invite_tok: Option<&str>,
        session: Option<&SessionIdentity>,
        credential_authority: &RequestAuthority,
        name_hint: Option<&str>,
    ) -> Option<String> {
        session
            .map(|s| s.name.clone())
            .or_else(|| {
                invite_tok
                    .and_then(|t| self.invite_by_token(t))
                    .map(|i| i.name)
            })
            // A member's own key presented as the room credential names its
            // member: without this the Lobby's ban was checked against the
            // caller's ?name= hint, which the caller chooses.
            .or_else(|| {
                invite_tok
                    .and_then(|t| self.member_for_credential(Some(t)))
                    .map(|m| m.name)
            })
            .or_else(|| credential_authority.display_name.clone())
            .or_else(|| name_hint.map(str::to_string))
    }

    /// The door of a loca whose very existence is private: a direct message, the
    /// direct message id space, a group loca. Everything here that costs anything
    /// (the invite's principal, which may read SQLite; the direct message and group
    /// lookups) is read before the answer depends on what the id names, so
    /// an id that names nothing costs this door what a hidden loca costs and
    /// timing does not tell them apart (PRINCIPLES "Birebir", "Groups").
    pub(in crate::hub) fn private_door(
        &self,
        room: &str,
        credential_authority: &RequestAuthority,
        admin_tok: Option<&str>,
        invite_tok: Option<&str>,
        session: Option<&SessionIdentity>,
        name_hint: Option<&str>,
    ) -> PrivateDoor {
        let pair = self.dm(room);
        // An agent at the door may hold an invite for this very loca: the invite
        // the opener's call minted for a direct message, or one for a group loca.
        let invited = self.principal_id_for_invite(room, invite_tok);
        let mut principals: Vec<&str> = Vec::new();
        if let Some(s) = session {
            // An invite session stays confined to the loca its invite opened.
            if s.loca.as_deref().is_none_or(|l| l == room) {
                principals.extend(s.principal_id.as_deref());
            }
        }
        principals.extend(credential_authority.principal_id.as_deref());
        principals.extend(invited.as_deref());
        let group_member = self.group_gate(room, &principals);

        // A direct message belongs to its two people: no Master or Smaster reaches
        // one by rank, and nobody else learns it exists (PRINCIPLES
        // "Birebir"). The other side may be an agent holding the invite the
        // opener's call minted for this room, or a session taken with it.
        if let Some(b) = pair {
            let principal = session
                .and_then(|s| s.principal_id.as_deref())
                .or(credential_authority.principal_id.as_deref());
            let party =
                principal.is_some_and(|p| b.has(p)) || invited.as_deref().is_some_and(|p| b.has(p));
            return PrivateDoor::Private(if party {
                EnterDecision::Allowed
            } else {
                EnterDecision::Denied
            });
        }
        // The direct message id space is reserved: nobody reaches (and so nobody
        // creates by joining or posting) an ordinary loca there, so a pair's
        // room can never be squatted with someone else's invites in it.
        if room.starts_with(Self::DM_PREFIX) {
            return PrivateDoor::Reserved;
        }
        // A group loca belongs to its group: a Smaster outside the group is
        // an outsider here, and only the Master reaches it by rank
        // (PRINCIPLES "Groups").
        match group_member {
            Some(member) => PrivateDoor::Private(self.group_door(
                room,
                credential_authority,
                admin_tok,
                invite_tok,
                session,
                name_hint,
                member,
            )),
            None => PrivateDoor::Ordinary,
        }
    }

    /// The door's answer for a direct message or a group loca, `None` for any other
    /// id; the same answer `enter_decision` gives. A door asks this where it
    /// decides whether the loca exists, so a caller who may not enter one is
    /// refused there exactly as for an id that names nothing, after the same
    /// work (see `private_door`), and no later fence is first to refuse.
    pub fn private_loca_decision(
        &self,
        room: &str,
        admin_tok: Option<&str>,
        invite_tok: Option<&str>,
        session: Option<&SessionIdentity>,
    ) -> Option<EnterDecision> {
        let credential_authority = self.resolve_authority(admin_tok, None);
        // A Master Agent is no member of the floor; the loca it opened there
        // is the one private loca whose existence it knows.
        let master_agent = session
            .filter(|s| s.building_role == BuildingRole::MasterAgent)
            .map(|s| s.principal_id.clone())
            .or_else(|| {
                Self::is_master_agent(&credential_authority)
                    .then(|| credential_authority.principal_id.clone())
            });
        if let Some(principal) = master_agent {
            if self.master_agent_door(room, principal.as_deref()) == EnterDecision::Allowed {
                return Some(EnterDecision::Allowed);
            }
        }
        let scoped_elsewhere =
            session.is_some_and(|s| s.loca.as_deref().is_some_and(|scope| scope != room));
        match self.private_door(
            room,
            &credential_authority,
            admin_tok,
            invite_tok,
            session,
            None,
        ) {
            PrivateDoor::Private(_) if scoped_elsewhere => Some(EnterDecision::Denied),
            PrivateDoor::Private(decision) => Some(decision),
            PrivateDoor::Reserved | PrivateDoor::Ordinary => None,
        }
    }

    /// The door of a group loca. The Master enters by rank; anyone else only
    /// as a member of the loca's group (`member`, from `private_door`: by the
    /// principal of their session or credential, or, for an agent, of the
    /// invite it holds for this loca). A member still answers to the loca's
    /// bans.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::hub) fn group_door(
        &self,
        room: &str,
        credential_authority: &RequestAuthority,
        admin_tok: Option<&str>,
        invite_tok: Option<&str>,
        session: Option<&SessionIdentity>,
        name_hint: Option<&str>,
        member: bool,
    ) -> EnterDecision {
        let master = (credential_authority.is_master()
            && (admin_tok.is_some() || self.admin_open()))
            || (!self.admin_token.is_empty() && invite_tok == Some(self.admin_token.as_str()))
            || session.is_some_and(|s| s.admin && s.building_role == BuildingRole::Master);
        if master {
            return EnterDecision::Allowed;
        }
        if !member {
            return EnterDecision::Denied;
        }
        if let Some(n) = self.door_name(invite_tok, session, credential_authority, name_hint) {
            if self.is_banned(room, &n) {
                return EnterDecision::Banned;
            }
        }
        EnterDecision::Allowed
    }

    /// The seat key for a connection — one key = one seat. Derived from the
    /// strongest credential at the door, so the same key entering under two
    /// display names lands on ONE seat (the name is just the label).
    ///
    /// Precedence mirrors `enter_decision`: rank first (the master is the
    /// master even if they also hold an invite), then the invite's member, then
    /// the session, then the bare name.
    ///
    /// In open dev mode (no ADMIN_TOKEN) rank identifies nobody, so identity
    /// falls through to the name — two differently-named anons stay distinct,
    /// exactly as before.
    ///
    /// The seat key travels in `Evicted` frames, so it names a membership by
    /// its management id (`mb:<mbid_>`) and an unbound legacy invite by its id
    /// (`dv:<ivid_>`), never by the key or the invite.
    pub fn seat_identity(
        &self,
        admin_tok: Option<&str>,
        invite_tok: Option<&str>,
        session: Option<&SessionIdentity>,
        session_tok: Option<&str>,
        name: &str,
    ) -> String {
        if let Some(t) = admin_tok {
            if let Some(smaster) = self.smaster_for_code(Some(t)) {
                return format!("sm:pr:{}", smaster.principal_id);
            }
            if !self.admin_token.is_empty() && t == self.admin_token.as_str() {
                // ONE fixed key for the master: whatever name they type, the
                // seat is the same. Per-room, because each roster is per-room.
                return "@master".into();
            }
        }
        if let Some(inv) = invite_tok.and_then(|t| self.invite_by_token(t)) {
            if !inv.member_id.is_empty() {
                // Two invites of the same member (or the same invite twice) are
                // one person — the membership is the identity.
                return format!("mb:{}", inv.member_id);
            }
            return format!("dv:{}", inv.id);
        }
        if let Some(member) = self.member_for_credential(invite_tok) {
            return format!("mb:{}", member.id);
        }
        if let Some(s) = session {
            if s.admin {
                // `admin` is true for every Building administrator, Smaster
                // included. A Smaster is a different person: give it the seat
                // of its own principal, or it would share (and keep evicting)
                // the Master's seat in every loca. Every Smaster seat starts
                // `sm:` so the roster can show the rank (`seat_rank`).
                if s.building_role == BuildingRole::Smaster {
                    if let Some(principal) = &s.principal_id {
                        return format!("sm:pr:{principal}");
                    }
                    if let Some(t) = session_tok {
                        // The seat key travels in `Evicted` frames: name the
                        // session by its hash, never by its token.
                        return format!("sm:st:{}", secret_hash(t));
                    }
                }
                // Every session minted for the one Master principal occupies the
                // same per-loca seat, regardless of the display name used
                // while taking the session.
                return "@master".into();
            }
            if let Some(m) = &s.member {
                return format!("mb:{m}");
            }
            if let Some(t) = session_tok {
                return format!("st:{}", secret_hash(t));
            }
        }
        format!("name:{name}")
    }

    /// Who an `op_id` belongs to (PRINCIPLES: a message that arrives twice
    /// does not produce its effect twice). The same sender gets the same key
    /// over REST and over the WebSocket, under any of its sessions and after
    /// a restart: the principal the door verified, read in the order the
    /// socket names its writer (the session's; else the invite's or the
    /// membership credential's; else the one behind an admin credential,
    /// which is the Master's for the root key and the Smaster's for its
    /// code). Without a principal (a memory-only store) the membership is
    /// the key, by its management id (`mb:<mbid_>`). With neither (the open
    /// sandbox, the shared building key) the display name and kind are all
    /// that names the sender; that key has a prefix no earlier version wrote,
    /// so it cannot meet an older row.
    ///
    /// `legacy` is the key the REST route stored before this version, built
    /// from the same request. It is only ever read, so an operation sent
    /// before the upgrade is still answered with its original message. That
    /// route keyed an invite session by `mb:<membership key>`; `Store::open`
    /// rewrites every such stored key to `mb:<mbid_>` (`rewrite_key_scopes`),
    /// which is what an invite session names its membership by now, so the
    /// same lookup finds it.
    pub fn operation_scope(
        &self,
        room: &str,
        admin_tok: Option<&str>,
        invite_tok: Option<&str>,
        session: Option<&SessionIdentity>,
        kind: SenderType,
        name: &str,
    ) -> OperationScope {
        let kind = match kind {
            SenderType::Agent => "agent",
            SenderType::User => "user",
        };
        let (principal, member) = match session {
            Some(session) => (session.principal_id.clone(), session.member.clone()),
            None => (
                self.principal_id_for_invite(room, invite_tok)
                    .or_else(|| {
                        self.member_for_credential(invite_tok)
                            .and_then(|member| member.principal_id)
                    })
                    .or_else(|| {
                        admin_tok
                            .filter(|token| !token.is_empty())
                            .and_then(|token| {
                                self.resolve_authority(Some(token), None).principal_id
                            })
                    })
                    .or_else(|| self.smaster_for_code(admin_tok).map(|s| s.principal_id)),
                self.invite_for(room, invite_tok)
                    .map(|invite| invite.member_id)
                    .filter(|member| !member.is_empty())
                    .or_else(|| self.member_for_credential(invite_tok).map(|m| m.id)),
            ),
        };
        let key = match (&principal, &member) {
            (Some(principal), _) => format!("pr:{principal}"),
            (None, Some(member)) => format!("mb:{member}"),
            (None, None) => format!("label:{kind}:{name}"),
        };
        let legacy = match session.and_then(|session| session.member.as_deref()) {
            Some(member) => format!("mb:{member}"),
            None => format!("{kind}:{name}"),
        };
        OperationScope {
            legacy: (legacy != key).then_some(legacy),
            key,
            principal,
        }
    }

    /// Is a record an earlier version stored under `legacy` this sender's?
    /// Proved, never assumed. A record that names its writer belongs to that
    /// writer. One that names none (every REST row before writers were
    /// recorded) is this sender's only when its key can have had one sender:
    /// a membership key (`mb:`). An unverified sender (the open sandbox, the
    /// shared Building key) is its label before and after, so the label is
    /// its proof. A display name proves nothing for a verified sender: two
    /// principals can bear one name, a revoked member's name can be admitted
    /// again, and a Master or Smaster rename leaves no history, so the one
    /// principal that bears a name today need not be the one that wrote
    /// under it. Such a record is `Unknown`; otherwise two senders could
    /// each be answered with the other's message and have their own dropped.
    pub(in crate::hub) fn legacy_operation_owner(
        scope: &OperationScope,
        legacy: &str,
        record: &Message,
    ) -> LegacyOwner {
        if let Some(writer) = record.sender_principal.as_deref() {
            return if scope.principal.as_deref() == Some(writer) {
                LegacyOwner::Sender
            } else {
                LegacyOwner::Other
            };
        }
        if scope.principal.is_none() || legacy.starts_with("mb:") {
            LegacyOwner::Sender
        } else {
            LegacyOwner::Unknown
        }
    }
}
