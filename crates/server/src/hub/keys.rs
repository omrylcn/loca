//! The key ledger and what the Master does with it.

use super::{BuildingRole, Hub, ProfileRenameError, RequestAuthority};
use crate::store::{KeyRecord, PrincipalIdentity};
use crate::sync::RecoverMutex;
use protocol::SenderType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyRevokeError {
    NotFound,
    /// Members leave through the Building directory (Suspend / Revoke), where
    /// the consequences are spelled out; admission rights simply expire.
    NotHere,
    Storage,
}

impl Hub {
    /// An unused Smaster code stops working after this long.
    pub const SMASTER_USE_MS: u64 = 7 * 24 * 60 * 60 * 1_000;

    pub fn key_ledger(&self) -> Vec<KeyRecord> {
        self.store
            .key_ledger(self.now(), Self::SMASTER_USE_MS)
            .unwrap_or_default()
    }

    /// Take back unused Smaster codes past their use-by. Called by the
    /// background sweep; a code that was used once is never touched here.
    pub fn expire_unused_smasters(&self) {
        let cutoff = self.now().saturating_sub(Self::SMASTER_USE_MS);
        for token in self.store.unused_smasters_issued_before(cutoff) {
            if self.smasters.lock_or_recover().contains_key(&token) {
                match self.revoke_smaster(&token) {
                    Ok(_) => tracing::info!("an unused Smaster code expired"),
                    Err(error) => {
                        tracing::error!(%error, "could not expire an unused Smaster code")
                    }
                }
            }
        }
    }

    /// Revoke one ledger entry by its id.
    pub fn revoke_key(&self, id: &str) -> Result<(), KeyRevokeError> {
        if id.starts_with("pl_") {
            let mut current = self.admin_pairing.lock_or_recover();
            if current.log_id.as_deref() != Some(id) {
                return Err(KeyRevokeError::NotFound);
            }
            let now = self.now();
            let _ = self.store.mark_pairing_superseded(id, now);
            *current = super::AdminPairing {
                code: Self::secure_token("pair_", 16),
                session_ttl_ms: Self::ADMIN_SESSION_TTL_MS,
                expires_at_ms: now.saturating_add(Self::ADMIN_PAIRING_TTL_MS),
                log_id: None,
            };
            return Ok(());
        }
        if id.starts_with("smid_") {
            return match self.revoke_smaster_ref(id) {
                Ok(true) => Ok(()),
                Ok(false) => Err(KeyRevokeError::NotFound),
                Err(_) => Err(KeyRevokeError::Storage),
            };
        }
        if id.starts_with("ivid_") {
            return match self.revoke_invite_ref(id) {
                Ok(true) => Ok(()),
                Ok(false) => Err(KeyRevokeError::NotFound),
                Err(_) => Err(KeyRevokeError::Storage),
            };
        }
        if id.starts_with("mbid_") || id.starts_with("adid_") {
            return Err(KeyRevokeError::NotHere);
        }
        Err(KeyRevokeError::NotFound)
    }

    /// Has this rank ever signed in? An issued but never-used Smaster code is
    /// a code, not a person, and the Building says so.
    pub fn principal_ever_used(&self, principal_id: &str) -> bool {
        self.store.principal_ever_used(principal_id)
    }

    /// The Master names a Smaster (a placeholder name, or a correction). The
    /// same rename the Smaster could do itself: history follows, open feeds
    /// relabel, titles and taken names are refused.
    pub fn rename_smaster_by_master(
        &self,
        reference: &str,
        name: &str,
    ) -> Result<PrincipalIdentity, ProfileRenameError> {
        let token = self
            .smasters
            .lock_or_recover()
            .keys()
            .find(|t| t.as_str() == reference || self.smaster_management_id(t) == reference)
            .cloned()
            .ok_or(ProfileRenameError::AuthorityRequired)?;
        let current = self.smaster_name(Some(&token)).unwrap_or_default();
        let actor = RequestAuthority {
            principal_id: Some(crate::store::hashed_id("pr_", &format!("smaster:{token}"))),
            display_name: Some(current),
            kind: Some(SenderType::User),
            building_role: Some(BuildingRole::Smaster),
        };
        self.rename_profile(&actor, name)
    }
}
