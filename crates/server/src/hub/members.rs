//! Building membership: admitting, suspending and revoking members, and the names they may take.

use super::{
    admission_name_refusal, secret_hash, AdmitError, BuildingRole, Hub, InviteRecord, LobbyEvent,
    MemberRecord, RecoverMutex,
};

impl Hub {
    /// True if `token` may connect / post. Open when no room token is set; the
    /// admin token also counts (admins are members).
    pub fn is_member(&self, token: Option<&str>) -> bool {
        if self.room_token.is_empty() {
            // No building key. In invite-only mode this is not an open house —
            // the door is closed and only an invite (checked per-loca elsewhere)
            // opens it. Being a "member of the building" now means holding the
            // admin token; everyone else needs an invite for the specific loca.
            return !self.require_invite
                || (!self.admin_token.is_empty() && token == Some(self.admin_token.as_str()));
        }
        token == Some(self.room_token.as_str())
            || (!self.admin_token.is_empty() && token == Some(self.admin_token.as_str()))
    }

    /// `is_member` for the key whose hash is `key_hash`: the configured
    /// building and admin keys are hashed and compared with it.
    pub(in crate::hub) fn is_member_key_hash(&self, key_hash: &str) -> bool {
        let is = |configured: &str| !configured.is_empty() && secret_hash(configured) == key_hash;
        if self.room_token.is_empty() {
            return !self.require_invite || is(&self.admin_token);
        }
        is(&self.room_token) || is(&self.admin_token)
    }

    /// Admit somebody to the building. The founding act: it creates the
    /// identity every invite later refers to. Legacy admission remains idempotent by
    /// name, so admitting an existing member returns the existing principal
    /// rather than minting a duplicate membership. Authority still resolves
    /// from principal identity, never from display name or credential text.
    /// The two-step "admit & invite" flow can therefore be replayed safely.
    /// A new name must pass the doors' rule (`admission_name_refusal`) and
    /// must not be one someone already wears in any letter case
    /// (`new_member_name_clashes`). The lookup of an existing member, the
    /// rule and the write all happen under `name_claims`: two racing requests
    /// cannot both get in, two racing requests for the same exact name get
    /// the same member, and a member admitted before the rule is found again
    /// while it lasts, but its name is not admitted anew once it is revoked.
    pub fn admit_member(
        &self,
        name: &str,
        kind: &str,
        by: &str,
    ) -> Result<MemberRecord, AdmitError> {
        self.admit_member_once(name, kind, by)
            .map(|(member, _)| member)
    }

    /// `admit_member`, with the key when this call admitted the member
    /// (`Some`, the only time the key exists outside its holder) or `None`
    /// when it found the member already admitted: an existing member's key is
    /// not stored and cannot be read back out.
    pub fn admit_member_once(
        &self,
        name: &str,
        kind: &str,
        by: &str,
    ) -> Result<(MemberRecord, Option<String>), AdmitError> {
        let _claim = self.name_claims.lock_or_recover();
        if let Some(existing) = self.member_by_name(name) {
            return Ok((existing, None));
        }
        if let Some(rule) = admission_name_refusal(name) {
            return Err(AdmitError::Rule(rule));
        }
        if self.new_member_name_clashes(name) {
            return Err(AdmitError::Worn);
        }
        let key = self.new_invite_token().replacen("dv_", "mb_", 1);
        let m = self
            .store
            .add_member(&key, name, kind, (self.now_ms)(), by)?;
        if !self.store.is_persistent() {
            self.member_keys
                .lock_or_recover()
                .insert(secret_hash(&key), m.id.clone());
        }
        self.members
            .lock_or_recover()
            .insert(m.id.clone(), m.clone());
        Ok((m, Some(key)))
    }

    /// End a membership. Their invites are not touched here — losing the
    /// building is a decision of its own, and the record of where they sat
    /// stays readable.
    /// End a membership — and everything it authorized. PRINCIPLES: when a
    /// parent credential is revoked, every child dies with it. Losing the
    /// building means losing every seat (invite), every proof (session), and
    /// every open connection at once — otherwise "you no longer belong here"
    /// would leave the ejected person still reading and speaking. By the
    /// membership's id (`mbid_`).
    pub fn revoke_member(&self, member_id: &str) -> rusqlite::Result<bool> {
        let _lifecycle = self.member_lifecycle.lock_or_recover();
        if !self.members.lock_or_recover().contains_key(member_id) {
            return Ok(false);
        }
        let now = (self.now_ms)();
        self.store.revoke_member_cascade(member_id, now)?;
        let member_invites: Vec<InviteRecord> = self
            .invites
            .lock_or_recover()
            .values()
            .filter(|i| i.member_id == member_id)
            .cloned()
            .collect();
        {
            let mut invites = self.invites.lock_or_recover();
            for inv in &member_invites {
                invites.remove(&inv.secret_hash);
            }
        }
        for inv in member_invites {
            self.revoke_sessions_for_invite(&inv.room, &inv.member_id);
            self.close_connections(&inv.room, &inv.name);
        }
        // Every key its invites claimed stops working (the store revoked them
        // above; a memory-only Building holds them here), and so does its own.
        self.memory_claims
            .lock_or_recover()
            .forget_member(member_id);
        self.member_keys
            .lock_or_recover()
            .retain(|_, member| member != member_id);
        // Any session still bound to this member (its own key in a
        // memory-only store) dies too — the identity behind it is gone. The
        // membership leaves `members` before the sweep, so a session being
        // recorded concurrently sees it gone (`create_session_scoped`).
        self.members.lock_or_recover().remove(member_id);
        self.revoke_sessions_for_member(member_id);
        let _ = self.lobby_tx.send(LobbyEvent::MembershipRevoked {
            member: member_id.to_string(),
        });
        Ok(true)
    }

    pub fn set_member_suspended(
        &self,
        reference: &str,
        suspended: bool,
        reason: &str,
    ) -> rusqlite::Result<bool> {
        let _lifecycle = self.member_lifecycle.lock_or_recover();
        if !self.members.lock_or_recover().contains_key(reference) {
            return Ok(false);
        }
        let member_id = reference;
        if !suspended {
            return self.store.resume_member(member_id);
        }
        if !self.store.suspend_member(member_id, reason, self.now())? {
            return Ok(false);
        }
        let member_invites: Vec<InviteRecord> = self
            .invites
            .lock_or_recover()
            .values()
            .filter(|invite| invite.member_id == member_id)
            .cloned()
            .collect();
        {
            let mut invites = self.invites.lock_or_recover();
            for invite in &member_invites {
                invites.remove(&invite.secret_hash);
            }
        }
        for invite in member_invites {
            self.close_connections(&invite.room, &invite.name);
        }
        self.revoke_sessions_for_member(member_id);
        let _ = self.lobby_tx.send(LobbyEvent::MembershipSuspended {
            member: member_id.to_string(),
        });
        Ok(true)
    }

    /// Is the membership `member_id` suspended? A suspension that cannot be
    /// read counts as one (fail-closed).
    pub fn member_suspended(&self, member_id: &str) -> bool {
        self.store.member_suspended(member_id).unwrap_or(true)
    }

    pub fn member_suspension_supported(&self) -> bool {
        self.store.is_persistent()
    }

    /// End the membership whose management id (`mbid_`) is `reference`. A
    /// raw key is not a reference: it finds nothing, like any unknown one,
    /// so a key never has to travel in a URL.
    pub fn revoke_member_ref(&self, reference: &str) -> rusqlite::Result<bool> {
        self.revoke_member(reference)
    }

    /// The live, unsuspended membership whose id (`mbid_`) is `member_id`.
    pub fn member_by_id(&self, member_id: &str) -> Option<MemberRecord> {
        if member_id.is_empty() || self.member_suspended(member_id) {
            return None;
        }
        self.members.lock_or_recover().get(member_id).cloned()
    }

    /// Resolve an active per-device/member credential to its durable Building
    /// membership. The credential is hashed and looked up, never compared
    /// with anything stored: in a persistent Building through `credentials`
    /// (its principal, then the membership that principal is), in a
    /// memory-only one through `member_keys` and the keys invites claimed.
    /// Revoking the original bearer makes that bearer stop working, while
    /// another credential for the same principal still finds the same
    /// membership record.
    pub fn member_for_credential(&self, token: Option<&str>) -> Option<MemberRecord> {
        let token = token.filter(|token| !token.is_empty())?;
        if !self.store.is_persistent() {
            let hash = secret_hash(token);
            let member = self.member_keys.lock_or_recover().get(&hash).cloned();
            // A key an invite claimed in memory-only mode.
            let member =
                member.or_else(|| self.memory_claims.lock_or_recover().member_for_key(token))?;
            return self.member_by_id(&member);
        }
        let principal = self.store.principal_for_credential(token)?;
        if principal.role != BuildingRole::Member {
            return None;
        }
        self.members
            .lock_or_recover()
            .values()
            .find(|member| member.principal_id.as_deref() == Some(principal.id.as_str()))
            .cloned()
    }

    /// Is this a correct, unrevoked credential of a membership the Master has
    /// suspended (and not revoked)? Doors use it to answer "suspended" instead
    /// of "invalid", so an agent can wait for Resume. Only the holder of that
    /// member's own live key gets `true`: an unknown key, a revoked key and
    /// any key of a revoked membership (whose record has left `members`) all
    /// get `false`, so they stay indistinguishable from each other.
    pub fn credential_of_suspended_member(&self, token: Option<&str>) -> bool {
        let Some(token) = token else { return false };
        if !self.store.is_persistent() {
            return false;
        }
        let Some(principal) = self.store.disabled_member_principal_for_credential(token) else {
            return false;
        };
        // Revocation commits and leaves `members` under this lock: holding it,
        // a membership still listed is not one being revoked right now. The
        // record names the principal its key was bound to; the suspension is
        // read by the membership's id.
        let _lifecycle = self.member_lifecycle.lock_or_recover();
        let record = self
            .members
            .lock_or_recover()
            .values()
            .find(|member| member.principal_id.as_deref() == Some(principal.as_str()))
            .map(|member| member.id.clone());
        record.is_some_and(|id| self.store.member_suspended(&id).unwrap_or(false))
    }

    pub fn memberships(&self) -> Vec<MemberRecord> {
        self.members.lock_or_recover().values().cloned().collect()
    }

    /// The membership carrying this name, if the building knows them.
    pub fn member_by_name(&self, name: &str) -> Option<MemberRecord> {
        self.members
            .lock_or_recover()
            .values()
            .find(|m| m.name == name)
            .cloned()
    }

    /// Would a new member named `name` share it, without regard to letter
    /// case, with someone already in the Building: a live member, the Master,
    /// a Smaster, or a configured caretaker under another spelling? `@Bob`
    /// in a message wakes `bob` too, and a target of `LOCA-CARE` summons the
    /// caretaker `loca-care` (`addressed_caretakers`), so such a name is
    /// refused at every admission door. A caretaker's own exact name is how
    /// that caretaker is admitted, so it is no clash. Compared with
    /// `store::search_fold`, the fold `validate_display_name` uses.
    pub fn new_member_name_clashes(&self, name: &str) -> bool {
        let folded = crate::store::search_fold(name);
        self.caretakers
            .iter()
            .any(|caretaker| caretaker != name && crate::store::search_fold(caretaker) == folded)
            || self.name_worn_by_member_or_rank(&folded)
    }

    /// Would a new Smaster named `name` share it, without regard to letter
    /// case, with a member, the Master, another Smaster or a configured
    /// caretaker (its exact spelling included: a rank is not a caretaker)?
    pub fn rank_name_clashes(&self, name: &str) -> bool {
        let folded = crate::store::search_fold(name);
        self.caretaker_name_folds_to(&folded) || self.name_worn_by_member_or_rank(&folded)
    }

    /// Is a configured caretaker's name `folded` once folded?
    pub(in crate::hub) fn caretaker_name_folds_to(&self, folded: &str) -> bool {
        self.caretakers
            .iter()
            .any(|caretaker| crate::store::search_fold(caretaker) == folded)
    }

    /// Does a live member, the Master or a Smaster wear a name that folds to
    /// `folded`? The Master's name is known only from the stored profiles,
    /// so a memory-only Building checks members and Smasters alone.
    pub(in crate::hub) fn name_worn_by_member_or_rank(&self, folded: &str) -> bool {
        let worn = |other: &str| crate::store::search_fold(other) == folded;
        // One lock at a time: each guard ends with its own statement.
        let by_member = self
            .members
            .lock_or_recover()
            .values()
            .any(|member| worn(&member.name));
        let by_smaster = self
            .smasters
            .lock_or_recover()
            .values()
            .any(|record| worn(&record.name));
        by_member
            || by_smaster
            || self.profiles().iter().any(|profile| {
                profile.role != crate::store::BuildingRole::Member && worn(&profile.display_name)
            })
    }
}
