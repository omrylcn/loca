//! Groups: named sets of people who share their own locas (PRINCIPLES
//! "Groups"). A loca with no row in `group_rooms` belongs to the default
//! group, so a database without these tables (or with them empty) behaves
//! exactly as before.
//!
//! Every method takes the connection once and does its whole change, audit
//! line included, in one transaction. None of them calls another
//! `self.conn()` method while holding it (`Store::conn` is not re-entrant).

use rusqlite::{params, Connection, OptionalExtension, Transaction};

use super::Store;

/// One group. `owner` is the principal who manages its membership; `None`
/// means the Master (a group the Master founded, or whose founding Smaster
/// was revoked).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GroupRecord {
    pub id: String,
    pub name: String,
    pub founder: String,
    pub owner: Option<String>,
    pub created_at: u64,
    pub archived_at: Option<u64>,
    pub deleted_at: Option<u64>,
}

/// One line of a group's record: what changed, about whom or which loca, by
/// whom and when.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GroupLogEntry {
    pub action: String,
    pub subject: Option<String>,
    pub by: Option<String>,
    pub at: u64,
}

const GROUP_COLUMNS: &str = "id, name, founder, owner, created_at, archived_at, deleted_at";

fn group_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<GroupRecord> {
    Ok(GroupRecord {
        id: r.get(0)?,
        name: r.get(1)?,
        founder: r.get(2)?,
        owner: r.get(3)?,
        created_at: r.get::<_, i64>(4)? as u64,
        archived_at: r.get::<_, Option<i64>>(5)?.map(|v| v as u64),
        deleted_at: r.get::<_, Option<i64>>(6)?.map(|v| v as u64),
    })
}

pub(super) const SCHEMA: &str = r#"
    CREATE TABLE IF NOT EXISTS groups (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        founder TEXT NOT NULL,
        owner TEXT,
        created_at INTEGER NOT NULL,
        archived_at INTEGER,
        deleted_at INTEGER
    );
    CREATE TABLE IF NOT EXISTS group_members (
        group_id TEXT NOT NULL REFERENCES groups(id),
        principal_id TEXT NOT NULL,
        added_by TEXT,
        added_at INTEGER NOT NULL,
        PRIMARY KEY (group_id, principal_id)
    );
    CREATE TABLE IF NOT EXISTS group_rooms (
        room TEXT PRIMARY KEY,
        group_id TEXT NOT NULL REFERENCES groups(id),
        set_by TEXT,
        set_at INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS group_log (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        group_id TEXT NOT NULL,
        action TEXT NOT NULL,
        subject TEXT,
        by_principal TEXT,
        at INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS group_log_group ON group_log(group_id, id);
"#;

pub(super) fn log(
    c: &Connection,
    group: &str,
    action: &str,
    subject: Option<&str>,
    by: Option<&str>,
    at: u64,
) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO group_log (group_id, action, subject, by_principal, at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![group, action, subject, by, at as i64],
    )?;
    Ok(())
}

/// Tie (or untie, with `None`) a room to a group inside an open transaction,
/// recording it on both sides. Used by loca creation so the room never exists
/// on disk outside its group.
pub(super) fn write_room_group(
    tx: &Transaction<'_>,
    room: &str,
    group: Option<&str>,
    by: Option<&str>,
    at: u64,
) -> rusqlite::Result<()> {
    let previous: Option<String> = tx
        .query_row(
            "SELECT group_id FROM group_rooms WHERE room = ?1",
            params![room],
            |r| r.get(0),
        )
        .optional()?;
    if previous.as_deref() == group {
        return Ok(());
    }
    if let Some(previous) = previous.as_deref() {
        tx.execute("DELETE FROM group_rooms WHERE room = ?1", params![room])?;
        log(tx, previous, "loca_left", Some(room), by, at)?;
    }
    if let Some(group) = group {
        tx.execute(
            "INSERT INTO group_rooms (room, group_id, set_by, set_at) VALUES (?1, ?2, ?3, ?4)",
            params![room, group, by, at as i64],
        )?;
        log(tx, group, "loca_joined", Some(room), by, at)?;
    }
    Ok(())
}

impl Store {
    pub fn load_groups(&self) -> rusqlite::Result<Vec<GroupRecord>> {
        let Some(c) = self.conn() else {
            return Ok(Vec::new());
        };
        let mut st = c.prepare(&format!("SELECT {GROUP_COLUMNS} FROM groups"))?;
        let rows = st.query_map([], group_row)?;
        rows.collect()
    }

    /// Every membership as (group id, principal id).
    pub fn load_group_members(&self) -> rusqlite::Result<Vec<(String, String)>> {
        let Some(c) = self.conn() else {
            return Ok(Vec::new());
        };
        let mut st = c.prepare("SELECT group_id, principal_id FROM group_members")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    }

    /// Every group loca as (room, group id).
    pub fn load_group_rooms(&self) -> rusqlite::Result<Vec<(String, String)>> {
        let Some(c) = self.conn() else {
            return Ok(Vec::new());
        };
        let mut st = c.prepare("SELECT room, group_id FROM group_rooms")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    }

    /// A new group; a founding Smaster is its first member.
    pub fn create_group(
        &self,
        g: &GroupRecord,
        first_member: Option<&str>,
        by: Option<&str>,
    ) -> rusqlite::Result<()> {
        let Some(mut c) = self.conn() else {
            return Ok(());
        };
        let tx = c.transaction()?;
        tx.execute(
            &format!(
                "INSERT INTO groups ({GROUP_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL)"
            ),
            params![g.id, g.name, g.founder, g.owner, g.created_at as i64],
        )?;
        log(&tx, &g.id, "created", Some(&g.name), by, g.created_at)?;
        if let Some(member) = first_member {
            tx.execute(
                "INSERT OR IGNORE INTO group_members (group_id, principal_id, added_by, added_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![g.id, member, by, g.created_at as i64],
            )?;
            log(&tx, &g.id, "member_added", Some(member), by, g.created_at)?;
        }
        tx.commit()
    }

    /// Persist a group's name, owner and archive/delete marks, with one audit
    /// line saying what changed.
    pub fn save_group(
        &self,
        g: &GroupRecord,
        action: &str,
        subject: Option<&str>,
        by: Option<&str>,
        at: u64,
    ) -> rusqlite::Result<()> {
        let Some(mut c) = self.conn() else {
            return Ok(());
        };
        let tx = c.transaction()?;
        tx.execute(
            "UPDATE groups SET name = ?2, owner = ?3, archived_at = ?4, deleted_at = ?5
             WHERE id = ?1",
            params![
                g.id,
                g.name,
                g.owner,
                g.archived_at.map(|v| v as i64),
                g.deleted_at.map(|v| v as i64)
            ],
        )?;
        if g.deleted_at.is_some() {
            let members: Vec<String> = {
                let mut st =
                    tx.prepare("SELECT principal_id FROM group_members WHERE group_id = ?1")?;
                let rows = st.query_map(params![g.id], |r| r.get(0))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            for member in members {
                log(&tx, &g.id, "member_removed", Some(&member), by, at)?;
            }
            tx.execute(
                "DELETE FROM group_members WHERE group_id = ?1",
                params![g.id],
            )?;
        }
        log(&tx, &g.id, action, subject, by, at)?;
        tx.commit()
    }

    pub fn add_group_member(
        &self,
        group: &str,
        principal: &str,
        by: Option<&str>,
        at: u64,
    ) -> rusqlite::Result<()> {
        let Some(mut c) = self.conn() else {
            return Ok(());
        };
        let tx = c.transaction()?;
        let added = tx.execute(
            "INSERT OR IGNORE INTO group_members (group_id, principal_id, added_by, added_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![group, principal, by, at as i64],
        )?;
        if added > 0 {
            log(&tx, group, "member_added", Some(principal), by, at)?;
        }
        tx.commit()
    }

    pub fn remove_group_member(
        &self,
        group: &str,
        principal: &str,
        by: Option<&str>,
        at: u64,
    ) -> rusqlite::Result<()> {
        let Some(mut c) = self.conn() else {
            return Ok(());
        };
        let tx = c.transaction()?;
        let removed = tx.execute(
            "DELETE FROM group_members WHERE group_id = ?1 AND principal_id = ?2",
            params![group, principal],
        )?;
        if removed > 0 {
            log(&tx, group, "member_removed", Some(principal), by, at)?;
        }
        tx.commit()
    }

    /// Move a room between the default group (`None`) and a group.
    pub fn set_room_group(
        &self,
        room: &str,
        group: Option<&str>,
        by: Option<&str>,
        at: u64,
    ) -> rusqlite::Result<()> {
        let Some(mut c) = self.conn() else {
            return Ok(());
        };
        let tx = c.transaction()?;
        write_room_group(&tx, room, group, by, at)?;
        tx.commit()
    }

    /// The group's record, oldest first.
    pub fn group_log(&self, group: &str) -> rusqlite::Result<Vec<GroupLogEntry>> {
        let Some(c) = self.conn() else {
            return Ok(Vec::new());
        };
        let mut st = c.prepare(
            "SELECT action, subject, by_principal, at FROM group_log
             WHERE group_id = ?1 ORDER BY id",
        )?;
        let rows = st.query_map(params![group], |r| {
            Ok(GroupLogEntry {
                action: r.get(0)?,
                subject: r.get(1)?,
                by: r.get(2)?,
                at: r.get::<_, i64>(3)? as u64,
            })
        })?;
        rows.collect()
    }
}
