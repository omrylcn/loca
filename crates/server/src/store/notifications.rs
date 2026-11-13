//! Notifications: a Building-level announcement sent to people. Not a chat
//! message and not a loca message (docs/concepts.md "Notifications").
//!
//! The audience is resolved when the notification is sent and frozen as one
//! receipt row per recipient, so a person who joins a group later never sees
//! an older notification. Every method takes the connection once and never
//! calls another `self.conn()` method while holding it (`Store::conn` is not
//! re-entrant).

use rusqlite::{params, OptionalExtension};

use super::Store;

pub(super) const SCHEMA: &str = r#"
    CREATE TABLE IF NOT EXISTS notifications (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        sender_principal TEXT NOT NULL,
        sender_name TEXT NOT NULL,
        audience TEXT NOT NULL,
        title TEXT NOT NULL,
        body TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS notifications_sender
        ON notifications(sender_principal, id);
    CREATE TABLE IF NOT EXISTS notification_receipts (
        notification_id INTEGER NOT NULL REFERENCES notifications(id),
        principal_id TEXT NOT NULL,
        read_at INTEGER,
        PRIMARY KEY (notification_id, principal_id)
    );
    CREATE INDEX IF NOT EXISTS notification_receipts_principal
        ON notification_receipts(principal_id, notification_id);
"#;

/// One notification as its recipient sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationRow {
    pub id: i64,
    pub sender_name: String,
    /// The audience as stored (JSON text), resolved for display by the hub.
    pub audience: String,
    pub title: String,
    pub body: String,
    pub created_at: u64,
    pub read_at: Option<u64>,
}

/// One notification as its sender sees it: counts, never names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentNotificationRow {
    pub id: i64,
    pub audience: String,
    pub title: String,
    pub body: String,
    pub created_at: u64,
    pub recipients: u64,
    pub read: u64,
}

/// Which page of a person's notifications to read.
#[derive(Debug, Clone, Copy, Default)]
pub struct NotificationPage {
    /// Only notifications newer than this id (the live poll).
    pub since: Option<i64>,
    /// Only notifications older than this id (paging back).
    pub before: Option<i64>,
    pub limit: u32,
}

impl Store {
    /// Persist a notification and one unread receipt per recipient in one
    /// transaction. Returns the new id.
    #[allow(clippy::too_many_arguments)]
    pub fn create_notification(
        &self,
        sender_principal: &str,
        sender_name: &str,
        audience: &str,
        title: &str,
        body: &str,
        created_at: u64,
        recipients: &[String],
    ) -> rusqlite::Result<Option<i64>> {
        let Some(mut c) = self.conn() else {
            return Ok(None);
        };
        let tx = c.transaction()?;
        tx.execute(
            "INSERT INTO notifications
                (sender_principal, sender_name, audience, title, body, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                sender_principal,
                sender_name,
                audience,
                title,
                body,
                created_at as i64
            ],
        )?;
        let id = tx.last_insert_rowid();
        {
            let mut st = tx.prepare(
                "INSERT OR IGNORE INTO notification_receipts
                    (notification_id, principal_id, read_at) VALUES (?1, ?2, NULL)",
            )?;
            for principal in recipients {
                st.execute(params![id, principal])?;
            }
        }
        tx.commit()?;
        Ok(Some(id))
    }

    /// A person's notifications, newest first, plus their unread count.
    pub fn notifications_for(
        &self,
        principal: &str,
        page: NotificationPage,
    ) -> rusqlite::Result<(Vec<NotificationRow>, u64)> {
        let Some(c) = self.conn() else {
            return Ok((Vec::new(), 0));
        };
        let mut st = c.prepare(
            "SELECT n.id, n.sender_name, n.audience, n.title, n.body, n.created_at, r.read_at
             FROM notification_receipts r
             JOIN notifications n ON n.id = r.notification_id
             WHERE r.principal_id = ?1
               AND (?2 IS NULL OR n.id > ?2)
               AND (?3 IS NULL OR n.id < ?3)
             ORDER BY n.id DESC
             LIMIT ?4",
        )?;
        let rows = st.query_map(
            params![principal, page.since, page.before, page.limit as i64],
            |r| {
                Ok(NotificationRow {
                    id: r.get(0)?,
                    sender_name: r.get(1)?,
                    audience: r.get(2)?,
                    title: r.get(3)?,
                    body: r.get(4)?,
                    created_at: r.get::<_, i64>(5)? as u64,
                    read_at: r.get::<_, Option<i64>>(6)?.map(|v| v as u64),
                })
            },
        )?;
        let list = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        let unread: i64 = c.query_row(
            "SELECT COUNT(*) FROM notification_receipts
             WHERE principal_id = ?1 AND read_at IS NULL",
            params![principal],
            |r| r.get(0),
        )?;
        Ok((list, unread.max(0) as u64))
    }

    /// Mark one of a person's own notifications read. `false` when the person
    /// never received it (the caller answers as for a missing one).
    pub fn mark_notification_read(
        &self,
        id: i64,
        principal: &str,
        at: u64,
    ) -> rusqlite::Result<bool> {
        let Some(c) = self.conn() else {
            return Ok(false);
        };
        let exists: Option<Option<i64>> = c
            .query_row(
                "SELECT read_at FROM notification_receipts
                 WHERE notification_id = ?1 AND principal_id = ?2",
                params![id, principal],
                |r| r.get(0),
            )
            .optional()?;
        match exists {
            None => Ok(false),
            Some(Some(_)) => Ok(true),
            Some(None) => {
                c.execute(
                    "UPDATE notification_receipts SET read_at = ?3
                     WHERE notification_id = ?1 AND principal_id = ?2 AND read_at IS NULL",
                    params![id, principal, at as i64],
                )?;
                Ok(true)
            }
        }
    }

    /// Mark every unread notification of one person read; returns how many.
    pub fn mark_all_notifications_read(&self, principal: &str, at: u64) -> rusqlite::Result<u64> {
        let Some(c) = self.conn() else {
            return Ok(0);
        };
        let changed = c.execute(
            "UPDATE notification_receipts SET read_at = ?2
             WHERE principal_id = ?1 AND read_at IS NULL",
            params![principal, at as i64],
        )?;
        Ok(changed as u64)
    }

    /// What one sender sent, newest first, with recipient and read counts.
    pub fn sent_notifications(
        &self,
        sender: &str,
        before: Option<i64>,
        limit: u32,
    ) -> rusqlite::Result<Vec<SentNotificationRow>> {
        let Some(c) = self.conn() else {
            return Ok(Vec::new());
        };
        let mut st = c.prepare(
            "SELECT n.id, n.audience, n.title, n.body, n.created_at,
                    (SELECT COUNT(*) FROM notification_receipts r
                      WHERE r.notification_id = n.id),
                    (SELECT COUNT(*) FROM notification_receipts r
                      WHERE r.notification_id = n.id AND r.read_at IS NOT NULL)
             FROM notifications n
             WHERE n.sender_principal = ?1 AND (?2 IS NULL OR n.id < ?2)
             ORDER BY n.id DESC
             LIMIT ?3",
        )?;
        let rows = st.query_map(params![sender, before, limit as i64], |r| {
            Ok(SentNotificationRow {
                id: r.get(0)?,
                audience: r.get(1)?,
                title: r.get(2)?,
                body: r.get(3)?,
                created_at: r.get::<_, i64>(4)? as u64,
                recipients: r.get::<_, i64>(5)?.max(0) as u64,
                read: r.get::<_, i64>(6)?.max(0) as u64,
            })
        })?;
        rows.collect()
    }
}
