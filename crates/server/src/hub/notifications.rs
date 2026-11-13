//! Notifications: a Building-level announcement sent to people
//! (docs/concepts.md "Notifications"). Not a chat message, not a loca message.
//!
//! Who sends: the Master, to everyone or to any group; a Smaster, only to
//! groups it founded or belongs to. Members and agents never send.
//!
//! The audience is resolved at send time and frozen as one receipt per
//! recipient. Agents among the recipients also get a `notification` frame on
//! their Lobby line; browsers pick new ones up with the health poll.
//!
//! Lock discipline: nothing here holds a hub lock across another. Group
//! membership is read through `groups_for` (which clones out of the leaf
//! `groups` lock), and the store connection is taken once per call.

use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use protocol::SenderType;

use super::{BuildingRole, Hub, LobbyEvent, RequestAuthority};
use crate::store::{NotificationPage, NotificationRow};

pub const NOTIFICATION_TITLE_MAX: usize = 120;
pub const NOTIFICATION_BODY_MAX: usize = 2000;
const PAGE_DEFAULT: u32 = 50;
const PAGE_MAX: u32 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationError {
    /// No signed-in person behind the request.
    Unauthenticated,
    /// Members and agents never send.
    RankRequired,
    /// A Smaster with no group has nobody to notify.
    NoGroup,
    /// Only the Master notifies everyone.
    MasterOnly,
    /// No such group — or one this sender may not see; the answer is the same.
    NoSuchGroup,
    /// An archived group takes no notification.
    ArchivedGroup,
    /// Nobody chosen.
    NoAudience,
    /// The chosen audience holds nobody but the sender.
    NoRecipients,
    /// Title or body out of bounds.
    InvalidTitle,
    InvalidBody,
    /// No such notification among the caller's own.
    NotFound,
    /// A memory-only server keeps no notifications.
    Unavailable,
    Storage,
}

impl NotificationError {
    pub fn message(self) -> &'static str {
        match self {
            Self::Unauthenticated => "sign in first",
            Self::RankRequired => "only the Master or a Smaster sends notifications",
            Self::NoGroup => "you belong to no group, so there is nobody to notify",
            Self::MasterOnly => "only the Master notifies everyone",
            Self::NoSuchGroup => "no such group",
            Self::ArchivedGroup => "this group is archived",
            Self::NoAudience => "choose who receives it: everyone or one or more groups",
            Self::NoRecipients => "nobody but you is in that audience yet",
            Self::InvalidTitle => "a title is 1-120 characters of plain text on one line",
            Self::InvalidBody => "a body is at most 2000 characters of plain text",
            Self::NotFound => "no such notification",
            Self::Unavailable => "notifications need a persistent database (DB_PATH)",
            Self::Storage => "could not save — try again",
        }
    }
}

/// Who a notification is for, as the sender asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationAudience {
    All,
    Groups(Vec<String>),
}

/// One notification as its recipient reads it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct NotificationView {
    pub id: i64,
    pub title: String,
    pub body: String,
    /// The sender's name when it was sent.
    pub from: String,
    /// `{"all": true}` or `{"groups": [names]}` — only groups the reader
    /// itself can see are named.
    pub audience: serde_json::Value,
    pub created_at: u64,
    pub read: bool,
}

/// One notification as its sender reviews it: how many received and read
/// it, never who.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SentNotificationView {
    pub id: i64,
    pub title: String,
    pub body: String,
    pub audience: serde_json::Value,
    pub created_at: u64,
    pub recipients: u64,
    pub read: u64,
}

/// A group a sender may address.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AudienceGroup {
    pub id: String,
    pub name: String,
    pub members: usize,
}

/// What the composer offers this viewer.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AudienceOptions {
    pub can_send: bool,
    pub everyone: bool,
    pub groups: Vec<AudienceGroup>,
}

fn valid_title(title: &str) -> Option<String> {
    let title = title.trim();
    (!title.is_empty()
        && title.chars().count() <= NOTIFICATION_TITLE_MAX
        && !title.chars().any(char::is_control))
    .then(|| title.to_string())
}

fn valid_body(body: &str) -> Option<String> {
    let body = body.replace("\r\n", "\n");
    let body = body.trim();
    (body.chars().count() <= NOTIFICATION_BODY_MAX
        && !body
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t'))
    .then(|| body.to_string())
}

/// May this viewer send at all, and as whom?
fn sender_of(viewer: &RequestAuthority) -> Result<(String, String), NotificationError> {
    if viewer.kind == Some(SenderType::Agent) {
        return Err(NotificationError::RankRequired);
    }
    match viewer.building_role {
        Some(BuildingRole::Master) => Ok((
            viewer
                .principal_id
                .clone()
                .unwrap_or_else(|| "master".into()),
            viewer
                .display_name
                .clone()
                .unwrap_or_else(|| "Master".into()),
        )),
        Some(BuildingRole::Smaster) => {
            let id = viewer
                .principal_id
                .clone()
                .ok_or(NotificationError::RankRequired)?;
            let name = viewer
                .display_name
                .clone()
                .unwrap_or_else(|| "Smaster".into());
            Ok((id, name))
        }
        _ => Err(NotificationError::RankRequired),
    }
}

fn parse_audience(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap_or_else(|_| serde_json::json!({}))
}

impl Hub {
    /// The groups this viewer may address: every live group for the Master,
    /// the ones a Smaster founded or belongs to. Archived groups are left out.
    fn addressable_groups(&self, viewer: &RequestAuthority) -> Vec<super::GroupView> {
        self.groups_for(viewer)
            .into_iter()
            .filter(|g| !g.archived)
            .collect()
    }

    /// `GET /notifications/audiences`: what the composer offers.
    pub fn notification_audiences(&self, viewer: &RequestAuthority) -> AudienceOptions {
        let Ok(_) = sender_of(viewer) else {
            return AudienceOptions {
                can_send: false,
                everyone: false,
                groups: Vec::new(),
            };
        };
        let everyone = viewer.is_master();
        let me = viewer.principal_id.as_deref();
        let groups: Vec<AudienceGroup> = self
            .addressable_groups(viewer)
            .into_iter()
            .map(|g| AudienceGroup {
                // Who it would reach: the sender never notifies itself.
                members: g
                    .members
                    .iter()
                    .filter(|m| Some(m.principal_id.as_str()) != me)
                    .count(),
                id: g.id,
                name: g.name,
            })
            .collect();
        AudienceOptions {
            can_send: self.store.is_persistent() && (everyone || !groups.is_empty()),
            everyone,
            groups,
        }
    }

    /// Send one notification. The audience is resolved now and frozen.
    pub fn send_notification(
        &self,
        viewer: &RequestAuthority,
        title: &str,
        body: &str,
        audience: NotificationAudience,
    ) -> Result<SentNotificationView, NotificationError> {
        let (sender_id, sender_name) = sender_of(viewer)?;
        let title = valid_title(title).ok_or(NotificationError::InvalidTitle)?;
        let body = valid_body(body).ok_or(NotificationError::InvalidBody)?;
        if !self.store.is_persistent() {
            return Err(NotificationError::Unavailable);
        }
        let (recipients, stored_audience): (BTreeSet<String>, serde_json::Value) = match audience {
            NotificationAudience::All => {
                if !viewer.is_master() {
                    return Err(NotificationError::MasterOnly);
                }
                let everyone = self
                    .store
                    .active_principals()
                    .into_iter()
                    .map(|p| p.id)
                    .collect();
                (everyone, serde_json::json!({ "all": true }))
            }
            NotificationAudience::Groups(ids) => {
                let wanted: BTreeSet<String> = ids
                    .into_iter()
                    .map(|id| id.trim().to_string())
                    .filter(|id| !id.is_empty())
                    .collect();
                let mine = self.groups_for(viewer);
                if mine.is_empty() && !viewer.is_master() {
                    return Err(NotificationError::NoGroup);
                }
                if wanted.is_empty() {
                    return Err(NotificationError::NoAudience);
                }
                let mut people = BTreeSet::new();
                let mut named = Vec::new();
                for id in &wanted {
                    let group = mine
                        .iter()
                        .find(|g| &g.id == id)
                        .ok_or(NotificationError::NoSuchGroup)?;
                    if group.archived {
                        return Err(NotificationError::ArchivedGroup);
                    }
                    people.extend(group.members.iter().map(|m| m.principal_id.clone()));
                    named.push(serde_json::json!({ "id": group.id, "name": group.name }));
                }
                (people, serde_json::json!({ "groups": named }))
            }
        };
        let recipients: Vec<String> = recipients.into_iter().filter(|p| p != &sender_id).collect();
        if recipients.is_empty() {
            return Err(NotificationError::NoRecipients);
        }
        let created_at = self.now();
        let id = self
            .store
            .create_notification(
                &sender_id,
                &sender_name,
                &stored_audience.to_string(),
                &title,
                &body,
                created_at,
                &recipients,
            )
            .map_err(|_| NotificationError::Storage)?
            .ok_or(NotificationError::Unavailable)?;
        // Agents hear it on their Lobby line. The frame carries no credential
        // and no group name: one frame goes to every recipient's socket.
        let scope = if stored_audience.get("all").is_some() {
            "building"
        } else {
            "group"
        };
        let frame = serde_json::json!({
            "t": "notification",
            "id": id,
            "title": title,
            "body": body,
            "from": sender_name,
            "scope": scope,
            "created_at": created_at,
        })
        .to_string();
        let _ = self.lobby_tx.send(LobbyEvent::Notification {
            recipients: Arc::new(recipients.iter().cloned().collect::<HashSet<_>>()),
            frame: Arc::new(frame),
        });
        Ok(SentNotificationView {
            id,
            title,
            body,
            audience: stored_audience,
            created_at,
            recipients: recipients.len() as u64,
            read: 0,
        })
    }

    fn reader_of(viewer: &RequestAuthority) -> Result<String, NotificationError> {
        viewer
            .principal_id
            .clone()
            .ok_or(NotificationError::Unauthenticated)
    }

    /// The audience as this reader may see it: "everyone", or only the
    /// groups it can see itself (never another group's name).
    fn audience_for_reader(&self, stored: &str, visible: &HashSet<String>) -> serde_json::Value {
        let audience = parse_audience(stored);
        if audience.get("all").is_some() {
            return serde_json::json!({ "all": true });
        }
        let names: Vec<serde_json::Value> = audience
            .get("groups")
            .and_then(|g| g.as_array())
            .map(|groups| {
                groups
                    .iter()
                    .filter(|g| {
                        g.get("id")
                            .and_then(|id| id.as_str())
                            .is_some_and(|id| visible.contains(id))
                    })
                    .filter_map(|g| g.get("name").cloned())
                    .collect()
            })
            .unwrap_or_default();
        serde_json::json!({ "groups": names })
    }

    /// `GET /notifications`: the caller's own, newest first, and its unread
    /// count.
    pub fn notifications_of(
        &self,
        viewer: &RequestAuthority,
        since: Option<i64>,
        before: Option<i64>,
        limit: Option<u32>,
    ) -> Result<(Vec<NotificationView>, u64, bool), NotificationError> {
        let me = Self::reader_of(viewer)?;
        let limit = limit.unwrap_or(PAGE_DEFAULT).clamp(1, PAGE_MAX);
        let (rows, unread) = self
            .store
            .notifications_for(
                &me,
                NotificationPage {
                    since,
                    before,
                    // One extra row says whether an older page exists.
                    limit: limit + 1,
                },
            )
            .map_err(|_| NotificationError::Storage)?;
        let more = rows.len() > limit as usize;
        let visible: HashSet<String> = self.groups_for(viewer).into_iter().map(|g| g.id).collect();
        let list = rows
            .into_iter()
            .take(limit as usize)
            .map(|row: NotificationRow| NotificationView {
                audience: self.audience_for_reader(&row.audience, &visible),
                id: row.id,
                title: row.title,
                body: row.body,
                from: row.sender_name,
                created_at: row.created_at,
                read: row.read_at.is_some(),
            })
            .collect();
        Ok((list, unread, more))
    }

    pub fn read_notification(
        &self,
        viewer: &RequestAuthority,
        id: i64,
    ) -> Result<(), NotificationError> {
        let me = Self::reader_of(viewer)?;
        match self.store.mark_notification_read(id, &me, self.now()) {
            Ok(true) => Ok(()),
            Ok(false) => Err(NotificationError::NotFound),
            Err(_) => Err(NotificationError::Storage),
        }
    }

    pub fn read_all_notifications(
        &self,
        viewer: &RequestAuthority,
    ) -> Result<u64, NotificationError> {
        let me = Self::reader_of(viewer)?;
        self.store
            .mark_all_notifications_read(&me, self.now())
            .map_err(|_| NotificationError::Storage)
    }

    /// `GET /notifications/sent`: what this sender sent, with counts.
    pub fn sent_notifications(
        &self,
        viewer: &RequestAuthority,
        before: Option<i64>,
        limit: Option<u32>,
    ) -> Result<Vec<SentNotificationView>, NotificationError> {
        let (sender, _) = sender_of(viewer)?;
        let limit = limit.unwrap_or(PAGE_DEFAULT).clamp(1, PAGE_MAX);
        let rows = self
            .store
            .sent_notifications(&sender, before, limit)
            .map_err(|_| NotificationError::Storage)?;
        Ok(rows
            .into_iter()
            .map(|row| SentNotificationView {
                id: row.id,
                title: row.title,
                body: row.body,
                audience: parse_audience(&row.audience),
                created_at: row.created_at,
                recipients: row.recipients,
                read: row.read,
            })
            .collect())
    }
}
