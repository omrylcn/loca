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
    CREATE INDEX IF NOT EXISTS notifications_sender_time
        ON notifications(sender_principal, created_at);
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
    /// Everyone it went to: people and agents.
    pub recipients: u64,
    /// How many of those recipients are agents.
    pub agents: u64,
    /// How many people read it. An agent's mark is not a read.
    pub read: u64,
    /// How many recipients' own clients fetched it: it reached them.
    pub delivered: u64,
    /// Who sent it, as named then.
    pub sender_name: String,
}

/// One recipient of a notification, as its sender (or the Master) sees it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct NotificationRecipientRow {
    pub name: String,
    /// `agent` or `human`.
    pub kind: String,
    /// When its own client first fetched the notification.
    pub delivered_at: Option<u64>,
    /// When a person marked it read; never set for an agent's list.
    pub read_at: Option<u64>,
}

/// Which page of a person's notifications to read.
#[derive(Debug, Clone, Copy, Default)]
pub struct NotificationPage {
    /// Only notifications newer than this id (the live poll).
    pub since: Option<i64>,
    /// Only notifications older than this id (paging back).
    pub before: Option<i64>,
    pub limit: u32,
    /// Agent recovery consumes oldest unseen first so its durable cursor
    /// never jumps over an unreceived page. Browsers retain newest-first.
    pub oldest_first: bool,
}

/// What sending stored: the new id and who it went to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreatedNotification {
    pub id: i64,
    /// Receipts written: people and agents.
    pub recipients: u64,
    /// How many of those receipts belong to agents.
    pub agents: u64,
}

/// How many notifications one sender may have stored within a window.
#[derive(Debug, Clone, Copy)]
pub struct SendLimit<'a> {
    /// Every `sender_principal` this sender's earlier sends may be stored
    /// under; they share one count. Empty counts `sender_principal` alone.
    pub senders: &'a [String],
    pub max: u32,
    pub window_ms: u64,
}

/// The outcome of a send that reached the database.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationSend {
    Created(CreatedNotification),
    /// The sender already has `max` notifications in the window; nothing was
    /// written. A slot frees in `retry_after_ms`, when the oldest of the
    /// counted ones leaves the window.
    Limited {
        retry_after_ms: u64,
    },
}

impl Store {
    /// Persist a notification and one unread receipt per recipient in one
    /// transaction, and count in that same transaction who it went to.
    ///
    /// The sending limit is checked in that same transaction, from the stored
    /// notifications themselves: a restart does not reset it, and the
    /// transaction is IMMEDIATE (the write lock is taken before the count), so
    /// two sends racing for the last slot cannot both see it free. A refused
    /// send writes nothing.
    ///
    /// The counts are part of the answer to the sender, so they are read
    /// before the commit: a failure here rolls the notification back, and the
    /// error says truthfully that nothing was sent. Reading them afterwards
    /// could fail with the notification already stored and announced, and a
    /// sender told "failed" sends it again.
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
        limit: SendLimit<'_>,
    ) -> rusqlite::Result<Option<NotificationSend>> {
        let Some(mut c) = self.conn() else {
            return Ok(None);
        };
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        // The `max` newest of this sender's notifications still inside the
        // window. When there are `max` of them, the oldest decides when the
        // next send fits: the moment it is `window_ms` old.
        let placeholders = vec!["?"; limit.senders.len().max(1)].join(", ");
        let in_window: Vec<i64> = {
            let mut st = tx.prepare(&format!(
                "SELECT created_at FROM notifications
                  WHERE sender_principal IN ({placeholders}) AND created_at > ?
                  ORDER BY created_at DESC
                  LIMIT ?"
            ))?;
            let mut args: Vec<rusqlite::types::Value> = if limit.senders.is_empty() {
                vec![rusqlite::types::Value::Text(sender_principal.to_string())]
            } else {
                limit
                    .senders
                    .iter()
                    .map(|s| rusqlite::types::Value::Text(s.clone()))
                    .collect()
            };
            args.push(rusqlite::types::Value::Integer(
                created_at.saturating_sub(limit.window_ms) as i64,
            ));
            args.push(rusqlite::types::Value::Integer(i64::from(limit.max)));
            let rows = st.query_map(rusqlite::params_from_iter(args), |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        if in_window.len() as u64 >= u64::from(limit.max) {
            let oldest = in_window.last().copied().unwrap_or(0).max(0) as u64;
            let retry_after_ms = oldest
                .saturating_add(limit.window_ms)
                .saturating_sub(created_at)
                .max(1);
            return Ok(Some(NotificationSend::Limited { retry_after_ms }));
        }
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
        let (written, agents): (i64, i64) = tx.query_row(
            "SELECT COUNT(*), COALESCE(SUM(p.kind = 'agent'), 0)
               FROM notification_receipts r
               LEFT JOIN principals p ON p.id = r.principal_id
              WHERE r.notification_id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        tx.commit()?;
        Ok(Some(NotificationSend::Created(CreatedNotification {
            id,
            recipients: written.max(0) as u64,
            agents: agents.max(0) as u64,
        })))
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
        let sql = format!(
            "SELECT n.id, n.sender_name, n.audience, n.title, n.body, n.created_at, r.read_at
             FROM notification_receipts r
             JOIN notifications n ON n.id = r.notification_id
             WHERE r.principal_id = ?1
               AND (?2 IS NULL OR n.id > ?2)
               AND (?3 IS NULL OR n.id < ?3)
             ORDER BY n.id {}
             LIMIT ?4",
            if page.oldest_first { "ASC" } else { "DESC" },
        );
        let mut st = c.prepare(&sql)?;
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
    ///
    /// "Read" counts people only. For an agent a notification is context: it
    /// is not recorded as having read it (PRINCIPLES, "Building
    /// notifications"), and an agent that marks its own list read has cleared
    /// a list, not read a notice. Counting agents made "2 of 5 read" a number
    /// that could never reach 5. A recipient whose principal is unknown is
    /// counted as a person, the kind every principal has unless it says agent.
    pub fn sent_notifications(
        &self,
        sender: Option<&str>,
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
                       JOIN principals p ON p.id = r.principal_id
                      WHERE r.notification_id = n.id AND p.kind = 'agent'),
                    (SELECT COUNT(*) FROM notification_receipts r
                       LEFT JOIN principals p ON p.id = r.principal_id
                      WHERE r.notification_id = n.id AND r.read_at IS NOT NULL
                        AND COALESCE(p.kind, 'human') <> 'agent'),
                    (SELECT COUNT(*) FROM notification_receipts r
                      WHERE r.notification_id = n.id AND r.delivered_at IS NOT NULL),
                    n.sender_name
             FROM notifications n
             WHERE (?1 IS NULL OR n.sender_principal = ?1) AND (?2 IS NULL OR n.id < ?2)
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
                agents: r.get::<_, i64>(6)?.max(0) as u64,
                read: r.get::<_, i64>(7)?.max(0) as u64,
                delivered: r.get::<_, i64>(8)?.max(0) as u64,
                sender_name: r.get(9)?,
            })
        })?;
        rows.collect()
    }

    /// Who sent notification `id` (principal), or `None` when there is none.
    pub fn notification_sender(&self, id: i64) -> rusqlite::Result<Option<String>> {
        let Some(c) = self.conn() else {
            return Ok(None);
        };
        c.query_row(
            "SELECT sender_principal FROM notifications WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .optional()
    }

    /// Every recipient of notification `id`: name, kind, and when it
    /// reached them and (for a person) was read. Agents first, then people,
    /// by name.
    pub fn notification_recipients(
        &self,
        id: i64,
    ) -> rusqlite::Result<Vec<NotificationRecipientRow>> {
        let Some(c) = self.conn() else {
            return Ok(Vec::new());
        };
        let mut st = c.prepare(
            "SELECT COALESCE(p.display_name, r.principal_id), COALESCE(p.kind, 'human'),
                    r.delivered_at, r.read_at
             FROM notification_receipts r
             LEFT JOIN principals p ON p.id = r.principal_id
             WHERE r.notification_id = ?1
             ORDER BY CASE COALESCE(p.kind, 'human') WHEN 'agent' THEN 0 ELSE 1 END,
                      COALESCE(p.display_name, r.principal_id)",
        )?;
        let rows = st.query_map(params![id], |r| {
            let kind: String = r.get(1)?;
            Ok(NotificationRecipientRow {
                name: r.get(0)?,
                kind: if kind == "agent" {
                    "agent".into()
                } else {
                    "human".into()
                },
                delivered_at: r.get::<_, Option<i64>>(2)?.map(|v| v as u64),
                read_at: r.get::<_, Option<i64>>(3)?.map(|v| v as u64),
            })
        })?;
        rows.collect()
    }

    /// The recipient's own client fetched these notifications: they reached
    /// it. Only the first fetch is kept.
    pub fn mark_notifications_delivered(
        &self,
        principal: &str,
        ids: &[i64],
        at: u64,
    ) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else {
            return Ok(());
        };
        for id in ids {
            c.execute(
                "UPDATE notification_receipts SET delivered_at = ?3
                 WHERE notification_id = ?1 AND principal_id = ?2 AND delivered_at IS NULL",
                params![id, principal, at as i64],
            )?;
        }
        Ok(())
    }
}
