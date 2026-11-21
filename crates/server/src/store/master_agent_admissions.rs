//! A Master Agent's one-step admissions: the bounded window in which the
//! same claim gets the same key back, and the counts its limits read.
//!
//! The key is kept in clear text (`delivery_token`) only until the Master
//! Agent acknowledges it or `ADMISSION_DELIVERY_WINDOW_MS` passes, the way a
//! join request's key and a membership claim's are; after that only the
//! claim's hash remains.

use rusqlite::{params, OptionalExtension};

use super::{secret_hash, Store};

/// How long the same claim may collect the same key again.
pub const ADMISSION_DELIVERY_WINDOW_MS: u64 = 24 * 60 * 60 * 1000;

pub(super) const SCHEMA: &str = r#"
    CREATE TABLE IF NOT EXISTS master_agent_admissions (
        claim_hash TEXT PRIMARY KEY,
        principal_id TEXT NOT NULL,
        room TEXT NOT NULL,
        member_id TEXT NOT NULL,
        name TEXT NOT NULL,
        delivery_token TEXT,
        created_at INTEGER NOT NULL,
        acked_at INTEGER,
        seated_at INTEGER
    );
"#;

/// What a claim already holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionClaim {
    pub principal_id: String,
    pub room: String,
    pub member_id: String,
    pub name: String,
    /// The key, while it may still be collected.
    pub key: Option<String>,
    pub created_at: u64,
    pub acked_at: Option<u64>,
    /// When the floor and seat steps of this admission completed; a replay
    /// of the claim repeats them only while this is empty.
    pub seated_at: Option<u64>,
}

impl Store {
    /// The claim recorded under `claim_id`, if any. A key past its window
    /// is cleared first, so it is never returned.
    pub fn admission_claim(
        &self,
        claim_id: &str,
        now: u64,
    ) -> rusqlite::Result<Option<AdmissionClaim>> {
        let Some(c) = self.conn() else {
            return Err(rusqlite::Error::InvalidQuery);
        };
        let hash = secret_hash(claim_id);
        c.execute(
            "UPDATE master_agent_admissions SET delivery_token = NULL
             WHERE claim_hash = ?1 AND delivery_token IS NOT NULL AND created_at <= ?2",
            params![
                hash,
                now.saturating_sub(ADMISSION_DELIVERY_WINDOW_MS) as i64
            ],
        )?;
        c.query_row(
            "SELECT principal_id, room, member_id, name, delivery_token, created_at, acked_at,
                    seated_at
             FROM master_agent_admissions WHERE claim_hash = ?1",
            params![hash],
            |r| {
                Ok(AdmissionClaim {
                    principal_id: r.get(0)?,
                    room: r.get(1)?,
                    member_id: r.get(2)?,
                    name: r.get(3)?,
                    key: r.get(4)?,
                    created_at: r.get::<_, i64>(5)? as u64,
                    acked_at: r.get::<_, Option<i64>>(6)?.map(|v| v as u64),
                    seated_at: r.get::<_, Option<i64>>(7)?.map(|v| v as u64),
                })
            },
        )
        .optional()
    }

    /// Record a new admission's claim with its key.
    #[allow(clippy::too_many_arguments)]
    pub fn record_admission_claim(
        &self,
        claim_id: &str,
        principal: &str,
        room: &str,
        member_id: &str,
        name: &str,
        key: &str,
        at: u64,
    ) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else {
            return Err(rusqlite::Error::InvalidQuery);
        };
        c.execute(
            "INSERT INTO master_agent_admissions
             (claim_hash, principal_id, room, member_id, name, delivery_token, created_at, acked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL)",
            params![
                secret_hash(claim_id),
                principal,
                room,
                member_id,
                name,
                key,
                at as i64
            ],
        )?;
        Ok(())
    }

    /// The Master Agent acknowledged the key: it is never handed out again.
    /// True when the claim is this Master Agent's, for this member.
    pub fn ack_admission_claim(
        &self,
        claim_id: &str,
        principal: &str,
        member_id: &str,
        at: u64,
    ) -> rusqlite::Result<bool> {
        let Some(c) = self.conn() else {
            return Err(rusqlite::Error::InvalidQuery);
        };
        let changed = c.execute(
            "UPDATE master_agent_admissions
             SET delivery_token = NULL, acked_at = COALESCE(acked_at, ?4)
             WHERE claim_hash = ?1 AND principal_id = ?2 AND member_id = ?3",
            params![secret_hash(claim_id), principal, member_id, at as i64],
        )?;
        Ok(changed > 0)
    }

    /// Members `by` (an `admitted_by` value) admitted that are not revoked,
    /// and how many of them it admitted at or after `since`. A count that
    /// cannot be read counts as over every limit.
    /// The floor and seat steps of a claim's admission completed.
    pub fn mark_admission_seated(&self, claim_id: &str, at: u64) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else {
            return Err(rusqlite::Error::InvalidQuery);
        };
        c.execute(
            "UPDATE master_agent_admissions SET seated_at = COALESCE(seated_at, ?2)
             WHERE claim_hash = ?1",
            params![secret_hash(claim_id), at as i64],
        )?;
        Ok(())
    }

    /// Did this Master Agent (by principal, not by display name) admit
    /// this member?
    pub fn admitted_by_principal(&self, member_id: &str, principal: &str) -> bool {
        let Some(c) = self.conn() else {
            return false;
        };
        c.query_row(
            "SELECT EXISTS(SELECT 1 FROM master_agent_admissions
                           WHERE member_id = ?1 AND principal_id = ?2)",
            params![member_id, principal],
            |r| r.get::<_, bool>(0),
        )
        .unwrap_or(false)
    }

    /// A Master Agent's admissions, by its principal: agents it admitted
    /// that are still members, and admissions since `since`. A storage
    /// error counts as the limit reached.
    pub fn admissions_by(&self, principal: &str, since: u64) -> (u32, u32) {
        let Some(c) = self.conn() else {
            return (u32::MAX, u32::MAX);
        };
        let count = |sql: &str, args: &[&dyn rusqlite::ToSql]| -> u32 {
            c.query_row(sql, args, |r| r.get::<_, i64>(0))
                .map(|n| u32::try_from(n).unwrap_or(u32::MAX))
                .unwrap_or(u32::MAX)
        };
        let live = count(
            "SELECT COUNT(*) FROM master_agent_admissions a JOIN members m ON m.id = a.member_id
             WHERE a.principal_id = ?1 AND m.revoked_at IS NULL",
            &[&principal],
        );
        let recent = count(
            "SELECT COUNT(*) FROM master_agent_admissions WHERE principal_id = ?1 AND created_at >= ?2",
            &[&principal, &(since as i64)],
        );
        (live, recent)
    }
}
