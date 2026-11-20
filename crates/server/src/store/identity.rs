use super::{
    current_time_ms, hashed_id, params, parse_sender_type, secret_hash, sender_type_str,
    BuildingRole, Connection, CredentialError, CredentialSummary, ModerationRecord,
    OptionalExtension, PrincipalIdentity, SenderType, Store, SESSIONS_PER_CREDENTIAL,
};

impl Store {
    /// Persist the access cutoff before the Hub disconnects live sockets.
    /// Credentials and principal IDs survive; existing sessions and invites do
    /// not. `member_id` is the membership's id (`mbid_`).
    pub fn suspend_member(&self, member_id: &str, reason: &str, at: u64) -> rusqlite::Result<bool> {
        let Some(mut c) = self.conn() else {
            return Err(rusqlite::Error::InvalidQuery);
        };
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let principal: Option<String> = tx
            .query_row(
                "SELECT p.id FROM members m JOIN principals p ON p.id = m.principal_id
             WHERE m.id = ?1 AND m.revoked_at IS NULL AND p.building_role = 'member'",
                params![member_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(principal) = principal else {
            return Ok(false);
        };
        tx.execute("INSERT OR IGNORE INTO member_suspensions(member_id, suspended_at, reason) VALUES (?1, ?2, ?3)", params![member_id, at, reason])?;
        tx.execute(
            "UPDATE principals SET disabled_at = ?2 WHERE id = ?1 AND disabled_at IS NULL",
            params![principal, at],
        )?;
        tx.execute("UPDATE principal_sessions SET revoked_at = ?2 WHERE principal_id = ?1 AND revoked_at IS NULL", params![principal, at])?;
        tx.execute(
            "UPDATE invites SET revoked_at = ?2 WHERE member_id = ?1 AND revoked_at IS NULL",
            params![member_id, at],
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub fn resume_member(&self, member_id: &str) -> rusqlite::Result<bool> {
        let Some(mut c) = self.conn() else {
            return Err(rusqlite::Error::InvalidQuery);
        };
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "DELETE FROM member_suspensions WHERE member_id = ?1
             AND EXISTS(SELECT 1 FROM members WHERE id = ?1 AND revoked_at IS NULL)",
            params![member_id],
        )?;
        if changed == 0 {
            return Ok(false);
        }
        tx.execute(
            "UPDATE principals SET disabled_at = NULL WHERE id IN
            (SELECT principal_id FROM members WHERE id = ?1) AND building_role = 'member'",
            params![member_id],
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub fn member_suspended(&self, member_id: &str) -> rusqlite::Result<bool> {
        let Some(c) = self.conn() else {
            return Ok(false);
        };
        c.query_row(
            "SELECT EXISTS(SELECT 1 FROM member_suspensions WHERE member_id = ?1)",
            params![member_id],
            |row| row.get(0),
        )
    }

    pub fn principal_for_credential(&self, credential: &str) -> Option<PrincipalIdentity> {
        if credential.is_empty() {
            return None;
        }
        let c = self.conn()?;
        let identity = c
            .query_row(
                "SELECT p.id, p.display_name, p.kind, p.building_role
             FROM credentials c
             JOIN principals p ON p.id = c.principal_id
             WHERE c.secret_hash = ?1
               AND c.revoked_at IS NULL
               AND p.disabled_at IS NULL",
                params![secret_hash(credential)],
                |row| {
                    let kind: String = row.get(2)?;
                    let role: String = row.get(3)?;
                    Ok(PrincipalIdentity {
                        id: row.get(0)?,
                        display_name: row.get(1)?,
                        kind: parse_sender_type(&kind),
                        role: parse_building_role(&role),
                    })
                },
            )
            .optional()
            .ok()
            .flatten();
        if identity.is_some() {
            let _ = c.execute(
                "UPDATE credentials SET last_used_at = ?2
                 WHERE secret_hash = ?1 AND revoked_at IS NULL",
                params![secret_hash(credential), current_time_ms()],
            );
        }
        identity
    }

    /// The Member principal behind a live (unrevoked) credential whose
    /// principal is disabled, which is what suspension and revocation both
    /// leave. Telling the two apart is the caller's job (`Hub`), which also
    /// requires a live, suspended membership record.
    pub fn disabled_member_principal_for_credential(&self, credential: &str) -> Option<String> {
        if credential.is_empty() {
            return None;
        }
        let c = self.conn()?;
        c.query_row(
            "SELECT p.id FROM credentials c
             JOIN principals p ON p.id = c.principal_id
             WHERE c.secret_hash = ?1
               AND c.revoked_at IS NULL
               AND p.disabled_at IS NOT NULL
               AND p.building_role = 'member'",
            params![secret_hash(credential)],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten()
    }

    pub fn credential_id_for_secret(&self, credential: &str) -> Option<String> {
        let c = self.conn()?;
        c.query_row(
            "SELECT id FROM credentials
             WHERE secret_hash = ?1 AND revoked_at IS NULL",
            params![secret_hash(credential)],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten()
    }

    pub fn credential_is_revoked(&self, credential: &str) -> bool {
        let Some(c) = self.conn() else {
            return false;
        };
        c.query_row(
            "SELECT revoked_at IS NOT NULL FROM credentials WHERE secret_hash = ?1",
            params![secret_hash(credential)],
            |row| row.get::<_, i64>(0),
        )
        .map(|revoked| revoked != 0)
        .unwrap_or(false)
    }

    /// The legacy registry a v2 credential came from, as `(role, secret_hash)`
    /// (`smaster` or `member`). The hash is the key the in-memory compatibility
    /// map is held under; Hub uses it only to retire that fallback after the
    /// revocation commits. No code is read back to answer this.
    pub fn legacy_credential_source(
        &self,
        principal_id: &str,
        credential_id: &str,
    ) -> Option<(String, String)> {
        let c = self.conn()?;
        let source: String = c
            .query_row(
                "SELECT legacy_source FROM credentials
                 WHERE id = ?1 AND principal_id = ?2 AND revoked_at IS NULL
                   AND legacy_source IS NOT NULL",
                params![credential_id, principal_id],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten()?;
        let (role, hash) = source.split_once(':')?;
        matches!(role, "member" | "smaster").then(|| (role.to_string(), hash.to_string()))
    }

    pub fn credential_id_for_session(&self, session_secret: &str, now: u64) -> Option<String> {
        let c = self.conn()?;
        c.query_row(
            "SELECT s.credential_id FROM principal_sessions s
             JOIN credentials c ON c.id = s.credential_id AND c.principal_id = s.principal_id
             JOIN principals p ON p.id = s.principal_id
             WHERE s.id = ?1 AND s.revoked_at IS NULL
               AND s.expires_at > ?2
               AND c.revoked_at IS NULL AND p.disabled_at IS NULL",
            params![secret_hash(session_secret), now],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten()
    }

    /// Was the session whose token hashes to `session_hash` taken with this
    /// credential? Given the hash the Hub keys its sessions by, not a token.
    pub fn session_uses_credential_by_hash(&self, session_hash: &str, credential_id: &str) -> bool {
        let Some(c) = self.conn() else {
            return false;
        };
        c.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM principal_sessions
                WHERE id = ?1 AND credential_id = ?2
             )",
            params![session_hash, credential_id],
            |row| row.get::<_, i64>(0),
        )
        .map(|found| found != 0)
        .unwrap_or(false)
    }

    pub fn list_credentials(&self, principal_id: &str) -> Vec<CredentialSummary> {
        let Some(c) = self.conn() else {
            return Vec::new();
        };
        let mut output = Vec::new();
        if let Ok(mut stmt) = c.prepare(
            "SELECT id, label, created_at, last_used_at, revoked_at,
                    COALESCE(legacy_source LIKE 'admin:%', 0)
             FROM credentials WHERE principal_id = ?1
             ORDER BY created_at, id",
        ) {
            if let Ok(rows) = stmt.query_map(params![principal_id], |row| {
                Ok(CredentialSummary {
                    id: row.get(0)?,
                    label: row.get(1)?,
                    created_at: row.get(2)?,
                    last_used_at: row.get(3)?,
                    revoked_at: row.get(4)?,
                    root_recovery: row.get::<_, i64>(5)? != 0,
                })
            }) {
                output.extend(rows.flatten());
            }
        }
        output
    }

    pub fn create_credential(
        &self,
        principal_id: &str,
        label: &str,
        secret: &str,
        at: u64,
    ) -> Result<CredentialSummary, CredentialError> {
        let Some(c) = self.conn() else {
            return Err(CredentialError::Storage);
        };
        let id = hashed_id("cr_", &format!("credential:{secret}"));
        c.execute(
            "INSERT INTO credentials
             (id, principal_id, label, secret_hash, created_at, last_used_at, revoked_at, legacy_source)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL, NULL)",
            params![id, principal_id, label, secret_hash(secret), at],
        )
        .map_err(|_| CredentialError::Storage)?;
        Ok(CredentialSummary {
            id,
            label: label.to_string(),
            created_at: at,
            last_used_at: None,
            revoked_at: None,
            root_recovery: false,
        })
    }

    pub fn revoke_credential(
        &self,
        principal_id: &str,
        credential_id: &str,
        at: u64,
    ) -> Result<(), CredentialError> {
        let Some(mut c) = self.conn() else {
            return Err(CredentialError::Storage);
        };
        let tx = c.transaction().map_err(|_| CredentialError::Storage)?;
        let row: Option<(Option<String>, Option<u64>)> = tx
            .query_row(
                "SELECT legacy_source, revoked_at FROM credentials
                 WHERE id = ?1 AND principal_id = ?2",
                params![credential_id, principal_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|_| CredentialError::Storage)?;
        let Some((legacy_source, revoked_at)) = row else {
            return Err(CredentialError::NotFound);
        };
        if legacy_source
            .as_deref()
            .is_some_and(|source| source.starts_with("admin:"))
        {
            return Err(CredentialError::RootRecovery);
        }
        if revoked_at.is_some() {
            return Err(CredentialError::NotFound);
        }
        tx.execute(
            "UPDATE credentials SET revoked_at = ?3
             WHERE id = ?1 AND principal_id = ?2 AND revoked_at IS NULL",
            params![credential_id, principal_id, at],
        )
        .map_err(|_| CredentialError::Storage)?;
        tx.execute(
            "UPDATE principal_sessions SET revoked_at = ?2
             WHERE credential_id = ?1 AND revoked_at IS NULL",
            params![credential_id, at],
        )
        .map_err(|_| CredentialError::Storage)?;
        if let Some(hash) = legacy_source
            .as_deref()
            .and_then(|source| source.strip_prefix("smaster:"))
        {
            tx.execute(
                "UPDATE smasters SET revoked_at = ?2
                 WHERE secret_hash = ?1 AND revoked_at IS NULL",
                params![hash, at],
            )
            .map_err(|_| CredentialError::Storage)?;
        }
        tx.commit().map_err(|_| CredentialError::Storage)
    }

    /// Record a session taken with `credential_id`, and end that credential's
    /// oldest live sessions beyond [`SESSIONS_PER_CREDENTIAL`]. The count and
    /// the eviction happen in the insert's own write transaction, so takes
    /// that race cannot leave more than the cap alive. Returns the hashes
    /// (`principal_sessions.id`, which is what the Hub keys its sessions by)
    /// of the sessions it ended, for the caller to drop from memory.
    pub fn save_principal_session(
        &self,
        session_secret: &str,
        principal_id: &str,
        credential_id: &str,
        created_at: u64,
        expires_at: u64,
    ) -> rusqlite::Result<Vec<String>> {
        let Some(mut c) = self.conn() else {
            return Ok(Vec::new());
        };
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT OR REPLACE INTO principal_sessions
             (id, principal_id, credential_id, created_at, expires_at, revoked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
            params![
                secret_hash(session_secret),
                principal_id,
                credential_id,
                created_at,
                expires_at
            ],
        )?;
        // Newest first; everything after the first SESSIONS_PER_CREDENTIAL is
        // ended. `rowid` orders sessions taken in the same millisecond by when
        // they were written.
        let ended: Vec<String> = tx
            .prepare(
                "SELECT id FROM principal_sessions
                 WHERE credential_id = ?1 AND revoked_at IS NULL AND expires_at > ?2
                 ORDER BY created_at DESC, rowid DESC
                 LIMIT -1 OFFSET ?3",
            )?
            .query_map(
                params![credential_id, created_at, SESSIONS_PER_CREDENTIAL as i64],
                |row| row.get(0),
            )?
            .collect::<rusqlite::Result<_>>()?;
        for id in &ended {
            tx.execute(
                "UPDATE principal_sessions SET revoked_at = ?2 WHERE id = ?1",
                params![id, created_at],
            )?;
            tx.execute(
                "DELETE FROM admin_sessions WHERE secret_hash = ?1",
                params![id],
            )?;
        }
        tx.commit()?;
        Ok(ended)
    }

    /// Delete every stored session that can no longer be presented: expired or
    /// ended. Returns how many session rows went. A storage error is returned
    /// as one, never as "nothing to sweep".
    pub fn sweep_sessions(&self, now: u64) -> rusqlite::Result<usize> {
        let Some(c) = self.conn() else {
            return Ok(0);
        };
        sweep_session_rows(&c, now)
    }

    /// Is the principal session whose token hashes to `session_hash` live?
    pub fn principal_session_active_by_hash(&self, session_hash: &str, now: u64) -> bool {
        let Some(c) = self.conn() else { return true };
        c.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM principal_sessions s
                JOIN credentials c ON c.id = s.credential_id AND c.principal_id = s.principal_id
                JOIN principals p ON p.id = s.principal_id
                WHERE s.id = ?1 AND s.revoked_at IS NULL
                  AND s.expires_at > ?2
                  AND c.revoked_at IS NULL AND p.disabled_at IS NULL
             )",
            params![session_hash, now],
            |row| row.get::<_, i64>(0),
        )
        .map(|exists| exists != 0)
        .unwrap_or(false)
    }

    /// Restore authority from the credential binding, never from a legacy
    /// display-name row or the server's current root credential. Given the
    /// session token's hash (`principal_sessions.id`), as a restart has it.
    pub fn principal_for_session_by_hash(
        &self,
        session_hash: &str,
        now: u64,
    ) -> Option<PrincipalIdentity> {
        let c = self.conn()?;
        c.query_row(
            "SELECT p.id, p.display_name, p.kind, p.building_role
             FROM principal_sessions s
             JOIN credentials c ON c.id = s.credential_id AND c.principal_id = s.principal_id
             JOIN principals p ON p.id = s.principal_id
             WHERE s.id = ?1 AND s.revoked_at IS NULL
               AND s.expires_at > ?2
               AND c.revoked_at IS NULL AND p.disabled_at IS NULL",
            params![session_hash, now],
            |row| {
                let kind: String = row.get(2)?;
                let role: String = row.get(3)?;
                Ok(PrincipalIdentity {
                    id: row.get(0)?,
                    display_name: row.get(1)?,
                    kind: parse_sender_type(&kind),
                    role: parse_building_role(&role),
                })
            },
        )
        .optional()
        .ok()
        .flatten()
    }

    pub fn revoke_principal_session_by_hash(
        &self,
        session_hash: &str,
        at: u64,
    ) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else { return Ok(()) };
        c.execute(
            "UPDATE principal_sessions SET revoked_at = ?2
             WHERE id = ?1 AND revoked_at IS NULL",
            params![session_hash, at],
        )
        .map(|_| ())
    }

    pub fn active_master_principal(&self) -> Option<PrincipalIdentity> {
        let c = self.conn()?;
        c.query_row(
            "SELECT id, display_name, kind, building_role FROM principals
             WHERE building_role = 'master' AND disabled_at IS NULL LIMIT 1",
            [],
            |row| {
                let kind: String = row.get(2)?;
                Ok(PrincipalIdentity {
                    id: row.get(0)?,
                    display_name: row.get(1)?,
                    kind: parse_sender_type(&kind),
                    role: BuildingRole::Master,
                })
            },
        )
        .optional()
        .ok()
        .flatten()
    }

    pub fn active_principal(&self, principal_id: &str) -> Option<PrincipalIdentity> {
        let c = self.conn()?;
        c.query_row(
            "SELECT id, display_name, kind, building_role FROM principals
             WHERE id = ?1 AND disabled_at IS NULL",
            params![principal_id],
            |row| {
                let kind: String = row.get(2)?;
                let role: String = row.get(3)?;
                Ok(PrincipalIdentity {
                    id: row.get(0)?,
                    display_name: row.get(1)?,
                    kind: parse_sender_type(&kind),
                    role: parse_building_role(&role),
                })
            },
        )
        .optional()
        .ok()
        .flatten()
    }

    pub fn active_principals(&self) -> Vec<PrincipalIdentity> {
        let Some(c) = self.conn() else {
            return Vec::new();
        };
        let Ok(mut statement) = c.prepare(
            "SELECT id, display_name, kind, building_role FROM principals
             WHERE disabled_at IS NULL
             ORDER BY CASE building_role WHEN 'master' THEN 0 WHEN 'smaster' THEN 1 ELSE 2 END,
                      display_name, id",
        ) else {
            return Vec::new();
        };
        let Ok(rows) = statement.query_map([], |row| {
            let kind: String = row.get(2)?;
            let role: String = row.get(3)?;
            Ok(PrincipalIdentity {
                id: row.get(0)?,
                display_name: row.get(1)?,
                kind: parse_sender_type(&kind),
                role: parse_building_role(&role),
            })
        }) else {
            return Vec::new();
        };
        rows.flatten().collect()
    }

    pub fn active_principals_named(&self, name: &str) -> rusqlite::Result<Vec<PrincipalIdentity>> {
        let Some(c) = self.conn() else {
            return Ok(Vec::new());
        };
        let mut statement = c.prepare(
            "SELECT id, display_name, kind, building_role FROM principals
             WHERE display_name = ?1 AND disabled_at IS NULL ORDER BY id",
        )?;
        let rows = statement.query_map(params![name], |row| {
            let kind: String = row.get(2)?;
            let role: String = row.get(3)?;
            Ok(PrincipalIdentity {
                id: row.get(0)?,
                display_name: row.get(1)?,
                kind: parse_sender_type(&kind),
                role: parse_building_role(&role),
            })
        })?;
        rows.collect()
    }

    /// Every active human principal carrying this display label. Migration
    /// callers must require exactly one result: a display name is a label, not
    /// an identity, so an ambiguous label can never grant room authority.
    pub fn active_human_principals_named(&self, display_name: &str) -> Vec<PrincipalIdentity> {
        let Some(c) = self.conn() else {
            return Vec::new();
        };
        let Ok(mut statement) = c.prepare(
            "SELECT id, display_name, kind, building_role FROM principals
             WHERE display_name = ?1 AND kind = 'human' AND disabled_at IS NULL
             ORDER BY id",
        ) else {
            return Vec::new();
        };
        let Ok(rows) = statement.query_map(params![display_name], |row| {
            let kind: String = row.get(2)?;
            let role: String = row.get(3)?;
            Ok(PrincipalIdentity {
                id: row.get(0)?,
                display_name: row.get(1)?,
                kind: parse_sender_type(&kind),
                role: parse_building_role(&role),
            })
        }) else {
            return Vec::new();
        };
        rows.flatten().collect()
    }

    /// Ensure the Building has its one logical Master principal. Re-running
    /// this with the same root credential is a no-op; a credential rotation
    /// replaces the recovery proof for the same Master, revoking the previous
    /// recovery credential and its derived sessions. Device keys are retained.
    pub fn ensure_master_principal(
        &self,
        root_credential: &str,
        display_name: &str,
        at: u64,
    ) -> rusqlite::Result<()> {
        if root_credential.is_empty() {
            return Ok(());
        }
        let Some(mut c) = self.conn() else {
            return Ok(());
        };
        let tx = c.transaction()?;
        let master_id: Option<String> = tx
            .query_row(
                "SELECT id FROM principals
                 WHERE building_role = 'master' AND disabled_at IS NULL LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let principal_id =
            master_id.unwrap_or_else(|| hashed_id("pr_", &format!("master:{root_credential}")));
        tx.execute(
            "INSERT OR IGNORE INTO principals
             (id, display_name, kind, building_role, created_at, disabled_at)
             VALUES (?1, ?2, 'human', 'master', ?3, NULL)",
            params![principal_id, display_name, at],
        )?;
        let hash = secret_hash(root_credential);
        tx.execute(
            "UPDATE principal_sessions SET revoked_at = ?3
             WHERE revoked_at IS NULL AND credential_id IN (
               SELECT id FROM credentials WHERE principal_id = ?1
               AND legacy_source LIKE 'admin:%' AND secret_hash != ?2
             )",
            params![principal_id, hash, at],
        )?;
        tx.execute(
            "UPDATE credentials SET revoked_at = COALESCE(revoked_at, ?3)
             WHERE principal_id = ?1 AND legacy_source LIKE 'admin:%' AND secret_hash != ?2",
            params![principal_id, hash, at],
        )?;
        tx.execute(
            "INSERT INTO credentials
             (id, principal_id, label, secret_hash, created_at, last_used_at, revoked_at, legacy_source)
             VALUES (?1, ?2, 'Root recovery', ?3, ?4, NULL, NULL, ?5)
             ON CONFLICT(id) DO UPDATE SET revoked_at = NULL",
            params![
                hashed_id("cr_", &format!("master:{root_credential}")),
                principal_id,
                hash,
                at,
                format!("admin:{hash}")
            ],
        )?;
        tx.commit()
    }

    /// Persist a browser admin session, under the session token's hash. The
    /// token itself is never written.
    pub fn save_admin_session(
        &self,
        session_secret: &str,
        name: &str,
        kind: SenderType,
        expires_at: u64,
    ) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else { return Ok(()) };
        c.execute(
            "INSERT OR REPLACE INTO admin_sessions (secret_hash, name, kind, expires_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                secret_hash(session_secret),
                name,
                sender_type_str(kind),
                expires_at
            ],
        )
        .inspect_err(|e| tracing::error!(error = %e, "save_admin_session failed"))
        .map(|_| ())
    }
    /// End an admin session by its token's hash: forget it and revoke its
    /// principal session (whose id is the same hash).
    pub fn delete_admin_session_by_hash(&self, session_hash: &str) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else { return Ok(()) };
        c.execute(
            "DELETE FROM admin_sessions WHERE secret_hash = ?1",
            params![session_hash],
        )?;
        c.execute(
            "UPDATE principal_sessions SET revoked_at = ?2
             WHERE id = ?1 AND revoked_at IS NULL",
            params![session_hash, current_time_ms()],
        )
        .inspect_err(|e| tracing::error!(error = %e, "delete_admin_session failed"))
        .map(|_| ())
    }
    /// Restore only unexpired admin sessions. Expired rows are removed at boot
    /// so persistence never widens the authority window selected at pairing.
    /// A storage error is an error, never an empty list: the caller must not
    /// start as if nobody were signed in.
    pub fn load_admin_sessions(&self, now: u64) -> rusqlite::Result<Vec<AdminSessionRecord>> {
        let Some(c) = self.conn() else {
            return Ok(Vec::new());
        };
        c.execute(
            "DELETE FROM admin_sessions WHERE expires_at <= ?1",
            params![now],
        )?;
        let mut stmt = c.prepare(
            "SELECT secret_hash, expires_at
             FROM admin_sessions WHERE expires_at > ?1",
        )?;
        let rows = stmt.query_map(params![now], |r| {
            Ok(AdminSessionRecord {
                secret_hash: r.get(0)?,
                expires_at: r.get(1)?,
            })
        })?;
        rows.collect()
    }
    /// Bind an invite recorded before invites named their membership (an empty
    /// `member_id`) to the membership `member_id`: the start's binding of an
    /// old invite. Only the binding is written; the invite's delivery state
    /// (a copy still owed, a first use already recorded) is left as it is.
    pub fn bind_invite_member_by_hash(
        &self,
        invite_hash: &str,
        member_id: &str,
    ) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else {
            return Ok(());
        };
        c.execute(
            "UPDATE invites SET member_id = ?2 WHERE secret_hash = ?1",
            params![invite_hash, member_id],
        )
        .inspect_err(|e| tracing::error!(error = %e, "bind_invite_member failed"))
        .map(|_| ())
    }
    /// Record an invite minted for delivery over the member's Lobby (a call, a
    /// direct message agent invite, `POST /rooms/:id/invites`). `invite` is the invite
    /// itself, in hand only here: the row keeps its hash, its id and its
    /// fingerprint, and the bounded delivery copy (`delivery_token`) until the
    /// invite's first use or its deadline (`INVITE_DELIVERY_WINDOW_MS`).
    pub fn issue_invite(&self, inv: &InviteRecord, invite: &str) -> rusqlite::Result<()> {
        let Some(mut c) = self.conn() else {
            return Ok(());
        };
        let tx = c.transaction()?;
        let suspended: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM member_suspensions WHERE member_id = ?1)",
            params![inv.member_id],
            |row| row.get(0),
        )?;
        if suspended {
            return Err(rusqlite::Error::InvalidQuery);
        }
        Self::require_unsealed_room(&tx, &inv.room)?;
        insert_issued_invite(&tx, inv, invite)
            .inspect_err(|e| tracing::error!(error = %e, "issue_invite failed"))?;
        tx.commit()
    }
    /// All live invites — loaded into memory at start-up. An unreadable table or
    /// row is an error, never "no invites": the start stops on it.
    pub fn load_invites(&self) -> rusqlite::Result<Vec<InviteRecord>> {
        let Some(c) = self.conn() else {
            return Ok(Vec::new());
        };
        let mut stmt = c.prepare(
            "SELECT id, secret_hash, room, member_id, name, kind, issued_at, issued_by
             FROM invites WHERE revoked_at IS NULL",
        )?;
        let rows = stmt.query_map([], |r| {
            let at: i64 = r.get(6)?;
            Ok(InviteRecord {
                id: r.get(0)?,
                secret_hash: r.get(1)?,
                room: r.get(2)?,
                member_id: r.get(3)?,
                name: r.get(4)?,
                kind: r.get(5)?,
                issued_at: at as u64,
                issued_by: r.get(7)?,
            })
        })?;
        rows.collect()
    }
    /// Revoke the invite — do not delete it, leave its trace (so who entered when stays known).
    /// By the invite's hash.
    pub fn revoke_invite_by_hash(&self, invite_hash: &str, at: u64) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else { return Ok(()) };
        c.execute(
            "UPDATE invites SET revoked_at = ?2 WHERE secret_hash = ?1 AND revoked_at IS NULL",
            params![invite_hash, at as i64],
        )
        .inspect_err(|e| tracing::error!(error = %e, "revoke_invite failed"))
        .map(|_| ())
    }
    /// Replace live invites with one fresh invite in one transaction: every
    /// invite whose hash is in `old` is ended and `fresh` (whose invite is
    /// `invite`) is written, or nothing is. Each old row must still be live,
    /// so a revoke that won the race is never undone by a re-issue, and a
    /// suspended member is refused as `issue_invite` refuses.
    pub fn reissue_invite(
        &self,
        old: &[String],
        fresh: &InviteRecord,
        invite: &str,
        at: u64,
    ) -> rusqlite::Result<()> {
        let Some(mut c) = self.conn() else {
            return Ok(());
        };
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let suspended: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM member_suspensions WHERE member_id = ?1)",
            params![fresh.member_id],
            |row| row.get(0),
        )?;
        if suspended {
            return Err(rusqlite::Error::InvalidQuery);
        }
        Self::require_unsealed_room(&tx, &fresh.room)?;
        for hash in old {
            let ended = tx.execute(
                "UPDATE invites SET revoked_at = ?2
                 WHERE secret_hash = ?1 AND room = ?3 AND member_id = ?4 AND revoked_at IS NULL",
                params![hash, at as i64, fresh.room, fresh.member_id],
            )?;
            if ended != 1 {
                return Err(rusqlite::Error::QueryReturnedNoRows);
            }
        }
        insert_issued_invite(&tx, fresh, invite)?;
        tx.commit()
            .inspect_err(|e| tracing::error!(error = %e, "reissue_invite failed"))
    }
    /// Record a newly admitted membership. The key is in hand only here, in
    /// the request that mints it: everything kept is derived from it now
    /// (`MemberIds`), its principal and credential are bound in the same
    /// transaction, and the key itself is not stored.
    pub fn add_member(
        &self,
        key: &str,
        name: &str,
        kind: &str,
        joined_at: u64,
        admitted_by: &str,
    ) -> rusqlite::Result<MemberRecord> {
        let ids = MemberIds::from_key(key);
        let mut record = MemberRecord {
            id: ids.id.clone(),
            secret_hash: ids.secret_hash.clone(),
            principal_id: None,
            name: name.to_string(),
            kind: kind.to_string(),
            joined_at,
            admitted_by: admitted_by.to_string(),
        };
        let Some(mut c) = self.conn() else {
            return Ok(record);
        };
        let tx = c.transaction()?;
        let principal_id =
            insert_member_record(&tx, "members", &ids, &record, None, "Member access")?;
        tx.commit()
            .inspect_err(|e| tracing::error!(error = %e, "add_member failed"))?;
        record.principal_id = Some(principal_id);
        Ok(record)
    }
    /// Revoke a building identity and every invite derived from it atomically.
    pub fn revoke_member_cascade(&self, member_id: &str, at: u64) -> rusqlite::Result<()> {
        let Some(mut c) = self.conn() else {
            return Ok(());
        };
        let tx = c.transaction()?;
        tx.execute(
            "UPDATE members SET revoked_at = ?2 WHERE id = ?1 AND revoked_at IS NULL",
            params![member_id, at],
        )?;
        tx.execute(
            "UPDATE invites SET revoked_at = ?2 WHERE member_id = ?1 AND revoked_at IS NULL",
            params![member_id, at],
        )?;
        // Every credential of the member's principal: its original key, the
        // keys its invites claimed and any added on the Profile. Disabling the
        // principal already stops them; revoking them says so in every list
        // and keeps them dead whatever later happens to the principal.
        tx.execute(
            "UPDATE credentials SET revoked_at = ?2
             WHERE revoked_at IS NULL AND principal_id IN
                 (SELECT principal_id FROM members WHERE id = ?1)",
            params![member_id, at],
        )?;
        tx.execute(
            "UPDATE principals SET disabled_at = ?2
             WHERE disabled_at IS NULL AND id IN
                 (SELECT principal_id FROM members WHERE id = ?1)",
            params![member_id, at],
        )?;
        tx.execute(
            "UPDATE principal_sessions SET revoked_at = ?2
             WHERE revoked_at IS NULL AND principal_id IN
                 (SELECT principal_id FROM members WHERE id = ?1)",
            params![member_id, at],
        )?;
        tx.commit()
            .inspect_err(|e| tracing::error!(error = %e, "revoke_member_cascade failed"))
    }
    /// Revoke every invite held by one displayed member in one loca atomically.
    pub fn revoke_invites_for(&self, room: &str, name: &str, at: u64) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else { return Ok(()) };
        c.execute(
            "UPDATE invites SET revoked_at = ?3
             WHERE room = ?1 AND name = ?2 AND revoked_at IS NULL",
            params![room, name, at as i64],
        )
        .inspect_err(|e| tracing::error!(error = %e, "revoke_invites_for failed"))
        .map(|_| ())
    }
    /// Live memberships. Revoked rows stay, so "who belonged, and when" keeps
    /// its answer. An unreadable table or row is an error, never "no
    /// members": the start stops on it.
    pub fn load_members(&self) -> rusqlite::Result<Vec<MemberRecord>> {
        let Some(c) = self.conn() else {
            return Ok(Vec::new());
        };
        let mut stmt = c.prepare(
            "SELECT id, secret_hash, principal_id, name, kind, joined_at, admitted_by
             FROM members WHERE revoked_at IS NULL",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(MemberRecord {
                id: r.get(0)?,
                secret_hash: r.get(1)?,
                principal_id: Some(r.get(2)?),
                name: r.get(3)?,
                kind: r.get(4)?,
                joined_at: r.get(5)?,
                admitted_by: r.get(6)?,
            })
        })?;
        rows.collect()
    }
    /// Record a ban or mute so it survives a restart. `kind` is 'ban'|'mute'.
    pub fn set_ban(
        &self,
        room: &str,
        name: &str,
        kind: &str,
        at: u64,
        authority_rank: u8,
        decided_by: Option<&str>,
    ) -> rusqlite::Result<()> {
        let Some(mut c) = self.conn() else {
            return Ok(());
        };
        let tx = c.transaction()?;
        tx.execute(
            "INSERT OR REPLACE INTO bans (room, name, kind, at, authority_rank, decided_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![room, name, kind, at as i64, authority_rank, decided_by],
        )?;
        if kind == "ban" {
            tx.execute(
                "UPDATE invites SET revoked_at = ?3
                WHERE room = ?1 AND name = ?2 AND revoked_at IS NULL",
                params![room, name, at as i64],
            )?;
        }
        tx.commit()
            .inspect_err(|e| tracing::error!(error = %e, "set_ban failed"))
    }
    /// Lift a ban or mute (unban/unmute) — the row is removed.
    pub fn clear_ban(&self, room: &str, name: &str, kind: &str) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else { return Ok(()) };
        c.execute(
            "DELETE FROM bans WHERE room = ?1 AND name = ?2 AND kind = ?3",
            params![room, name, kind],
        )
        .inspect_err(|e| tracing::error!(error = %e, "clear_ban failed"))
        .map(|_| ())
    }
    /// All persisted bans/mutes, as (room, name, kind). Loaded into each room
    /// on boot so the door state is exactly what it was before the restart.
    pub fn load_bans(&self) -> rusqlite::Result<Vec<ModerationRecord>> {
        let Some(c) = self.conn() else {
            return Ok(Vec::new());
        };
        let mut stmt = c.prepare("SELECT room, name, kind, authority_rank FROM bans")?;
        let rows = stmt.query_map([], |r| {
            Ok(ModerationRecord {
                room: r.get(0)?,
                name: r.get(1)?,
                kind: r.get(2)?,
                authority_rank: {
                    let rank = r.get::<_, u8>(3)?;
                    if (1..=3).contains(&rank) {
                        rank
                    } else {
                        3
                    }
                },
            })
        })?;
        rows.collect()
    }
    /// Change an active principal's display name. The principal id, its
    /// credentials, sessions and Building role are untouched: the name is only
    /// the label (PRINCIPLES). Returns `Ok(false)` when another active
    /// principal already wears that name (case-insensitively), so two people
    /// can never be told apart only by letter case. A live Smaster registry
    /// row of this principal follows the new name in the same transaction.
    pub fn rename_principal(
        &self,
        principal_id: &str,
        name: &str,
        relabel: bool,
    ) -> rusqlite::Result<bool> {
        let Some(mut c) = self.conn() else {
            return Ok(true);
        };
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let clash: Option<String> = tx
            .query_row(
                "SELECT id FROM principals
                 WHERE disabled_at IS NULL AND id <> ?1 AND lower(display_name) = lower(?2)
                 LIMIT 1",
                params![principal_id, name],
                |row| row.get(0),
            )
            .optional()?;
        if clash.is_some() {
            return Ok(false);
        }
        let changed = tx.execute(
            "UPDATE principals SET display_name = ?2 WHERE id = ?1 AND disabled_at IS NULL",
            params![principal_id, name],
        )?;
        if changed == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        tx.execute(
            "UPDATE smasters SET name = ?2 WHERE principal_id = ?1 AND revoked_at IS NULL",
            params![principal_id, name],
        )?;
        // Past messages follow the person: exactly the rows this principal
        // is recorded as having written. A row with no recorded writer keeps
        // its label; a label is never evidence of who wrote it.
        if relabel {
            tx.execute(
                "UPDATE messages SET sender = ?2 WHERE sender_principal = ?1",
                params![principal_id, name],
            )?;
        }
        tx.commit()
            .inspect_err(|e| tracing::error!(error = %e, "rename_principal failed"))?;
        Ok(true)
    }
    /// Record a newly minted Smaster code. The code is in hand only here,
    /// in the request that mints it: everything kept is derived from it now
    /// (`SmasterIds`), and the code itself is not stored.
    pub fn add_smaster(&self, code: &str, name: &str, at: u64) -> rusqlite::Result<SmasterRecord> {
        let ids = SmasterIds::from_code(code);
        let record = SmasterRecord {
            secret_hash: ids.secret_hash.clone(),
            principal_id: ids.principal_id.clone(),
            management_id: ids.management_id.clone(),
            name: name.to_string(),
        };
        let Some(mut c) = self.conn() else {
            return Ok(record);
        };
        let tx = c.transaction()?;
        insert_principal_credential(&tx, "smaster", code, name, "user", at, "Smaster access")?;
        tx.execute(
            "INSERT INTO smasters
             (management_id, secret_hash, principal_id, credential_id, fingerprint,
              name, issued_at, revoked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL)",
            params![
                ids.management_id,
                ids.secret_hash,
                ids.principal_id,
                ids.credential_id,
                ids.fingerprint,
                name,
                at
            ],
        )?;
        tx.commit()
            .inspect_err(|e| tracing::error!(error = %e, "add_smaster failed"))?;
        Ok(record)
    }
    /// Revoke a Smaster by its code's `secret_hash`: the registry row, its
    /// credentials and its principal end in one transaction.
    pub fn revoke_smaster(&self, secret_hash: &str, at: u64) -> rusqlite::Result<()> {
        let Some(mut c) = self.conn() else {
            return Ok(());
        };
        let tx = c.transaction()?;
        tx.execute(
            "UPDATE smasters SET revoked_at = ?2 WHERE secret_hash = ?1 AND revoked_at IS NULL",
            params![secret_hash, at],
        )?;
        revoke_principal_credential_hash(&tx, secret_hash, at)?;
        tx.commit()
            .inspect_err(|e| tracing::error!(error = %e, "revoke_smaster failed"))
    }
    /// Live Smasters. Revoked rows stay for the record. A storage error is an
    /// error, never an empty registry.
    pub fn load_smasters(&self) -> rusqlite::Result<Vec<SmasterRecord>> {
        let Some(c) = self.conn() else {
            return Ok(Vec::new());
        };
        let mut stmt = c.prepare(
            "SELECT secret_hash, principal_id, management_id, name
             FROM smasters WHERE revoked_at IS NULL",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(SmasterRecord {
                secret_hash: r.get(0)?,
                principal_id: r.get(1)?,
                management_id: r.get(2)?,
                name: r.get(3)?,
            })
        })?;
        rows.collect()
    }
}

/// A persisted admin session as the Hub restores it. The token is not part
/// of it: `secret_hash` is what a presented token is hashed to and looked up
/// by, and the hash is not itself a token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminSessionRecord {
    pub secret_hash: String,
    pub expires_at: u64,
}

/// A live Smaster as the registry and the Hub's in-memory map hold it. The
/// code is not part of it: `secret_hash` is what a presented code is hashed
/// to and looked up by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmasterRecord {
    pub secret_hash: String,
    pub principal_id: String,
    pub management_id: String,
    pub name: String,
}

/// What the Smaster registry keeps about one code, derived from the code
/// while it is in hand: when it is minted, or when a v0.13.0 registry that
/// still held it is migrated. The derivations are v0.13.0's own, so an
/// existing Smaster keeps the same principal (`pr_`), management id
/// (`smid_`) and legacy credential (`cr_`).
pub(crate) struct SmasterIds {
    pub secret_hash: String,
    pub principal_id: String,
    pub management_id: String,
    pub credential_id: String,
    pub fingerprint: String,
}

impl SmasterIds {
    pub(crate) fn from_code(code: &str) -> Self {
        let source = format!("smaster:{code}");
        Self {
            secret_hash: secret_hash(code),
            principal_id: hashed_id("pr_", &source),
            management_id: hashed_id("smid_", &source),
            credential_id: hashed_id("cr_", &source),
            fingerprint: super::keys::fingerprint(code),
        }
    }
}

/// A Building membership as the store and the Hub keep it. The key is not
/// part of it: a presented key is hashed and found through its credential
/// (`credentials.secret_hash`), and the hash is not a key. `id` is the
/// management id (`mbid_`) v0.13.0 already showed. `secret_hash` is the hash
/// of the membership's original key, kept for the one rule that asks about
/// that key (`Hub::reaches_building`: a legacy building key that is a
/// member's key). `principal_id` is `None` only in a memory-only Building,
/// which has no principals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberRecord {
    pub id: String,
    pub secret_hash: String,
    pub principal_id: Option<String>,
    pub name: String,
    pub kind: String,
    pub joined_at: u64,
    pub admitted_by: String,
}

/// An invite (invitation) is how someone enters a loca. Metaphor and mechanism
/// are the same thing: the invitation IS the key. It opens ONE loca — never
/// the building, never a second room. Revoking it ends the invitation; the
/// row stays so we can still say who was let in.
///
/// This is a live invite as the store and the Hub keep it. The invite is not
/// part of it: `secret_hash` is what a presented invite is hashed to and looked up
/// by. `id` is the management id (`ivid_`); `member_id` is the membership it
/// seats (`mbid_`), empty for an invite recorded before invites named one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteRecord {
    pub id: String,
    pub secret_hash: String,
    pub room: String,
    pub member_id: String,
    pub name: String,
    pub kind: String,
    pub issued_at: u64,
    pub issued_by: String,
}

impl InviteRecord {
    /// The record of `invite`, minted now: its hash and id are derived from
    /// it here, the only place it is in hand.
    pub fn issued(
        invite: &str,
        room: &str,
        member_id: &str,
        name: &str,
        kind: &str,
        issued_at: u64,
        issued_by: &str,
    ) -> Self {
        let ids = InviteIds::from_invite(invite);
        Self {
            id: ids.id,
            secret_hash: ids.secret_hash,
            room: room.to_string(),
            member_id: member_id.to_string(),
            name: name.to_string(),
            kind: kind.to_string(),
            issued_at,
            issued_by: issued_by.to_string(),
        }
    }
}

/// What the store keeps about one membership key, derived from the key while
/// it is in hand: when it is minted, or when a `members` table that still
/// held it (v0.13.0 and the builds up to this one) is migrated. The
/// derivations are v0.13.0's own, so a member keeps the same management id
/// (`mbid_`), principal (`pr_`) and legacy credential (`cr_`); the three
/// share one digest of `member:<key>` and differ only in their prefix.
pub(crate) struct MemberIds {
    pub secret_hash: String,
    pub id: String,
    pub principal_id: String,
    pub credential_id: String,
    pub fingerprint: String,
}

impl MemberIds {
    pub(crate) fn from_key(key: &str) -> Self {
        let source = format!("member:{key}");
        Self {
            secret_hash: secret_hash(key),
            id: hashed_id("mbid_", &source),
            principal_id: hashed_id("pr_", &source),
            credential_id: hashed_id("cr_", &source),
            fingerprint: super::keys::fingerprint(key),
        }
    }
}

/// The same for an invite: its hash, its management id (`ivid_`, v0.13.0's
/// derivation) and the ledger's fingerprint.
pub(crate) struct InviteIds {
    pub secret_hash: String,
    pub id: String,
    pub fingerprint: String,
}

impl InviteIds {
    pub(crate) fn from_invite(invite: &str) -> Self {
        Self {
            secret_hash: secret_hash(invite),
            id: hashed_id("ivid_", &format!("invite:{invite}")),
            fingerprint: super::keys::fingerprint(invite),
        }
    }
}

/// Write one `members` row for the key `ids` were derived from, binding its
/// principal and legacy credential first with v0.13.0's identifiers (`INSERT
/// OR IGNORE`: a binding that already exists is kept as it is, renamed,
/// disabled or revoked). The row names the principal and credential that
/// binding actually holds. `revoked_at` is the membership's revocation, which
/// a binding made here carries too. `table` is `members`, or the table
/// `migrate_member_keys` builds. Returns the principal's id. Used at
/// admission, at join-request approval and by `migrate_member_keys`.
pub(super) fn insert_member_record(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    ids: &MemberIds,
    member: &MemberRecord,
    revoked_at: Option<u64>,
    label: &str,
) -> rusqlite::Result<String> {
    let principal_kind = if member.kind == "agent" {
        "agent"
    } else {
        "human"
    };
    let legacy_source = format!("member:{}", ids.secret_hash);
    tx.execute(
        "INSERT OR IGNORE INTO principals
         (id, display_name, kind, building_role, created_at, disabled_at)
         VALUES (?1, ?2, ?3, 'member', ?4, ?5)",
        params![
            ids.principal_id,
            member.name,
            principal_kind,
            member.joined_at,
            revoked_at
        ],
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO credentials
         (id, principal_id, label, secret_hash, created_at, last_used_at, revoked_at, legacy_source)
         VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7)",
        params![
            ids.credential_id,
            ids.principal_id,
            label,
            ids.secret_hash,
            member.joined_at,
            revoked_at,
            legacy_source
        ],
    )?;
    let (credential_id, principal_id): (String, String) = tx.query_row(
        "SELECT id, principal_id FROM credentials WHERE legacy_source = ?1",
        params![legacy_source],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    tx.execute(
        &format!(
            "INSERT INTO {table}
             (id, secret_hash, principal_id, credential_id, fingerprint, name, kind, joined_at,
              admitted_by, revoked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"
        ),
        params![
            ids.id,
            ids.secret_hash,
            principal_id,
            credential_id,
            ids.fingerprint,
            member.name,
            member.kind,
            member.joined_at,
            member.admitted_by,
            revoked_at
        ],
    )?;
    Ok(principal_id)
}

/// Write the row of an invite minted for delivery, with its delivery copy.
pub(super) fn insert_issued_invite(
    tx: &rusqlite::Transaction<'_>,
    inv: &InviteRecord,
    invite: &str,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO invites
         (id, secret_hash, fingerprint, room, member_id, name, kind, issued_at, issued_by,
          revoked_at, delivery_token, first_used_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, ?10, NULL)",
        params![
            inv.id,
            inv.secret_hash,
            super::keys::fingerprint(invite),
            inv.room,
            inv.member_id,
            inv.name,
            inv.kind,
            inv.issued_at as i64,
            inv.issued_by,
            invite
        ],
    )
    .map(|_| ())
}

/// Register a `smasters` row's identity in the identity-v2 principals +
/// credentials tables (`add_smaster`). Members are bound by
/// `insert_member_record`.
pub(super) fn insert_principal_credential(
    tx: &rusqlite::Transaction<'_>,
    role: &str,
    token: &str,
    name: &str,
    kind: &str,
    at: u64,
    label: &str,
) -> rusqlite::Result<()> {
    let principal_id = hashed_id("pr_", &format!("{role}:{token}"));
    let credential_id = hashed_id("cr_", &format!("{role}:{token}"));
    let kind = if kind == "agent" { "agent" } else { "human" };
    tx.execute(
        "INSERT OR IGNORE INTO principals
         (id, display_name, kind, building_role, created_at, disabled_at)
         VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
        params![principal_id, name, kind, role, at],
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO credentials
         (id, principal_id, label, secret_hash, created_at, last_used_at, revoked_at, legacy_source)
         VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL, ?6)",
        params![
            credential_id,
            principal_id,
            label,
            secret_hash(token),
            at,
            format!("{role}:{}", secret_hash(token))
        ],
    )?;
    Ok(())
}

fn revoke_principal_credential_hash(
    tx: &rusqlite::Transaction<'_>,
    hash: &str,
    at: u64,
) -> rusqlite::Result<()> {
    tx.execute(
        "UPDATE credentials SET revoked_at = ?2
         WHERE secret_hash = ?1 AND revoked_at IS NULL",
        params![hash, at],
    )?;
    tx.execute(
        "UPDATE principals SET disabled_at = ?2
         WHERE id IN (SELECT principal_id FROM credentials WHERE secret_hash = ?1)
           AND disabled_at IS NULL",
        params![hash, at],
    )?;
    Ok(())
}

fn parse_building_role(role: &str) -> BuildingRole {
    match role {
        "master" => BuildingRole::Master,
        "smaster" => BuildingRole::Smaster,
        _ => BuildingRole::Member,
    }
}

/// Delete the session rows that can no longer be presented: a principal
/// session that has expired or was ended (sign-out, revocation, suspension,
/// eviction), and a browser admin session that has expired or whose principal
/// session was ended. No reader needs an ended row: every check of a session
/// asks for a live row, so a missing row reads as ended, and a credential's
/// "last used" lives on the credential. A row written before every session
/// had a lifetime (`expires_at = 0`) counts as expired.
pub(super) fn sweep_session_rows(c: &Connection, now: u64) -> rusqlite::Result<usize> {
    // One transaction: a failure deletes nothing, so "none deleted" is true.
    let tx = c.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM admin_sessions WHERE expires_at <= ?1
            OR secret_hash IN (SELECT id FROM principal_sessions
                               WHERE revoked_at IS NOT NULL OR expires_at <= ?1)",
        params![now],
    )?;
    let deleted = tx.execute(
        "DELETE FROM principal_sessions WHERE revoked_at IS NOT NULL OR expires_at <= ?1",
        params![now],
    )?;
    tx.commit()?;
    Ok(deleted)
}
