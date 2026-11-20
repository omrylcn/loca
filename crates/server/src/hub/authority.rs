//! Who holds rank: the Master's key and sessions, Smasters, profile credentials and preferences.

use super::*;

impl Hub {
    /// True if `token` authorizes admin actions. When no token is configured,
    /// any request is treated as admin (dev mode).
    /// May this token act — issue invites, run a loca, change settings?
    /// True for the master and for every smaster: a smaster does everything a
    /// master does.
    pub fn is_admin(&self, token: Option<&str>) -> bool {
        self.is_master(token) || self.smaster_name(token).is_some()
    }

    /// True when no ADMIN_TOKEN is configured: a dev server with no host, where
    /// everyone present acts as the operator.
    pub fn admin_open(&self) -> bool {
        self.admin_token.is_empty()
    }

    /// Is this the master themselves? The master has the last word, so this is
    /// what guards the few things a smaster must not do: undoing the master's
    /// own decisions, and minting or revoking smasters.
    pub fn is_master(&self, token: Option<&str>) -> bool {
        self.admin_token.is_empty() || token == Some(self.admin_token.as_str())
    }

    /// Resolve authority from server-held credentials/session state. Client
    /// names and request payload roles are deliberately absent from this API.
    pub fn resolve_authority(
        &self,
        admin_token: Option<&str>,
        session_token: Option<&str>,
    ) -> RequestAuthority {
        if let Some(session) = self.session_identity(session_token) {
            // An invite proves identity only inside one room. Never turn that
            // proof into Building/profile authority (including key issuance).
            if session.loca.is_some() {
                return RequestAuthority {
                    principal_id: None,
                    display_name: Some(session.name),
                    kind: Some(session.kind),
                    building_role: Some(BuildingRole::Member),
                };
            }
            if let Some(principal_id) = session.principal_id.as_deref() {
                return authority_from_principal(self.store.active_principal(principal_id))
                    .unwrap_or(RequestAuthority {
                        principal_id: None,
                        display_name: None,
                        kind: None,
                        building_role: None,
                    });
            }
            return RequestAuthority {
                principal_id: session.principal_id,
                display_name: Some(session.name),
                kind: Some(session.kind),
                building_role: Some(session.building_role),
            };
        }
        let Some(token) = admin_token.filter(|token| !token.is_empty()) else {
            return if self.admin_token.is_empty() {
                RequestAuthority {
                    principal_id: None,
                    display_name: Some("operator".into()),
                    kind: Some(SenderType::User),
                    building_role: Some(BuildingRole::Master),
                }
            } else {
                RequestAuthority {
                    principal_id: None,
                    display_name: None,
                    kind: None,
                    building_role: None,
                }
            };
        };
        if let Some(authority) =
            authority_from_principal(self.store.principal_for_credential(token))
        {
            return authority;
        }
        if self.is_master(Some(token)) {
            return authority_from_principal(self.store.principal_for_credential(token)).unwrap_or(
                RequestAuthority {
                    principal_id: None,
                    display_name: Some("operator".into()),
                    kind: Some(SenderType::User),
                    building_role: Some(BuildingRole::Master),
                },
            );
        }
        if self.smaster_name(Some(token)).is_some() {
            return authority_from_principal(self.store.principal_for_credential(token)).unwrap_or(
                RequestAuthority {
                    principal_id: None,
                    display_name: self.smaster_name(Some(token)),
                    kind: Some(SenderType::User),
                    building_role: Some(BuildingRole::Smaster),
                },
            );
        }
        RequestAuthority {
            principal_id: None,
            display_name: None,
            kind: None,
            building_role: None,
        }
    }

    pub fn admin_session_grant(
        &self,
        credential: Option<&str>,
        ttl_ms: u64,
    ) -> Option<AdminSessionGrant> {
        let authority = self.resolve_authority(credential, None);
        (authority.principal_id.is_some() || authority.is_building_admin()).then(|| {
            AdminSessionGrant {
                ttl_ms,
                credential_id: credential
                    .and_then(|token| self.store.credential_id_for_secret(token)),
                authority,
            }
        })
    }

    pub fn master_pairing_grant(&self, ttl_ms: u64) -> AdminSessionGrant {
        let authority = authority_from_principal(self.store.active_master_principal()).unwrap_or(
            RequestAuthority {
                principal_id: None,
                display_name: Some("operator".into()),
                kind: Some(SenderType::User),
                building_role: Some(BuildingRole::Master),
            },
        );
        AdminSessionGrant {
            ttl_ms,
            credential_id: self.store.credential_id_for_secret(&self.admin_token),
            authority,
        }
    }

    pub fn credentials_for(&self, principal_id: &str) -> Vec<CredentialSummary> {
        self.store.list_credentials(principal_id)
    }

    /// The principal's own appearance preferences (defaults when unsaved).
    pub fn preferences_for(&self, principal_id: &str) -> crate::store::Preferences {
        self.store.principal_preferences(principal_id)
    }

    /// Validate and save a partial preference update for this principal.
    pub fn update_preferences(
        &self,
        principal_id: &str,
        patch: &serde_json::Value,
    ) -> Result<crate::store::Preferences, crate::store::PreferenceError> {
        self.store
            .merge_principal_preferences(principal_id, patch, self.now())
    }

    pub fn credential_id_for_request(
        &self,
        credential: Option<&str>,
        session: Option<&str>,
    ) -> Option<String> {
        session
            .and_then(|secret| self.store.credential_id_for_session(secret, self.now()))
            .or_else(|| credential.and_then(|secret| self.store.credential_id_for_secret(secret)))
    }

    pub fn credential_is_revoked(&self, credential: &str) -> bool {
        self.store.credential_is_revoked(credential)
    }

    /// The id (`cr_`) of the live credential this secret is, if any.
    pub fn credential_id_for_secret(&self, credential: &str) -> Option<String> {
        self.store.credential_id_for_secret(credential)
    }

    pub fn create_profile_credential(
        &self,
        principal_id: &str,
        label: &str,
    ) -> Result<(CredentialSummary, String), CredentialError> {
        let secret = Self::secure_token("ak_", 32);
        let summary = self
            .store
            .create_credential(principal_id, label, &secret, self.now())?;
        Ok((summary, secret))
    }

    pub fn revoke_profile_credential(
        &self,
        principal_id: &str,
        credential_id: &str,
    ) -> Result<(), CredentialError> {
        let legacy = self
            .store
            .legacy_credential_source(principal_id, credential_id);
        self.store
            .revoke_credential(principal_id, credential_id, self.now())?;
        self.sessions.lock_or_recover().retain(|session_hash, _| {
            !self
                .store
                .session_uses_credential_by_hash(session_hash, credential_id)
        });
        // Door B: the Lobby sockets opened with this credential close. Its
        // reconnect is refused at the handshake (the credential is revoked).
        let _ = self.lobby_tx.send(LobbyEvent::CredentialRevoked {
            credential_id: credential_id.to_string(),
        });
        // v2 authority is DB-backed, but a legacy Smaster key also lives in
        // the in-memory compatibility map. Retire that fallback only after the
        // credential transaction commits, so "revoked" cannot still mean
        // "Smaster through the old path".
        if let Some((role, hash)) = legacy {
            if role == "smaster" {
                self.smasters.lock_or_recover().remove(&hash);
            }
        }
        Ok(())
    }

    /// The live Smaster a presented code belongs to. The code is hashed and
    /// looked up by its hash; nothing else (the hash itself, a `pr_` or a
    /// `smid_`) is a code.
    pub fn smaster_for_code(&self, code: Option<&str>) -> Option<crate::store::SmasterRecord> {
        let code = code.filter(|code| !code.is_empty())?;
        self.smasters
            .lock_or_recover()
            .get(&crate::store::secret_hash(code))
            .cloned()
    }

    /// The smaster behind this token, if it is a live one.
    pub fn smaster_name(&self, token: Option<&str>) -> Option<String> {
        self.smaster_for_code(token).map(|record| record.name)
    }

    /// Mint a second master. Only the master may call this — the caller checks.
    /// The code is returned once, to the response that hands it over. A name
    /// someone already wears in any letter case (`rank_name_clashes`) is
    /// refused; the check and the write happen under `name_claims`.
    pub fn add_smaster(&self, name: &str) -> Result<String, NameClaimError> {
        let _claim = self.name_claims.lock_or_recover();
        if self.rank_name_clashes(name) {
            return Err(NameClaimError::Worn);
        }
        let code = self.new_invite_token().replacen("dv_", "sm_", 1);
        let now = (self.now_ms)();
        let record = self.store.add_smaster(&code, name, now)?;
        self.smasters
            .lock_or_recover()
            .insert(record.secret_hash.clone(), record);
        Ok(code)
    }

    /// Take one back, by its code's hash. The row survives, so "who could
    /// act, and when" stays answerable.
    pub(in crate::hub) fn revoke_smaster_hash(&self, secret_hash: &str) -> rusqlite::Result<bool> {
        let Some(record) = self.smasters.lock_or_recover().get(secret_hash).cloned() else {
            return Ok(false);
        };
        self.store.revoke_smaster(secret_hash, (self.now_ms)())?;
        self.smasters.lock_or_recover().remove(secret_hash);
        // A group whose founder this was now belongs to the Master.
        self.pass_groups_to_master(&record.principal_id);
        Ok(true)
    }

    /// The live Smaster a management reference names: its `smid_`, or the
    /// Smaster's own code presented by its holder (hashed, then compared).
    pub(in crate::hub) fn smaster_by_reference(
        &self,
        reference: &str,
    ) -> Option<crate::store::SmasterRecord> {
        if reference.is_empty() {
            return None;
        }
        let hash = crate::store::secret_hash(reference);
        let smasters = self.smasters.lock_or_recover();
        smasters.get(&hash).cloned().or_else(|| {
            smasters
                .values()
                .find(|record| record.management_id == reference)
                .cloned()
        })
    }

    /// Revoke the Smaster a management reference names (`smid_…`, or the
    /// code itself).
    pub fn revoke_smaster_ref(&self, reference: &str) -> rusqlite::Result<bool> {
        match self.smaster_by_reference(reference) {
            Some(record) => self.revoke_smaster_hash(&record.secret_hash),
            None => Ok(false),
        }
    }

    /// Live Smasters. Never a code: a record carries its hash and ids.
    pub fn smasters(&self) -> Vec<crate::store::SmasterRecord> {
        self.smasters.lock_or_recover().values().cloned().collect()
    }
}
