//! Claiming a membership key with an invite (`POST /membership/claim`).
//!
//! An invite proves which member holds it. Claiming with it mints a key of the
//! holder's own for that member's principal: a new credential, labelled with
//! the loca the invite was for, shown and revocable under Profile -> Keys.
//! The member's original key is never read back out, and nothing existing is
//! revoked or replaced. The rules (one key per set-up, a delivery copy kept
//! until the set-up's ACK or 24 hours, at most `MEMBERSHIP_CLAIMS_PER_INVITE`
//! set-ups per invite) live in `store/claims.rs`; a memory-only Building keeps
//! the same rules here, in memory.

use super::{secret_hash, HashMap, Hub, MemberRecord, RecoverMutex};
use crate::store::{MemberClaim, CLAIM_DELIVERY_WINDOW_MS, MEMBERSHIP_CLAIMS_PER_INVITE};

/// Why a claim was refused before anything was looked up for a set-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimRefusal {
    /// No live invite: unknown, revoked, or its member is suspended.
    NoInvite,
    /// A membership key was presented. A key cannot mint another key.
    MembershipKey,
    /// The claim could not be read or written.
    Storage,
}

/// What a persistent Building keeps in `credentials` and `membership_claims`,
/// for a Building that has no database.
#[derive(Default)]
pub(crate) struct MemoryClaims {
    /// `secret_hash` of a claimed key -> the member's id, which is what the
    /// in-memory member map is keyed by.
    keys: HashMap<String, String>,
    /// `(invite id, claim hash)` -> that set-up.
    setups: HashMap<(String, String), MemorySetup>,
    /// Invite id -> how many set-ups it has.
    counts: HashMap<String, i64>,
}

struct MemorySetup {
    member: String,
    credential_id: String,
    copy: Option<String>,
    claimed_at: u64,
    acked: bool,
}

impl MemoryClaims {
    pub(crate) fn member_for_key(&self, key: &str) -> Option<String> {
        self.keys.get(&secret_hash(key)).cloned()
    }

    /// The membership ended: its claimed keys stop working and no copy is
    /// held for it any more.
    pub(crate) fn forget_member(&mut self, member: &str) {
        self.keys.retain(|_, owner| owner != member);
        for setup in self.setups.values_mut() {
            if setup.member == member {
                setup.copy = None;
            }
        }
    }

    fn expire(&mut self, now: u64) -> usize {
        let mut cleared = 0;
        for setup in self.setups.values_mut() {
            if !setup.acked
                && setup.copy.is_some()
                && now >= setup.claimed_at.saturating_add(CLAIM_DELIVERY_WINDOW_MS)
            {
                setup.copy = None;
                cleared += 1;
            }
        }
        cleared
    }

    fn claim(
        &mut self,
        invite_id: &str,
        claim_hash: Option<&str>,
        member: &str,
        key: &str,
        now: u64,
    ) -> MemberClaim {
        if let Some(claim_hash) = claim_hash {
            if let Some(setup) = self
                .setups
                .get_mut(&(invite_id.to_string(), claim_hash.to_string()))
            {
                if setup.acked {
                    return MemberClaim::AlreadyDelivered;
                }
                if now >= setup.claimed_at.saturating_add(CLAIM_DELIVERY_WINDOW_MS) {
                    setup.copy = None;
                }
                return match &setup.copy {
                    Some(copy) => MemberClaim::Delivered {
                        key: copy.clone(),
                        credential_id: setup.credential_id.clone(),
                        fresh: false,
                    },
                    None => MemberClaim::DeliveryEnded,
                };
            }
        }
        let count = self.counts.entry(invite_id.to_string()).or_insert(0);
        if *count >= MEMBERSHIP_CLAIMS_PER_INVITE {
            return MemberClaim::CapReached;
        }
        *count += 1;
        let credential_id = crate::store::hashed_id("cr_", &format!("credential:{key}"));
        self.keys.insert(secret_hash(key), member.to_string());
        if let Some(claim_hash) = claim_hash {
            self.setups.insert(
                (invite_id.to_string(), claim_hash.to_string()),
                MemorySetup {
                    member: member.to_string(),
                    credential_id: credential_id.clone(),
                    copy: Some(key.to_string()),
                    claimed_at: now,
                    acked: false,
                },
            );
        }
        MemberClaim::Delivered {
            key: key.to_string(),
            credential_id,
            fresh: true,
        }
    }

    fn ack(&mut self, invite_id: &str, claim_hash: &str) -> Option<bool> {
        let setup = self
            .setups
            .get_mut(&(invite_id.to_string(), claim_hash.to_string()))?;
        if setup.acked {
            return Some(false);
        }
        setup.acked = true;
        setup.copy = None;
        Some(true)
    }
}

impl Hub {
    /// Claim a membership key with the invite `token`, for the set-up that
    /// named itself `claim_id` (`None`: an older kit, every call a new
    /// set-up). Returns the answer and the member the invite seats.
    pub fn claim_membership_key(
        &self,
        token: &str,
        claim_id: Option<&str>,
    ) -> Result<(MemberClaim, MemberRecord), ClaimRefusal> {
        // Serialized with the membership's lifecycle (revoke, suspend), so a
        // key is never minted for a member whose revocation is under way.
        let _lifecycle = self.member_lifecycle.lock_or_recover();
        let Some(invite) = self.invite_by_token(token) else {
            return Err(if self.member_for_credential(Some(token)).is_some() {
                ClaimRefusal::MembershipKey
            } else {
                ClaimRefusal::NoInvite
            });
        };
        let member = self
            .member_by_id(&invite.member_id)
            .ok_or(ClaimRefusal::NoInvite)?;
        let invite_id = invite.id.clone();
        let claim_hash = claim_id.map(secret_hash);
        let key = Self::secure_token("mb_", 32);
        let now = self.now();
        if !self.store.is_persistent() {
            let outcome = self.memory_claims.lock_or_recover().claim(
                &invite_id,
                claim_hash.as_deref(),
                &member.id,
                &key,
                now,
            );
            return Ok((outcome, member));
        }
        let principal = member.principal_id.clone().ok_or(ClaimRefusal::Storage)?;
        let label = format!("Membership key (claimed with davet for #{})", invite.room);
        self.store
            .claim_member_key(
                &invite_id,
                claim_hash.as_deref(),
                &principal,
                &label,
                &key,
                now,
            )
            .map(|outcome| (outcome, member))
            .map_err(|error| {
                tracing::error!(%error, "could not claim a membership key");
                ClaimRefusal::Storage
            })
    }

    /// The set-up `claim_id` verified and stored the key it claimed with the
    /// invite `token`: end its delivery. The invite need not be live any more
    /// (only its id is used), and an ACK only ever clears a copy.
    /// `Some(true)` = acknowledged now, `Some(false)` = already acknowledged,
    /// `None` = no such set-up.
    pub fn ack_membership_claim(
        &self,
        token: &str,
        claim_id: &str,
    ) -> rusqlite::Result<Option<bool>> {
        let invite_id = self.invite_management_id(token);
        let claim_hash = secret_hash(claim_id);
        if !self.store.is_persistent() {
            return Ok(self
                .memory_claims
                .lock_or_recover()
                .ack(&invite_id, &claim_hash));
        }
        self.store
            .ack_member_claim(&invite_id, &claim_hash, self.now())
            .inspect_err(
                |error| tracing::error!(%error, "could not acknowledge a membership claim"),
            )
    }

    /// The memory-only share of the 15-second sweep: clear claimed-key copies
    /// past their deadline. Returns how many were cleared.
    pub(super) fn expire_memory_claim_copies(&self) -> usize {
        self.memory_claims.lock_or_recover().expire(self.now())
    }
}
