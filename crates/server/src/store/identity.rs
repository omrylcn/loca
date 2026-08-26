use super::*;

impl Store {
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

    /// Resolve the stable principal behind a legacy membership record without
    /// treating that record's original bearer as active. This is used to map
    /// newer per-device credentials back to the same Building membership.
    pub fn principal_id_for_member_record(&self, member_token: &str) -> Option<String> {
        let c = self.conn()?;
        c.query_row(
            "SELECT principal_id FROM credentials WHERE legacy_source = ?1",
            params![format!("member:{}", secret_hash(member_token))],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten()
    }

    /// Return the original legacy auth source for a v2 credential. The raw
    /// token never leaves the Store; Hub uses it only to retire an in-memory
    /// compatibility fallback after the DB transaction commits.
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
        let (role, expected_hash) = source.split_once(':')?;
        let sql = match role {
            "member" => "SELECT token FROM members",
            "smaster" => "SELECT token FROM smasters",
            _ => return None,
        };
        let mut stmt = c.prepare(sql).ok()?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0)).ok()?;
        rows.flatten()
            .collect::<Vec<_>>()
            .into_iter()
            .find(|token| secret_hash(token) == expected_hash)
            .map(|token| (role.to_string(), token))
    }

    pub fn credential_id_for_session(&self, session_secret: &str, now: u64) -> Option<String> {
        let c = self.conn()?;
        c.query_row(
            "SELECT s.credential_id FROM principal_sessions s
             JOIN credentials c ON c.id = s.credential_id AND c.principal_id = s.principal_id
             JOIN principals p ON p.id = s.principal_id
             WHERE s.id = ?1 AND s.revoked_at IS NULL
               AND (s.expires_at = 0 OR s.expires_at > ?2)
               AND c.revoked_at IS NULL AND p.disabled_at IS NULL",
            params![secret_hash(session_secret), now],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten()
    }

    pub fn session_uses_credential(&self, session_secret: &str, credential_id: &str) -> bool {
        let Some(c) = self.conn() else {
            return false;
        };
        c.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM principal_sessions
                WHERE id = ?1 AND credential_id = ?2
             )",
            params![secret_hash(session_secret), credential_id],
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
        if let Some(expected_hash) = legacy_source
            .as_deref()
            .and_then(|source| source.strip_prefix("smaster:"))
        {
            let matching_token = {
                let mut stmt = tx
                    .prepare("SELECT token FROM smasters WHERE revoked_at IS NULL")
                    .map_err(|_| CredentialError::Storage)?;
                let rows = stmt
                    .query_map([], |row| row.get::<_, String>(0))
                    .map_err(|_| CredentialError::Storage)?;
                rows.flatten()
                    .collect::<Vec<_>>()
                    .into_iter()
                    .find(|token| secret_hash(token) == expected_hash)
            };
            if let Some(token) = matching_token {
                tx.execute(
                    "UPDATE smasters SET revoked_at = ?2
                     WHERE token = ?1 AND revoked_at IS NULL",
                    params![token, at],
                )
                .map_err(|_| CredentialError::Storage)?;
            }
        }
        tx.commit().map_err(|_| CredentialError::Storage)
    }

    pub fn save_principal_session(
        &self,
        session_secret: &str,
        principal_id: &str,
        credential_id: &str,
        created_at: u64,
        expires_at: u64,
    ) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else { return Ok(()) };
        c.execute(
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
        )
        .map(|_| ())
    }

    pub fn principal_session_active(&self, session_secret: &str, now: u64) -> bool {
        let Some(c) = self.conn() else { return true };
        c.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM principal_sessions s
                JOIN credentials c ON c.id = s.credential_id AND c.principal_id = s.principal_id
                JOIN principals p ON p.id = s.principal_id
                WHERE s.id = ?1 AND s.revoked_at IS NULL
                  AND (s.expires_at = 0 OR s.expires_at > ?2)
                  AND c.revoked_at IS NULL AND p.disabled_at IS NULL
             )",
            params![secret_hash(session_secret), now],
            |row| row.get::<_, i64>(0),
        )
        .map(|exists| exists != 0)
        .unwrap_or(false)
    }

    /// Whether this token has ever been bound to a principal credential.
    /// Revoked rows deliberately count: boot migration may backfill truly
    /// legacy admin sessions, but must never resurrect a revoked binding by
    /// attaching the same token to the root recovery credential.
    pub fn principal_session_exists(&self, session_secret: &str) -> bool {
        let Some(c) = self.conn() else { return false };
        c.query_row(
            "SELECT EXISTS(SELECT 1 FROM principal_sessions WHERE id = ?1)",
            params![secret_hash(session_secret)],
            |row| row.get::<_, i64>(0),
        )
        .map(|exists| exists != 0)
        .unwrap_or(false)
    }

    pub fn revoke_principal_session(&self, session_secret: &str, at: u64) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else { return Ok(()) };
        c.execute(
            "UPDATE principal_sessions SET revoked_at = ?2
             WHERE id = ?1 AND revoked_at IS NULL",
            params![secret_hash(session_secret), at],
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
    /// attaches another proof to the same Master instead of creating Master #2.
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
            "INSERT OR IGNORE INTO credentials
             (id, principal_id, label, secret_hash, created_at, last_used_at, revoked_at, legacy_source)
             VALUES (?1, ?2, 'Root recovery', ?3, ?4, NULL, NULL, ?5)",
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

    pub fn save_admin_session(
        &self,
        token: &str,
        name: &str,
        kind: SenderType,
        expires_at: u64,
    ) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else { return Ok(()) };
        c.execute(
            "INSERT OR REPLACE INTO admin_sessions (token, name, kind, expires_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![token, name, sender_type_str(kind), expires_at],
        )
        .inspect_err(|e| tracing::error!(error = %e, "save_admin_session failed"))
        .map(|_| ())
    }
    pub fn delete_admin_session(&self, token: &str) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else { return Ok(()) };
        c.execute(
            "DELETE FROM admin_sessions WHERE token = ?1",
            params![token],
        )?;
        c.execute(
            "UPDATE principal_sessions SET revoked_at = ?2
             WHERE id = ?1 AND revoked_at IS NULL",
            params![secret_hash(token), current_time_ms()],
        )
        .inspect_err(|e| tracing::error!(error = %e, "delete_admin_session failed"))
        .map(|_| ())
    }
    /// Restore only unexpired admin sessions. Expired rows are removed at boot
    /// so persistence never widens the authority window selected at pairing.
    pub fn load_admin_sessions(&self, now: u64) -> Vec<(String, String, SenderType, u64)> {
        let Some(c) = self.conn() else {
            return Vec::new();
        };
        if let Err(e) = c.execute(
            "DELETE FROM admin_sessions WHERE expires_at <= ?1",
            params![now],
        ) {
            tracing::error!(error = %e, "expired admin session cleanup failed");
        }
        let mut out = Vec::new();
        if let Ok(mut stmt) = c.prepare(
            "SELECT token, name, kind, expires_at
             FROM admin_sessions WHERE expires_at > ?1",
        ) {
            if let Ok(rows) = stmt.query_map(params![now], |r| {
                let kind: String = r.get(2)?;
                Ok((r.get(0)?, r.get(1)?, parse_sender_type(&kind), r.get(3)?))
            }) {
                out.extend(rows.flatten());
            }
        }
        out
    }
    /// Bir daveti kaydet. Master üretir; token'ın kendisi burada üretilmez.
    pub fn insert_invite(&self, inv: &Invite) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else { return Ok(()) };
        c.execute(
            "INSERT OR REPLACE INTO invites
             (token, room, member, name, kind, issued_at, issued_by, revoked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL)",
            params![
                inv.token,
                inv.room,
                inv.member,
                inv.name,
                inv.kind,
                inv.issued_at as i64,
                inv.issued_by
            ],
        )
        .inspect_err(|e| tracing::error!(error = %e, "insert_invite failed"))
        .map(|_| ())
    }
    /// Tüm geçerli davetler — açılışta belleğe alınır.
    pub fn load_invites(&self) -> Vec<Invite> {
        let Some(c) = self.conn() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if let Ok(mut stmt) = c.prepare(
            "SELECT token, room, member, name, kind, issued_at, issued_by
             FROM invites WHERE revoked_at IS NULL",
        ) {
            if let Ok(rows) = stmt.query_map([], |r| {
                let at: i64 = r.get(5)?;
                Ok(Invite {
                    token: r.get(0)?,
                    room: r.get(1)?,
                    member: r.get(2)?,
                    name: r.get(3)?,
                    kind: r.get(4)?,
                    issued_at: at as u64,
                    issued_by: r.get(6)?,
                })
            }) {
                out.extend(rows.flatten());
            }
        }
        out
    }
    /// Daveti iptal et — silme, izini bırak (kim ne zaman girmişti bilinsin).
    pub fn revoke_invite(&self, token: &str, at: u64) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else { return Ok(()) };
        c.execute(
            "UPDATE invites SET revoked_at = ?2 WHERE token = ?1 AND revoked_at IS NULL",
            params![token, at as i64],
        )
        .inspect_err(|e| tracing::error!(error = %e, "revoke_invite failed"))
        .map(|_| ())
    }
    pub fn add_member(&self, m: &protocol::Membership) -> rusqlite::Result<()> {
        let Some(mut c) = self.conn() else {
            return Ok(());
        };
        let tx = c.transaction()?;
        tx.execute(
            "INSERT OR REPLACE INTO members (token, name, kind, joined_at, admitted_by, revoked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
            params![m.token, m.name, m.kind, m.joined_at, m.admitted_by],
        )?;
        insert_principal_credential(
            &tx,
            "member",
            &m.token,
            &m.name,
            &m.kind,
            m.joined_at,
            "Member access",
        )?;
        tx.commit()
            .inspect_err(|e| tracing::error!(error = %e, "add_member failed"))
    }
    /// Revoke a building identity and every davet derived from it atomically.
    pub fn revoke_member_cascade(&self, token: &str, at: u64) -> rusqlite::Result<()> {
        let Some(mut c) = self.conn() else {
            return Ok(());
        };
        let tx = c.transaction()?;
        tx.execute(
            "UPDATE members SET revoked_at = ?2 WHERE token = ?1 AND revoked_at IS NULL",
            params![token, at],
        )?;
        tx.execute(
            "UPDATE invites SET revoked_at = ?2 WHERE member = ?1 AND revoked_at IS NULL",
            params![token, at],
        )?;
        revoke_principal_credential(&tx, token, at)?;
        tx.commit()
            .inspect_err(|e| tracing::error!(error = %e, "revoke_member_cascade failed"))
    }
    /// Revoke every davet held by one displayed member in one loca atomically.
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
    /// its answer.
    pub fn load_members(&self) -> Vec<protocol::Membership> {
        let Some(c) = self.conn() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if let Ok(mut stmt) = c.prepare(
            "SELECT token, name, kind, joined_at, admitted_by FROM members WHERE revoked_at IS NULL",
        ) {
            if let Ok(rows) = stmt.query_map([], |r| {
                Ok(protocol::Membership {
                    token: r.get(0)?,
                    name: r.get(1)?,
                    kind: r.get(2)?,
                    joined_at: r.get(3)?,
                    admitted_by: r.get(4)?,
                })
            }) {
                out.extend(rows.flatten());
            }
        }
        out
    }
    /// Record a ban or mute so it survives a restart. `kind` is 'ban'|'mute'.
    pub fn set_ban(&self, room: &str, name: &str, kind: &str, at: u64) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else { return Ok(()) };
        c.execute(
            "INSERT OR REPLACE INTO bans (room, name, kind, at) VALUES (?1, ?2, ?3, ?4)",
            params![room, name, kind, at as i64],
        )
        .inspect_err(|e| tracing::error!(error = %e, "set_ban failed"))
        .map(|_| ())
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
    pub fn load_bans(&self) -> Vec<(String, String, String)> {
        let Some(c) = self.conn() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if let Ok(mut stmt) = c.prepare("SELECT room, name, kind FROM bans") {
            if let Ok(rows) = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            }) {
                out.extend(rows.flatten());
            }
        }
        out
    }
    pub fn add_smaster(&self, token: &str, name: &str, at: u64) -> rusqlite::Result<()> {
        let Some(mut c) = self.conn() else {
            return Ok(());
        };
        let tx = c.transaction()?;
        tx.execute(
            "INSERT OR REPLACE INTO smasters (token, name, issued_at, revoked_at) VALUES (?1, ?2, ?3, NULL)",
            params![token, name, at],
        )?;
        insert_principal_credential(&tx, "smaster", token, name, "user", at, "Smaster access")?;
        tx.commit()
            .inspect_err(|e| tracing::error!(error = %e, "add_smaster failed"))
    }
    pub fn revoke_smaster(&self, token: &str, at: u64) -> rusqlite::Result<()> {
        let Some(mut c) = self.conn() else {
            return Ok(());
        };
        let tx = c.transaction()?;
        tx.execute(
            "UPDATE smasters SET revoked_at = ?2 WHERE token = ?1 AND revoked_at IS NULL",
            params![token, at],
        )?;
        revoke_principal_credential(&tx, token, at)?;
        tx.commit()
            .inspect_err(|e| tracing::error!(error = %e, "revoke_smaster failed"))
    }
    /// Live smasters: (token, name). Revoked rows stay for the record.
    pub fn load_smasters(&self) -> Vec<(String, String)> {
        let Some(c) = self.conn() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if let Ok(mut stmt) = c.prepare("SELECT token, name FROM smasters WHERE revoked_at IS NULL")
        {
            if let Ok(rows) = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))) {
                out.extend(rows.flatten());
            }
        }
        out
    }
}

/// Register a `members`/`smasters` row's identity in the identity-v2
/// principals + credentials tables. `add_member` calls this; the join-request
/// atomic approve (in the parent module) calls it too, so an approve-issued
/// `mb_` authenticates immediately on a persistent store instead of only after
/// the next restart-time migration. `pub(super)` so `store.rs` can reuse it.
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

fn revoke_principal_credential(
    tx: &rusqlite::Transaction<'_>,
    token: &str,
    at: u64,
) -> rusqlite::Result<()> {
    let hash = secret_hash(token);
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
