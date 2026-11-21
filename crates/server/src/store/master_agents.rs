//! The Master Agent registry and the Building's Master Agent setting.
//!
//! A Master Agent is an automation the Master appoints: its own principal
//! (`kind = 'agent'`, `building_role = 'master_agent'`), one credential, and a
//! row here naming who appointed it and when. Whether any Master Agent may act
//! at all, on which floors and within which limits, is one row of
//! `building_settings` (key `master_agent`, off by default) that only the
//! Master changes; every change of it is written to `building_settings_log`.

use rusqlite::{params, Connection, OptionalExtension};

use super::{hashed_id, secret_hash, Store};

pub(super) const SCHEMA: &str = r#"
    CREATE TABLE IF NOT EXISTS master_agents (
        management_id TEXT PRIMARY KEY,
        secret_hash TEXT NOT NULL UNIQUE,
        principal_id TEXT NOT NULL UNIQUE REFERENCES principals(id),
        credential_id TEXT NOT NULL UNIQUE REFERENCES credentials(id),
        fingerprint TEXT NOT NULL,
        name TEXT NOT NULL,
        appointed_by TEXT NOT NULL,
        appointed_at INTEGER NOT NULL,
        revoked_at INTEGER
    );
    CREATE TABLE IF NOT EXISTS building_settings_log (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        key TEXT NOT NULL,
        value TEXT NOT NULL,
        by_principal TEXT,
        at INTEGER NOT NULL
    );
"#;

/// The `building_settings` key of the Master Agent setting.
pub const MASTER_AGENT_SETTING: &str = "master_agent";

/// The role list of `principals` before the Master Agent rank, and after.
const OLD_ROLES: &str = "IN ('master', 'smaster', 'member')";
const NEW_ROLES: &str = "IN ('master', 'smaster', 'member', 'master_agent')";

/// Widen an older `principals` table's role CHECK to admit `master_agent`.
///
/// SQLite cannot change a CHECK in place, so the table is rebuilt: the new
/// table is the old one's own definition (every column a later version added
/// included) with only the role list widened, the rows are copied column for
/// column, the old table is dropped and the new one takes its name, all in
/// one transaction with foreign keys off (the tables that reference
/// `principals(id)` keep referring to it by name), and
/// `PRAGMA foreign_key_check` must find no row the rebuild left without its
/// principal before the commit. Any failure rolls all of it back and the
/// open fails with that error; a definition that does not hold the old role
/// list exactly is not guessed at.
///
/// An older server can still open the result: the wider CHECK accepts every
/// row it writes, and it reads a `master_agent` role as an ordinary member's
/// (see docs/backup-restore.md for revoking Master Agents before a rollback).
/// Returns whether the table was rebuilt.
pub(super) fn widen_principal_roles(conn: &mut Connection) -> rusqlite::Result<bool> {
    let sql: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'principals'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let Some(sql) = sql else {
        return Ok(false);
    };
    if sql.contains("'master_agent'") {
        return Ok(false);
    }
    conn.execute_batch("PRAGMA foreign_keys = OFF")?;
    let rebuilt = rebuild_principals(conn, &sql);
    let restored = conn.execute_batch("PRAGMA foreign_keys = ON");
    let rows = rebuilt?;
    restored?;
    tracing::warn!(
        principals = rows,
        "principals may now hold the master_agent role (table rebuilt once)"
    );
    Ok(true)
}

fn rebuild_principals(conn: &mut Connection, sql: &str) -> rusqlite::Result<usize> {
    let refuse = |why: &str| {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT_CHECK),
            Some(format!("{why}; principals was left as it is")),
        )
    };
    if sql.matches(OLD_ROLES).count() != 1 {
        return Err(refuse("principals has an unexpected role CHECK"));
    }
    let widened = sql.replacen(OLD_ROLES, NEW_ROLES, 1);
    let Some(rest) = widened.strip_prefix("CREATE TABLE principals") else {
        return Err(refuse("principals has an unexpected definition"));
    };
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let before = foreign_key_violations(&tx)?;
    let columns: Vec<String> = {
        let mut st = tx.prepare("PRAGMA table_info(principals)")?;
        let names = st.query_map([], |r| r.get::<_, String>(1))?;
        names
            .map(|name| name.map(|n| format!("\"{}\"", n.replace('"', "\"\""))))
            .collect::<rusqlite::Result<_>>()?
    };
    let columns = columns.join(", ");
    tx.execute_batch(&format!("CREATE TABLE principals_widened{rest}"))?;
    let rows = tx.execute(
        &format!("INSERT INTO principals_widened ({columns}) SELECT {columns} FROM principals"),
        [],
    )?;
    tx.execute_batch(
        "DROP TABLE principals;
         ALTER TABLE principals_widened RENAME TO principals;
         CREATE UNIQUE INDEX IF NOT EXISTS principals_one_master
             ON principals(building_role) WHERE building_role = 'master' AND disabled_at IS NULL;",
    )?;
    let after = foreign_key_violations(&tx)?;
    if after > before {
        return Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY),
            Some(format!(
                "widening principals would leave {} more rows naming no principal; nothing was changed",
                after - before
            )),
        ));
    }
    tx.commit()?;
    Ok(rows)
}

/// Rows anywhere that name a row that is not there (`PRAGMA foreign_key_check`).
fn foreign_key_violations(c: &Connection) -> rusqlite::Result<usize> {
    let mut check = c.prepare("PRAGMA foreign_key_check")?;
    let mut rows = check.query([])?;
    let mut n = 0;
    while rows.next()?.is_some() {
        n += 1;
    }
    Ok(n)
}

/// What the registry keeps about one Master Agent key, derived from the key
/// while it is in hand (when it is minted). The key itself is never stored.
pub(crate) struct MasterAgentIds {
    pub secret_hash: String,
    pub principal_id: String,
    pub management_id: String,
    pub credential_id: String,
    pub fingerprint: String,
}

impl MasterAgentIds {
    pub(crate) fn from_code(code: &str) -> Self {
        let source = format!("master_agent:{code}");
        Self {
            secret_hash: secret_hash(code),
            principal_id: hashed_id("pr_", &source),
            management_id: hashed_id("maid_", &source),
            credential_id: hashed_id("cr_", &source),
            fingerprint: super::keys::fingerprint(code),
        }
    }
}

/// One Master Agent as the Master sees it. Never the key.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MasterAgentRecord {
    pub id: String,
    pub principal_id: String,
    pub name: String,
    pub fingerprint: String,
    pub appointed_by: String,
    pub appointed_at: u64,
    pub revoked_at: Option<u64>,
    pub last_used_at: Option<u64>,
}

/// The Master Agent setting: whether the feature is on, the floors the
/// Master designated for it, and the limits it acts within.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct MasterAgentSetting {
    pub on: bool,
    #[serde(default)]
    pub floors: Vec<String>,
    #[serde(default)]
    pub limits: MasterAgentLimits,
}

/// What each Master Agent may do at most. The Master adjusts them; a value
/// it leaves out keeps its default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct MasterAgentLimits {
    /// Locas a Master Agent may have open on one floor.
    pub locas_per_floor: u32,
    /// Locas a Master Agent may open in any hour.
    pub locas_per_hour: u32,
    /// Agents a Master Agent may admit in any hour.
    pub admissions_per_hour: u32,
    /// Live agents (members not revoked) a Master Agent admitted.
    pub live_agents: u32,
}

impl MasterAgentLimits {
    /// Each limit's highest value the Master may set.
    pub const CEILING: u32 = 1000;
}

impl Default for MasterAgentLimits {
    fn default() -> Self {
        Self {
            locas_per_floor: 20,
            locas_per_hour: 10,
            admissions_per_hour: 30,
            live_agents: 50,
        }
    }
}

/// One change of a Building setting.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SettingChange {
    pub value: serde_json::Value,
    pub by: Option<String>,
    pub at: u64,
}

impl Store {
    /// Appoint a Master Agent: its principal, its credential and its registry
    /// row, in one transaction.
    pub fn add_master_agent(
        &self,
        code: &str,
        name: &str,
        appointed_by: &str,
        at: u64,
    ) -> rusqlite::Result<MasterAgentRecord> {
        let ids = MasterAgentIds::from_code(code);
        let Some(mut c) = self.conn() else {
            return Err(rusqlite::Error::InvalidQuery);
        };
        let tx = c.transaction()?;
        super::identity::insert_principal_credential(
            &tx,
            "master_agent",
            code,
            name,
            "agent",
            at,
            "Master Agent access",
        )?;
        tx.execute(
            "INSERT INTO master_agents
             (management_id, secret_hash, principal_id, credential_id, fingerprint,
              name, appointed_by, appointed_at, revoked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL)",
            params![
                ids.management_id,
                ids.secret_hash,
                ids.principal_id,
                ids.credential_id,
                ids.fingerprint,
                name,
                appointed_by,
                at
            ],
        )?;
        tx.commit()?;
        Ok(MasterAgentRecord {
            id: ids.management_id,
            principal_id: ids.principal_id,
            name: name.to_string(),
            fingerprint: ids.fingerprint,
            appointed_by: appointed_by.to_string(),
            appointed_at: at,
            revoked_at: None,
            last_used_at: None,
        })
    }

    /// Every Master Agent ever appointed, newest first.
    pub fn master_agents(&self) -> rusqlite::Result<Vec<MasterAgentRecord>> {
        let Some(c) = self.conn() else {
            return Ok(Vec::new());
        };
        let mut st = c.prepare(
            "SELECT m.management_id, m.principal_id, m.name, m.fingerprint,
                    m.appointed_by, m.appointed_at, m.revoked_at, cr.last_used_at
             FROM master_agents m
             LEFT JOIN credentials cr ON cr.id = m.credential_id
             ORDER BY m.appointed_at DESC, m.management_id",
        )?;
        let rows = st.query_map([], |r| {
            Ok(MasterAgentRecord {
                id: r.get(0)?,
                principal_id: r.get(1)?,
                name: r.get(2)?,
                fingerprint: r.get(3)?,
                appointed_by: r.get(4)?,
                appointed_at: r.get::<_, i64>(5)? as u64,
                revoked_at: r.get::<_, Option<i64>>(6)?.map(|v| v as u64),
                last_used_at: r.get::<_, Option<i64>>(7)?.map(|v| v as u64),
            })
        })?;
        rows.collect()
    }

    /// Revoke a Master Agent by its management id: the registry row, its
    /// credentials, its sessions and its principal end in one transaction.
    /// Returns the principal it ended, or `None` if no live one has that id.
    pub fn revoke_master_agent(&self, id: &str, at: u64) -> rusqlite::Result<Option<String>> {
        let Some(mut c) = self.conn() else {
            return Ok(None);
        };
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let principal: Option<String> = tx
            .query_row(
                "SELECT principal_id FROM master_agents
                 WHERE management_id = ?1 AND revoked_at IS NULL",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(principal) = principal else {
            return Ok(None);
        };
        tx.execute(
            "UPDATE master_agents SET revoked_at = ?2 WHERE management_id = ?1",
            params![id, at],
        )?;
        tx.execute(
            "UPDATE credentials SET revoked_at = ?2 WHERE principal_id = ?1 AND revoked_at IS NULL",
            params![principal, at],
        )?;
        tx.execute(
            "UPDATE principal_sessions SET revoked_at = ?2
             WHERE principal_id = ?1 AND revoked_at IS NULL",
            params![principal, at],
        )?;
        tx.execute(
            "UPDATE principals SET disabled_at = ?2 WHERE id = ?1 AND disabled_at IS NULL",
            params![principal, at],
        )?;
        tx.commit()?;
        Ok(Some(principal))
    }

    /// The Master Agent setting as stored; off with no floors when it was
    /// never set (or cannot be read: the feature then stays off).
    pub fn master_agent_setting(&self) -> MasterAgentSetting {
        let Some(c) = self.conn() else {
            return MasterAgentSetting::default();
        };
        c.query_row(
            "SELECT value FROM building_settings WHERE key = ?1",
            params![MASTER_AGENT_SETTING],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_str(&value).ok())
        .unwrap_or_default()
    }

    /// Save the Master Agent setting and journal the change, together.
    pub fn save_master_agent_setting(
        &self,
        setting: &MasterAgentSetting,
        by: Option<&str>,
        at: u64,
    ) -> rusqlite::Result<()> {
        let Some(mut c) = self.conn() else {
            return Err(rusqlite::Error::InvalidQuery);
        };
        let value = serde_json::to_string(setting)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        let tx = c.transaction()?;
        tx.execute(
            "INSERT INTO building_settings (key, value, updated_by_principal, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(key) DO UPDATE SET
                value = excluded.value,
                updated_by_principal = excluded.updated_by_principal,
                updated_at = excluded.updated_at",
            params![MASTER_AGENT_SETTING, value, by, at],
        )?;
        tx.execute(
            "INSERT INTO building_settings_log (key, value, by_principal, at)
             VALUES (?1, ?2, ?3, ?4)",
            params![MASTER_AGENT_SETTING, value, by, at],
        )?;
        tx.commit()
    }

    /// Every change of the Master Agent setting, newest first.
    pub fn master_agent_setting_log(&self) -> rusqlite::Result<Vec<SettingChange>> {
        let Some(c) = self.conn() else {
            return Ok(Vec::new());
        };
        let mut st = c.prepare(
            "SELECT l.value, p.display_name, l.at FROM building_settings_log l
             LEFT JOIN principals p ON p.id = l.by_principal
             WHERE l.key = ?1 ORDER BY l.id DESC",
        )?;
        let rows = st.query_map(params![MASTER_AGENT_SETTING], |r| {
            let value: String = r.get(0)?;
            Ok(SettingChange {
                value: serde_json::from_str(&value).unwrap_or(serde_json::Value::Null),
                by: r.get(1)?,
                at: r.get::<_, i64>(2)? as u64,
            })
        })?;
        rows.collect()
    }
}
