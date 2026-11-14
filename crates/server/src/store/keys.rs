//! The key ledger: every code the Building has handed out, in one list.
//!
//! Smaster codes, memberships, davets and admission rights already live in
//! their own tables; the ledger reads them there rather than copying them, so
//! nothing can be issued without appearing here. Master pairing codes were
//! only ever held in memory, so they get their own log (`pairing_log`).
//! A code's secret never appears: only a short fingerprint to recognise it.

use rusqlite::{params, Connection};

use super::{hashed_id, Store};

/// One issued code, as the Master's ledger shows it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct KeyRecord {
    /// Opaque id used to act on it (revoke); never the secret.
    pub id: String,
    /// master | smaster | member | davet | admission
    pub kind: &'static str,
    /// The first characters of the code, enough to recognise the one handed
    /// over, never enough to use it.
    pub fingerprint: String,
    /// Whom it was issued for (a name, or a loca for a davet).
    pub for_name: Option<String>,
    pub loca: Option<String>,
    pub issued_by: Option<String>,
    pub issued_at: u64,
    /// Unused past this moment, it no longer works.
    pub use_by: Option<u64>,
    pub used_at: Option<u64>,
    pub last_used_at: Option<u64>,
    pub revoked_at: Option<u64>,
    /// unused | active | used | expired | superseded | revoked
    pub state: &'static str,
}

/// Fingerprint of a raw code: its prefix and four characters, then an ellipsis.
pub fn fingerprint(code: &str) -> String {
    let (prefix, rest) = code.split_once('_').unwrap_or(("", code));
    let head: String = rest.chars().take(4).collect();
    if prefix.is_empty() {
        format!("{head}…")
    } else {
        format!("{prefix}_{head}…")
    }
}

/// Last use of the credential minted for a legacy code, on a connection the
/// caller already holds.
fn credential_last_used_on(c: &Connection, role: &str, token: &str) -> Option<u64> {
    c.query_row(
        "SELECT last_used_at FROM credentials WHERE id = ?1",
        params![hashed_id("cr_", &format!("{role}:{token}"))],
        |r| r.get::<_, Option<i64>>(0),
    )
    .ok()
    .flatten()
    .map(|v| v as u64)
}

/// (token, name, joined_at, admitted_by, revoked_at)
type MemberRow = (String, String, u64, Option<String>, Option<u64>);
/// (token, room, name, issued_at, issued_by, revoked_at)
type InviteRow = (String, String, String, u64, Option<String>, Option<u64>);

fn opt(v: Option<i64>) -> Option<u64> {
    v.map(|v| v as u64)
}

impl Store {
    pub fn log_pairing(
        &self,
        id: &str,
        code: &str,
        issued_at: u64,
        expires_at: u64,
        session_ttl_ms: u64,
    ) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else { return Ok(()) };
        c.execute(
            "INSERT OR IGNORE INTO pairing_log (id, fingerprint, issued_at, expires_at, session_ttl_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                id,
                fingerprint(code),
                issued_at as i64,
                expires_at as i64,
                session_ttl_ms as i64
            ],
        )?;
        Ok(())
    }

    pub fn mark_pairing_used(&self, id: &str, at: u64) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else { return Ok(()) };
        c.execute(
            "UPDATE pairing_log SET used_at = ?2 WHERE id = ?1 AND used_at IS NULL",
            params![id, at as i64],
        )?;
        Ok(())
    }

    pub fn mark_pairing_superseded(&self, id: &str, at: u64) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else { return Ok(()) };
        c.execute(
            "UPDATE pairing_log SET superseded_at = ?2
             WHERE id = ?1 AND used_at IS NULL AND superseded_at IS NULL",
            params![id, at as i64],
        )?;
        Ok(())
    }

    /// Smaster codes issued before `cutoff` that were never used.
    pub fn unused_smasters_issued_before(&self, cutoff: u64) -> Vec<String> {
        let Some(c) = self.conn() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let Ok(mut rows) =
            c.prepare("SELECT token FROM smasters WHERE revoked_at IS NULL AND issued_at < ?1")
        else {
            return out;
        };
        let tokens: Vec<String> = rows
            .query_map(params![cutoff as i64], |r| r.get(0))
            .map(|it| it.flatten().collect())
            .unwrap_or_default();
        // Reuse the connection already held: the store mutex is not
        // re-entrant, and taking it again here would deadlock the sweep.
        for token in tokens {
            if credential_last_used_on(&c, "smaster", &token).is_none() {
                out.push(token);
            }
        }
        out
    }

    /// Has any credential of this principal ever been used?
    pub fn principal_ever_used(&self, principal_id: &str) -> bool {
        let Some(c) = self.conn() else { return true };
        c.query_row(
            "SELECT EXISTS(SELECT 1 FROM credentials
                 WHERE principal_id = ?1 AND last_used_at IS NOT NULL)",
            params![principal_id],
            |r| r.get::<_, bool>(0),
        )
        .unwrap_or(true)
    }

    /// The whole ledger, newest first. `smaster_use_ms` is how long an unused
    /// Smaster code stays usable.
    pub fn key_ledger(&self, now: u64, smaster_use_ms: u64) -> rusqlite::Result<Vec<KeyRecord>> {
        let Some(c) = self.conn() else {
            return Ok(Vec::new());
        };
        let mut out: Vec<KeyRecord> = Vec::new();

        let mut st = c.prepare(
            "SELECT id, fingerprint, issued_at, expires_at, used_at, superseded_at FROM pairing_log",
        )?;
        for r in st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)? as u64,
                r.get::<_, i64>(3)? as u64,
                opt(r.get(4)?),
                opt(r.get(5)?),
            ))
        })? {
            let (id, fp, issued, expires, used, superseded) = r?;
            let state = if used.is_some() {
                "used"
            } else if superseded.is_some() {
                "superseded"
            } else if now >= expires {
                "expired"
            } else {
                "unused"
            };
            out.push(KeyRecord {
                id,
                kind: "master",
                fingerprint: fp,
                for_name: Some("Master".into()),
                loca: None,
                issued_by: Some("Master".into()),
                issued_at: issued,
                use_by: Some(expires),
                used_at: used,
                last_used_at: used,
                revoked_at: superseded,
                state,
            });
        }

        let mut st = c.prepare("SELECT token, name, issued_at, revoked_at FROM smasters")?;
        let rows: Vec<(String, String, u64, Option<u64>)> = st
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get::<_, i64>(2)? as u64,
                    opt(r.get(3)?),
                ))
            })?
            .collect::<Result<_, _>>()?;
        for (token, name, issued, revoked) in rows {
            let last = credential_last_used_on(&c, "smaster", &token);
            let use_by = issued.saturating_add(smaster_use_ms);
            let state = match (revoked, last) {
                (Some(_), None) if revoked.is_some_and(|r| r >= use_by) => "expired",
                (Some(_), _) => "revoked",
                (None, Some(_)) => "active",
                (None, None) if now >= use_by => "expired",
                (None, None) => "unused",
            };
            out.push(KeyRecord {
                id: hashed_id("smid_", &format!("smaster:{token}")),
                kind: "smaster",
                fingerprint: fingerprint(&token),
                for_name: Some(name),
                loca: None,
                issued_by: Some("Master".into()),
                issued_at: issued,
                use_by: last.is_none().then_some(use_by),
                used_at: last,
                last_used_at: last,
                revoked_at: revoked,
                state,
            });
        }

        let mut st =
            c.prepare("SELECT token, name, joined_at, admitted_by, revoked_at FROM members")?;
        let rows: Vec<MemberRow> = st
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get::<_, i64>(2)? as u64,
                    r.get(3)?,
                    opt(r.get(4)?),
                ))
            })?
            .collect::<Result<_, _>>()?;
        for (token, name, issued, by, revoked) in rows {
            let last = credential_last_used_on(&c, "member", &token);
            let state = match (revoked, last) {
                (Some(_), _) => "revoked",
                (None, Some(_)) => "active",
                (None, None) => "unused",
            };
            out.push(KeyRecord {
                id: hashed_id("mbid_", &format!("member:{token}")),
                kind: "member",
                fingerprint: fingerprint(&token),
                for_name: Some(name),
                loca: None,
                issued_by: by,
                issued_at: issued,
                use_by: None,
                used_at: last,
                last_used_at: last,
                revoked_at: revoked,
                state,
            });
        }

        let mut st =
            c.prepare("SELECT token, room, name, issued_at, issued_by, revoked_at FROM invites")?;
        let rows: Vec<InviteRow> = st
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get::<_, i64>(3)? as u64,
                    r.get(4)?,
                    opt(r.get(5)?),
                ))
            })?
            .collect::<Result<_, _>>()?;
        for (token, room, name, issued, by, revoked) in rows {
            // A birebir's davet is that pair's business, not the ledger's.
            if room.starts_with("bb-") {
                continue;
            }
            out.push(KeyRecord {
                id: hashed_id("ivid_", &format!("invite:{token}")),
                kind: "davet",
                fingerprint: fingerprint(&token),
                for_name: Some(name),
                loca: Some(room),
                issued_by: by,
                issued_at: issued,
                use_by: None,
                used_at: None,
                last_used_at: None,
                revoked_at: revoked,
                state: if revoked.is_some() {
                    "revoked"
                } else {
                    "active"
                },
            });
        }

        let mut st = c.prepare(
            "SELECT id, created_at, expires_at, consumed_at, consumed_by_name FROM admission_stock",
        )?;
        for r in st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)? as u64,
                r.get::<_, i64>(2)? as u64,
                opt(r.get(3)?),
                r.get::<_, Option<String>>(4)?,
            ))
        })? {
            let (id, issued, expires, consumed, by_name) = r?;
            let state = if consumed.is_some() {
                "used"
            } else if now >= expires {
                "expired"
            } else {
                "unused"
            };
            out.push(KeyRecord {
                id: hashed_id("adid_", &id),
                kind: "admission",
                fingerprint: fingerprint(&id),
                for_name: by_name,
                loca: None,
                issued_by: Some("Master".into()),
                issued_at: issued,
                use_by: Some(expires),
                used_at: consumed,
                last_used_at: consumed,
                revoked_at: None,
                state,
            });
        }

        out.sort_by_key(|k| std::cmp::Reverse(k.issued_at));
        Ok(out)
    }
}
