//! Sessions: minting, resolving, revoking and sweeping them.

use super::{
    secret_hash, AdminSessionGrant, BuildingRole, Hub, InviteRecord, MemberRecord, Ordering,
    RecoverMutex, SenderType, SessionIdentity,
};

impl Hub {
    /// Same, but taken with an invite: the session is confined to the invite's
    /// loca AND its identity comes from the invite's member — the session is
    /// proof of who the invite seats, not of whatever name the request body
    /// claims. (Before this, alice's invite could mint a "bob" session.)
    ///
    /// `admin_grant` is resolved server-side from a principal credential (or
    /// the one-use root pairing). It binds both role and credential ownership;
    /// request-body names never manufacture authority.
    ///
    /// `building_key` says the session was taken with a credential that
    /// reaches the whole Building through the legacy building key; only then
    /// does a loca-less session inherit that reach. `key_member` is the
    /// membership whose own key took the session when no principal grant can
    /// bind it (a memory-only store): the session speaks as that member and
    /// dies with the membership.
    ///
    /// `Ok(None)`: the invite or membership the caller resolved ended before
    /// the session could be recorded, so no session exists.
    pub fn create_session_scoped(
        &self,
        req: protocol::CreateSession,
        invite_record: Option<&InviteRecord>,
        admin_grant: Option<AdminSessionGrant>,
        building_key: bool,
        key_member: Option<MemberRecord>,
    ) -> rusqlite::Result<Option<protocol::SessionInfo>> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let now = (self.now_ms)();
        // A session is a bearer credential. Generate it directly from the
        // operating system CSPRNG; timestamps, counters and SipHash output are
        // not suitable secrets.
        let t = Self::secure_token("st_", 32);
        // The session names the membership by its id; whether it still
        // stands is checked below.
        let key_member_id = key_member.as_ref().map(|member| member.id.clone());
        let identity = match invite_record {
            Some(inv) => {
                // The membership is the identity; the invite's snapshot is the
                // fallback if the member record has since been revoked.
                let (name, kind) = match self.member_by_id(&inv.member_id) {
                    Some(m) => (m.name, m.kind),
                    None => (inv.name.clone(), inv.kind.clone()),
                };
                let kind = match kind.as_str() {
                    "user" => SenderType::User,
                    _ => SenderType::Agent,
                };
                // The principal comes from the membership record, not from
                // the member's original key: an invite session must not depend
                // on that one credential still being live (see `invite`).
                SessionIdentity {
                    principal_id: self.member_principal(&inv.member_id),
                    building_role: BuildingRole::Member,
                    name,
                    kind,
                    loca: Some(inv.room.clone()),
                    member: Some(inv.member_id.clone()),
                    invite: Some(inv.secret_hash.clone()),
                    building_key: false,
                    admin: false,
                    expires_at: now.saturating_add(Self::ADMIN_SESSION_TTL_MS),
                }
            }
            // No invite: a Building-level session. A verified credential names
            // it through `admin_grant` (its principal) or, in a memory-only
            // store, through `key_member` (its membership). Only a session with
            // neither — the shared building key, the sandbox — takes its name
            // from the body; that name is a label, and the key it was taken
            // with is the only authority it carries.
            None => SessionIdentity {
                principal_id: admin_grant
                    .as_ref()
                    .and_then(|grant| grant.authority.principal_id.clone()),
                building_role: admin_grant
                    .as_ref()
                    .and_then(|grant| grant.authority.building_role)
                    .unwrap_or(BuildingRole::Member),
                name: admin_grant
                    .as_ref()
                    .and_then(|grant| grant.authority.display_name.clone())
                    .or_else(|| key_member.as_ref().map(|m| m.name.clone()))
                    .unwrap_or_else(|| req.name.clone()),
                kind: admin_grant
                    .as_ref()
                    .and_then(|grant| grant.authority.kind)
                    .or_else(|| {
                        key_member.as_ref().map(|m| match m.kind.as_str() {
                            "user" => SenderType::User,
                            _ => SenderType::Agent,
                        })
                    })
                    .unwrap_or_else(|| req.kind.unwrap_or(SenderType::Agent)),
                loca: None,
                member: key_member.map(|m| m.id),
                invite: None,
                building_key,
                admin: admin_grant
                    .as_ref()
                    .map(|grant| grant.authority.is_building_admin())
                    .unwrap_or(false),
                expires_at: now.saturating_add(
                    admin_grant
                        .as_ref()
                        .map_or(Self::ADMIN_SESSION_TTL_MS, |grant| grant.ttl_ms),
                ),
            },
        };
        let name = identity.name.clone();
        let session_admin = identity.admin;
        let expires_at = Some(identity.expires_at);
        // Record the session under the session map's lock, and only while the
        // invite or membership it was taken with still stands. Every revocation
        // and suspension removes the invite from `invites` (and a revoked
        // membership from `members`) BEFORE it sweeps this map, so checking
        // under the same lock as the insert leaves no window: a revocation
        // either finished before this check (the session is refused) or
        // sweeps after the insert (the session dies with the rest). Lock
        // order: sessions -> invites, members, the store's connection, each
        // taken and released in turn under it (session_identity already
        // reaches the store under this lock). This is the record-time half of
        // the one rule for an invite session (`invite_session_live` is the
        // use-time half): it stands while its invite stands. The credential's
        // session cap is enforced inside this same critical section: the
        // store transaction below counts, ends the oldest and inserts, and
        // the ended sessions leave this map before the new one enters it, so
        // "source still stands, count, evict, insert" is one step.
        let mut sessions = self.sessions.lock_or_recover();
        let source_live = match invite_record {
            Some(inv) => self.invite_live_by_hash(&inv.secret_hash),
            None => key_member_id
                .as_deref()
                .is_none_or(|member| self.member_by_id(member).is_some()),
        };
        if !source_live {
            return Ok(None);
        }
        if identity.admin {
            self.store.save_admin_session(
                &t,
                &identity.name,
                identity.kind,
                identity.expires_at,
            )?;
        }
        // An invite session is taken with the invite, not with a credential, so
        // it gets no `principal_sessions` row: revoking a credential ends the
        // sessions taken with that credential and nothing else.
        let credential_id = match invite_record {
            Some(_) => None,
            None => admin_grant.and_then(|grant| grant.credential_id),
        };
        // A session filed under a credential counts against that credential's
        // cap (`store::SESSIONS_PER_CREDENTIAL`); the store ends the oldest in
        // the same transaction and names them so memory forgets them too.
        let mut ended = Vec::new();
        if let (Some(principal_id), Some(credential_id)) =
            (identity.principal_id.as_deref(), credential_id.as_deref())
        {
            ended = self.store.save_principal_session(
                &t,
                principal_id,
                credential_id,
                now,
                identity.expires_at,
            )?;
        }
        if !ended.is_empty() {
            tracing::info!(
                ended = ended.len(),
                "a credential took more than its live sessions allow; its oldest were ended"
            );
        }
        for session_hash in &ended {
            sessions.remove(session_hash);
        }
        // An invite session is filed under its invite, not under a credential
        // (above), so the credential's cap never counts it. Its own cap is
        // counted here, by the invite, in this same critical section: one
        // invite holds at most `SESSIONS_PER_CREDENTIAL` sessions, and taking
        // one more ends its oldest. These sessions live in memory only, so
        // there is no row to end.
        if let Some(invite_hash) = identity.invite.as_deref() {
            let mut held: Vec<(u64, String)> = sessions
                .iter()
                .filter(|(_, held)| held.invite.as_deref() == Some(invite_hash))
                .map(|(hash, held)| (held.expires_at, hash.clone()))
                .collect();
            if held.len() >= crate::store::SESSIONS_PER_CREDENTIAL {
                // Every session has the same lifetime, so the earliest to
                // expire is the oldest.
                held.sort();
                let over = held.len() + 1 - crate::store::SESSIONS_PER_CREDENTIAL;
                tracing::info!(
                    ended = over,
                    "an invite took more than its live sessions allow; its oldest were ended"
                );
                for (_, session_hash) in held.into_iter().take(over) {
                    sessions.remove(&session_hash);
                }
            }
        }
        sessions.insert(secret_hash(&t), identity);
        drop(sessions);
        if let Some(invite) = invite_record {
            self.note_invite_used_by_hash(&invite.secret_hash);
        }
        Ok(Some(protocol::SessionInfo {
            participant_id: format!("p_{id}"),
            session_token: t,
            name,
            admin: session_admin,
            expires_at,
        }))
    }

    /// Resolve a session token to its bound identity. An expired session (admin
    /// TTL passed) resolves to nothing — and is dropped — so a stale token is
    /// as good as no token.
    pub fn session_identity(&self, token: Option<&str>) -> Option<SessionIdentity> {
        let session_hash = secret_hash(token?);
        let idy = self.sessions.lock_or_recover().get(&session_hash)?.clone();
        let live = match idy.invite.as_deref() {
            Some(invite_hash) => self.invite_session_live(&idy, invite_hash),
            None => {
                idy.principal_id.is_none()
                    || !self.store.is_persistent()
                    || self
                        .store
                        .principal_session_active_by_hash(&session_hash, (self.now_ms)())
            }
        };
        let mut sessions = self.sessions.lock_or_recover();
        if !live {
            sessions.remove(&session_hash);
            return None;
        }
        if idy.expires_at <= (self.now_ms)() {
            sessions.remove(&session_hash);
            if idy.admin {
                let _ = self.store.delete_admin_session_by_hash(&session_hash);
            }
            return None;
        }
        Some(idy)
    }

    /// An invite session is live while the invite it was taken with is live and
    /// the member's principal is active, whatever happened to the member's
    /// credentials. Revoking, releasing or kicking the invite, suspending the
    /// member and revoking the membership all end it. The use-time half of
    /// the rule `create_session_scoped` checks when it records the session.
    /// Takes no lock while holding another: `invites`, then the store.
    pub(in crate::hub) fn invite_session_live(
        &self,
        idy: &SessionIdentity,
        invite_hash: &str,
    ) -> bool {
        let (Some(room), Some(member)) = (idy.loca.as_deref(), idy.member.as_deref()) else {
            return false;
        };
        let invite_live = self
            .invites
            .lock_or_recover()
            .get(invite_hash)
            .is_some_and(|i| i.room == room && i.member_id == member);
        invite_live
            && match idy.principal_id.as_deref() {
                Some(principal) if self.store.is_persistent() => {
                    self.store.active_principal(principal).is_some()
                }
                _ => true,
            }
    }

    /// Does this request carry admin authority — either the raw admin/master
    /// key in the header, OR a live admin session token? This is the one place
    /// the two are treated as equal, so the browser can hold a short-lived
    /// session instead of the raw key (PRINCIPLES: the key never leaves .env).
    pub fn is_admin_session(&self, session_token: Option<&str>) -> bool {
        self.resolve_authority(None, session_token)
            .is_building_admin()
    }

    /// Revoke one browser/agent session immediately. Logout must end the
    /// server-side bearer credential, not merely forget it in the client.
    pub fn revoke_session(&self, token: &str) -> rusqlite::Result<bool> {
        let session_hash = secret_hash(token);
        let admin = self
            .sessions
            .lock_or_recover()
            .get(&session_hash)
            .is_some_and(|identity| identity.admin);
        if admin {
            self.store.delete_admin_session_by_hash(&session_hash)?;
        }
        self.store
            .revoke_principal_session_by_hash(&session_hash, self.now())?;
        Ok(self
            .sessions
            .lock_or_recover()
            .remove(&session_hash)
            .is_some())
    }

    /// End what has expired without waiting for someone to present it: drop
    /// expired sessions from memory, then delete expired and ended sessions
    /// from storage. Called by the background sweep. Memory is swept even when
    /// storage fails; a storage error is logged as one and returned, never
    /// read as "nothing to sweep". Returns how many stored sessions went.
    pub fn sweep_sessions(&self) -> rusqlite::Result<usize> {
        let now = (self.now_ms)();
        let forgotten = {
            let mut sessions = self.sessions.lock_or_recover();
            let before = sessions.len();
            sessions.retain(|_, identity| identity.expires_at > now);
            before - sessions.len()
        };
        if forgotten > 0 {
            tracing::info!(forgotten, "expired sessions were dropped from memory");
        }
        match self.store.sweep_sessions(now) {
            Ok(0) => Ok(0),
            Ok(deleted) => {
                tracing::info!(
                    deleted,
                    "expired and ended sessions were deleted from storage"
                );
                Ok(deleted)
            }
            Err(error) => {
                tracing::error!(
                    %error,
                    "could not delete expired and ended sessions from storage; none deleted"
                );
                Err(error)
            }
        }
    }
}
