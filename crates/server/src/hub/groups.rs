//! Groups: a named set of people with their own locas (PRINCIPLES "Groups").
//!
//! Everyone is in the default group: a loca with no group works exactly as
//! before, by invitation. A group loca opens only to the group's members and
//! the Master — `enter_decision` asks [`Hub::group_gate`] before any rank
//! bypass, the way it asks the birebir branch.
//!
//! Lock discipline: `groups` is a leaf lock. While it is held nothing takes
//! another hub lock (`rooms`, `invites`, `sessions`, …); only the store
//! connection, so memory and disk change together. Code that needs both
//! clones what it needs out of `groups` first. `rooms` → `groups` (as in
//! `create_loca` and `post`) is therefore the only order that ever occurs.

use std::collections::{HashMap, HashSet};

use protocol::SenderType;

use super::{BuildingRole, Hub, Invite, LobbyEvent, RequestAuthority};
use crate::store::{GroupLogEntry, GroupRecord};
use crate::sync::RecoverMutex;

/// The in-memory mirror of the group tables.
#[derive(Default)]
pub(super) struct GroupBook {
    groups: HashMap<String, GroupRecord>,
    /// group id → member principal ids.
    members: HashMap<String, HashSet<String>>,
    /// room → group id. A room absent here is a default-group loca.
    rooms: HashMap<String, String>,
}

impl GroupBook {
    fn live(&self, id: &str) -> Option<&GroupRecord> {
        self.groups.get(id).filter(|g| g.deleted_at.is_none())
    }
    fn has_member(&self, group: &str, principal: &str) -> bool {
        self.members
            .get(group)
            .is_some_and(|members| members.contains(principal))
    }
    fn rooms_of(&self, group: &str) -> Vec<String> {
        let mut rooms: Vec<String> = self
            .rooms
            .iter()
            .filter(|(_, g)| g.as_str() == group)
            .map(|(room, _)| room.clone())
            .collect();
        rooms.sort();
        rooms
    }
    /// Does this viewer see the group at all? The Master sees every group;
    /// anyone else only one they own or belong to.
    fn visible_to(&self, g: &GroupRecord, viewer: &RequestAuthority) -> bool {
        if g.deleted_at.is_some() {
            return false;
        }
        if viewer.is_master() {
            return true;
        }
        viewer
            .principal_id
            .as_deref()
            .is_some_and(|me| g.owner.as_deref() == Some(me) || self.has_member(&g.id, me))
    }
}

/// Only the Master, or the Smaster who owns the group, manages it.
fn manages(g: &GroupRecord, viewer: &RequestAuthority) -> bool {
    viewer.is_master()
        || (viewer.building_role == Some(BuildingRole::Smaster)
            && g.owner.is_some()
            && viewer.principal_id.as_deref() == g.owner.as_deref())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupError {
    /// No such group — or one this caller may not see; the answer is the same.
    NotFound,
    /// Only the founder or the Master may do this.
    NotOwner,
    /// Only the Master or a Smaster creates a group.
    RankRequired,
    InvalidName,
    /// Nobody adds themselves to a group.
    SelfAdd,
    /// No active person by that id.
    NoSuchPerson,
    /// The Master is never a member; it sees every group already.
    MasterNeverMember,
    /// The founder stays in its own group while it owns it.
    FounderStays,
    /// An archived group takes no new loca.
    Archived,
    /// A group with open locas cannot be deleted; archive it first.
    OpenLocas,
    /// Birebirs and İye are never tied to a group.
    Excluded,
    /// No such loca.
    NoSuchLoca,
    /// Only the Master moves a loca between groups.
    MasterOnly,
    /// Moving to the default group would need more davets than the loca has
    /// seats left: nobody is dropped silently, so the move does not happen.
    CarryOverCapacity,
    Storage,
}

impl GroupError {
    pub fn message(self) -> &'static str {
        match self {
            Self::NotFound => "no such group",
            Self::NotOwner => "only the group's founder or the Master manages it",
            Self::RankRequired => "only the Master or a Smaster creates a group",
            Self::InvalidName => "a group name is 1-64 characters, no control characters",
            Self::SelfAdd => "nobody adds themselves to a group",
            Self::NoSuchPerson => "nobody active has that id",
            Self::MasterNeverMember => "the Master sees every group and is never a member",
            Self::FounderStays => "the founder stays in its own group",
            Self::Archived => "this group is archived",
            Self::OpenLocas => "close the group's locas first (archive the group)",
            Self::Excluded => "a birebir or İye never belongs to a group",
            Self::NoSuchLoca => "no such loca",
            Self::MasterOnly => "only the Master moves a loca between groups",
            Self::CarryOverCapacity => {
                "this loca has too few free seats to keep everyone from its group \
                 seated by davet — nothing was moved. Release seats or remove people \
                 from the group first; the preview shows how many seats are needed"
            }
            Self::Storage => "could not save — try again",
        }
    }
}

/// What a group move would do (see `loca_group_move_preview`).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct GroupMovePreview {
    pub losing: Vec<String>,
    pub keeping: Vec<String>,
    pub carried_by_davet: Vec<String>,
    pub lead_ends: Option<String>,
    pub operator_ends: Option<String>,
    /// New davets the move must mint so carried people keep their seat.
    pub seats_needed: usize,
    /// Davets the loca can still take (its capacity minus its live davets).
    pub seats_free: usize,
    /// `seats_needed > seats_free`: the move would be refused as it stands.
    pub blocked: bool,
}

/// The davets a move to the default group has to mint (see
/// `Hub::group_move_carry`).
struct GroupMoveCarry {
    needs: Vec<protocol::Membership>,
    free: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GroupMemberView {
    pub principal_id: String,
    pub name: String,
    pub kind: SenderType,
    pub role: BuildingRole,
}

/// What a viewer who may see the group learns about it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GroupView {
    pub id: String,
    pub name: String,
    /// The owner's current name; `None` when the Master owns it.
    pub owner: Option<String>,
    pub owner_id: Option<String>,
    pub owned_by_master: bool,
    pub created_at: u64,
    pub archived: bool,
    pub members: Vec<GroupMemberView>,
    pub locas: Vec<String>,
    /// May this viewer add/remove members, rename, archive, delete?
    pub can_manage: bool,
    /// May this viewer open a loca in it?
    pub can_create_loca: bool,
}

/// One audit line with names resolved for reading.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GroupLogView {
    pub action: String,
    pub subject: Option<String>,
    pub by: String,
    pub at: u64,
}

/// A group a person belongs to, as shown on a profile.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GroupBadge {
    pub id: String,
    pub name: String,
}

fn valid_group_name(name: &str) -> Option<String> {
    let name = name.trim();
    (!name.is_empty() && name.chars().count() <= 64 && !name.chars().any(char::is_control))
        .then(|| name.to_string())
}

impl Hub {
    /// Restore the group mirror at boot, then pass any group whose owning
    /// Smaster is gone to the Master (a revocation that happened while the
    /// server was down, or before groups existed, is still honoured).
    pub(super) fn load_groups(&self) {
        let groups = self
            .store
            .load_groups()
            .expect("persistent groups are unreadable; refusing to start");
        let members = self
            .store
            .load_group_members()
            .expect("persistent group members are unreadable; refusing to start");
        let rooms = self
            .store
            .load_group_rooms()
            .expect("persistent group locas are unreadable; refusing to start");
        let orphaned: Vec<String> = {
            let mut book = self.groups.lock_or_recover();
            for g in groups {
                book.groups.insert(g.id.clone(), g);
            }
            for (group, principal) in members {
                book.members.entry(group).or_default().insert(principal);
            }
            book.rooms = rooms.into_iter().collect();
            book.groups
                .values()
                .filter(|g| g.deleted_at.is_none())
                .filter_map(|g| g.owner.clone())
                .collect()
        };
        for owner in orphaned {
            let still_smaster = self
                .store
                .active_principal(&owner)
                .is_some_and(|p| p.role == BuildingRole::Smaster);
            if !still_smaster {
                self.pass_groups_to_master(&owner);
            }
        }
    }

    /// The group a loca belongs to, if any.
    pub fn group_of(&self, room: &str) -> Option<String> {
        self.groups.lock_or_recover().rooms.get(room).cloned()
    }

    pub fn is_group_loca(&self, room: &str) -> bool {
        self.groups.lock_or_recover().rooms.contains_key(room)
    }

    /// Every group loca, for sweeps that must skip them.
    pub fn group_locas(&self) -> HashSet<String> {
        self.groups
            .lock_or_recover()
            .rooms
            .keys()
            .cloned()
            .collect()
    }

    /// room → (group id, group name), for loca summaries.
    pub(super) fn group_names_by_room(&self) -> HashMap<String, (String, String)> {
        let book = self.groups.lock_or_recover();
        book.rooms
            .iter()
            .filter_map(|(room, id)| {
                book.groups
                    .get(id)
                    .map(|g| (room.clone(), (id.clone(), g.name.clone())))
            })
            .collect()
    }

    /// The door's question for a group loca: `None` when `room` is not one;
    /// otherwise whether any of `principals` is a member of its group. A
    /// deleted group has no members, so only the Master reaches its locas.
    pub(super) fn group_gate(&self, room: &str, principals: &[&str]) -> Option<bool> {
        let book = self.groups.lock_or_recover();
        let group = book.rooms.get(room)?;
        Some(principals.iter().any(|p| book.has_member(group, p)))
    }

    /// Group locas a viewer may not enter: they never name them in a
    /// "where is this person" listing. Empty for the Master.
    pub fn group_locas_hidden_from(&self, viewer: &RequestAuthority) -> HashSet<String> {
        if viewer.is_master() {
            return HashSet::new();
        }
        let me = viewer.principal_id.as_deref();
        let book = self.groups.lock_or_recover();
        book.rooms
            .iter()
            .filter(|(_, group)| !me.is_some_and(|me| book.has_member(group, me)))
            .map(|(room, _)| room.clone())
            .collect()
    }

    /// The groups this person belongs to (or owns).
    pub fn groups_of_principal(&self, principal: &str) -> Vec<GroupBadge> {
        let book = self.groups.lock_or_recover();
        let mut list: Vec<GroupBadge> = book
            .groups
            .values()
            .filter(|g| g.deleted_at.is_none())
            .filter(|g| book.has_member(&g.id, principal) || g.owner.as_deref() == Some(principal))
            .map(|g| GroupBadge {
                id: g.id.clone(),
                name: g.name.clone(),
            })
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }

    fn group_view_from(
        &self,
        g: GroupRecord,
        members: Vec<String>,
        locas: Vec<String>,
        viewer: &RequestAuthority,
    ) -> GroupView {
        let mut members: Vec<GroupMemberView> = members
            .iter()
            .filter_map(|id| self.store.active_principal(id))
            .map(|p| GroupMemberView {
                principal_id: p.id,
                name: p.display_name,
                kind: p.kind,
                role: p.role,
            })
            .collect();
        members.sort_by(|a, b| a.name.cmp(&b.name));
        let owner_name = g
            .owner
            .as_deref()
            .and_then(|id| self.store.active_principal(id))
            .map(|p| p.display_name);
        let me = viewer.principal_id.as_deref();
        let member_smaster = viewer.building_role == Some(BuildingRole::Smaster)
            && me.is_some_and(|me| members.iter().any(|m| m.principal_id == me));
        let open = g.archived_at.is_none();
        GroupView {
            can_manage: manages(&g, viewer),
            can_create_loca: open && (viewer.is_master() || member_smaster),
            id: g.id,
            name: g.name,
            owner: owner_name,
            owned_by_master: g.owner.is_none(),
            owner_id: g.owner,
            created_at: g.created_at,
            archived: g.archived_at.is_some(),
            members,
            locas: locas
                .into_iter()
                .filter(|room| !self.is_deleted(room))
                .collect(),
        }
    }

    /// Snapshot one group the viewer may see: record, members, locas.
    fn visible_group(
        &self,
        id: &str,
        viewer: &RequestAuthority,
    ) -> Result<(GroupRecord, Vec<String>, Vec<String>), GroupError> {
        let book = self.groups.lock_or_recover();
        let g = book.groups.get(id).ok_or(GroupError::NotFound)?;
        if !book.visible_to(g, viewer) {
            return Err(GroupError::NotFound);
        }
        let members = book
            .members
            .get(id)
            .map(|m| m.iter().cloned().collect())
            .unwrap_or_default();
        Ok((g.clone(), members, book.rooms_of(id)))
    }

    /// `GET /groups`: every group the viewer may see.
    pub fn groups_for(&self, viewer: &RequestAuthority) -> Vec<GroupView> {
        let snapshots: Vec<(GroupRecord, Vec<String>, Vec<String>)> = {
            let book = self.groups.lock_or_recover();
            book.groups
                .values()
                .filter(|g| book.visible_to(g, viewer))
                .map(|g| {
                    (
                        g.clone(),
                        book.members
                            .get(&g.id)
                            .map(|m| m.iter().cloned().collect())
                            .unwrap_or_default(),
                        book.rooms_of(&g.id),
                    )
                })
                .collect()
        };
        let mut list: Vec<GroupView> = snapshots
            .into_iter()
            .map(|(g, members, locas)| self.group_view_from(g, members, locas, viewer))
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        list
    }

    pub fn group_view(&self, id: &str, viewer: &RequestAuthority) -> Result<GroupView, GroupError> {
        let (g, members, locas) = self.visible_group(id, viewer)?;
        Ok(self.group_view_from(g, members, locas, viewer))
    }

    /// The group's record — who changed what, and when — for those who
    /// manage it.
    pub fn group_log(
        &self,
        id: &str,
        viewer: &RequestAuthority,
    ) -> Result<Vec<GroupLogView>, GroupError> {
        let (g, _, _) = self.visible_group(id, viewer)?;
        if !manages(&g, viewer) {
            return Err(GroupError::NotOwner);
        }
        let entries: Vec<GroupLogEntry> =
            self.store.group_log(id).map_err(|_| GroupError::Storage)?;
        let name = |id: &str| {
            self.store
                .active_principal(id)
                .map(|p| p.display_name)
                .unwrap_or_else(|| "former member".into())
        };
        Ok(entries
            .into_iter()
            .map(|e| {
                let subject = match e.action.as_str() {
                    "member_added" | "member_removed" | "owner_passed" => {
                        e.subject.as_deref().map(name)
                    }
                    _ => e.subject,
                };
                GroupLogView {
                    by: e.by.as_deref().map(name).unwrap_or_else(|| "Master".into()),
                    action: e.action,
                    subject,
                    at: e.at,
                }
            })
            .collect())
    }

    /// The Master or a Smaster founds a group. A Smaster owns it and is its
    /// first member; the Master owns it without ever being a member.
    pub fn create_group(
        &self,
        name: &str,
        viewer: &RequestAuthority,
    ) -> Result<GroupView, GroupError> {
        if !viewer.is_building_admin() {
            return Err(GroupError::RankRequired);
        }
        let name = valid_group_name(name).ok_or(GroupError::InvalidName)?;
        let master = viewer.is_master();
        let founder = match (master, viewer.principal_id.clone()) {
            (true, id) => id.unwrap_or_else(|| "master".into()),
            (false, Some(id)) => id,
            (false, None) => return Err(GroupError::RankRequired),
        };
        let g = GroupRecord {
            id: Self::secure_token("grp_", 8),
            name,
            owner: (!master).then(|| founder.clone()),
            founder: founder.clone(),
            created_at: self.now(),
            archived_at: None,
            deleted_at: None,
        };
        let first_member = (!master).then_some(founder.as_str());
        {
            let mut book = self.groups.lock_or_recover();
            self.store
                .create_group(&g, first_member, viewer.principal_id.as_deref())
                .map_err(|_| GroupError::Storage)?;
            if let Some(member) = first_member {
                book.members
                    .entry(g.id.clone())
                    .or_default()
                    .insert(member.to_string());
            }
            book.groups.insert(g.id.clone(), g.clone());
        }
        self.group_view(&g.id, viewer)
    }

    /// Change a group's record under the lock, persist it with one audit
    /// line, and return the new record.
    fn update_group(
        &self,
        id: &str,
        viewer: &RequestAuthority,
        action: &str,
        subject: Option<&str>,
        change: impl FnOnce(&mut GroupRecord, &[String]) -> Result<(), GroupError>,
    ) -> Result<GroupRecord, GroupError> {
        // Renaming, archiving and deleting a group take turns with a loca
        // move and with founding a loca in the group: each of those checks
        // that the group is live and open, and that answer must still be true
        // when it writes (see `set_loca_group`).
        let _lifecycle = self.member_lifecycle.lock_or_recover();
        let mut book = self.groups.lock_or_recover();
        let current = book.groups.get(id).ok_or(GroupError::NotFound)?;
        if !book.visible_to(current, viewer) {
            return Err(GroupError::NotFound);
        }
        if !manages(current, viewer) {
            return Err(GroupError::NotOwner);
        }
        let mut next = current.clone();
        let rooms = book.rooms_of(id);
        change(&mut next, &rooms)?;
        self.store
            .save_group(
                &next,
                action,
                subject,
                viewer.principal_id.as_deref(),
                self.now(),
            )
            .map_err(|_| GroupError::Storage)?;
        if next.deleted_at.is_some() {
            book.members.remove(id);
        }
        book.groups.insert(id.to_string(), next.clone());
        Ok(next)
    }

    pub fn rename_group(
        &self,
        id: &str,
        name: &str,
        viewer: &RequestAuthority,
    ) -> Result<GroupView, GroupError> {
        let name = valid_group_name(name).ok_or(GroupError::InvalidName)?;
        let subject = name.clone();
        self.update_group(id, viewer, "renamed", Some(&subject), |g, _| {
            g.name = name;
            Ok(())
        })?;
        self.group_view(id, viewer)
    }

    /// Archiving a group closes every one of its locas (read-only).
    /// Unarchiving reopens the group, not its locas.
    pub fn set_group_archived(
        &self,
        id: &str,
        archived: bool,
        viewer: &RequestAuthority,
    ) -> Result<GroupView, GroupError> {
        let now = self.now();
        let action = if archived { "archived" } else { "unarchived" };
        let mut rooms = Vec::new();
        self.update_group(id, viewer, action, None, |g, group_rooms| {
            g.archived_at = archived.then_some(g.archived_at.unwrap_or(now));
            rooms = group_rooms.to_vec();
            Ok(())
        })?;
        if archived {
            for room in rooms {
                if self.is_deleted(&room) || self.is_archived(&room) {
                    continue;
                }
                if self.close_loca(&room).is_err() {
                    return Err(GroupError::Storage);
                }
            }
        }
        self.group_view(id, viewer)
    }

    fn close_loca(&self, room: &str) -> rusqlite::Result<protocol::RoomSettings> {
        self.set_settings(
            room,
            None,
            None,
            None,
            Some(true),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
    }

    /// Delete a group with no open loca. Its members are dropped (recorded);
    /// its closed locas stay tied to it, so only the Master still reads them.
    pub fn delete_group(&self, id: &str, viewer: &RequestAuthority) -> Result<(), GroupError> {
        // Decided outside the groups lock: `is_archived` takes `rooms`.
        let rooms = self.groups.lock_or_recover().rooms_of(id);
        let open: HashSet<String> = rooms
            .iter()
            .filter(|room| !self.is_deleted(room) && !self.is_archived(room))
            .cloned()
            .collect();
        let now = self.now();
        let mut dropped: Vec<String> = Vec::new();
        {
            let book = self.groups.lock_or_recover();
            dropped.extend(book.members.get(id).into_iter().flatten().cloned());
        }
        self.update_group(id, viewer, "deleted", None, |g, current_rooms| {
            // A loca that joined since the check above counts as open.
            if current_rooms
                .iter()
                .any(|room| open.contains(room) || !rooms.contains(room))
            {
                return Err(GroupError::OpenLocas);
            }
            g.deleted_at = Some(now);
            Ok(())
        })?;
        for principal in dropped {
            self.cut_group_access(id, &principal);
        }
        Ok(())
    }

    /// The founder (or the Master) adds a person. Nobody adds themselves, and
    /// the Master is never a member.
    pub fn add_group_member(
        &self,
        id: &str,
        principal: &str,
        viewer: &RequestAuthority,
    ) -> Result<GroupView, GroupError> {
        if viewer.principal_id.as_deref() == Some(principal) {
            return Err(GroupError::SelfAdd);
        }
        let person = self
            .store
            .active_principal(principal)
            .ok_or(GroupError::NoSuchPerson)?;
        if person.role == BuildingRole::Master {
            return Err(GroupError::MasterNeverMember);
        }
        {
            // A membership change and a loca move decide who holds a seat
            // from the same facts, so they take turns (see `set_loca_group`).
            let _lifecycle = self.member_lifecycle.lock_or_recover();
            let mut book = self.groups.lock_or_recover();
            let g = book.groups.get(id).ok_or(GroupError::NotFound)?;
            if !book.visible_to(g, viewer) {
                return Err(GroupError::NotFound);
            }
            if !manages(g, viewer) {
                return Err(GroupError::NotOwner);
            }
            if !book.has_member(id, principal) {
                self.store
                    .add_group_member(id, principal, viewer.principal_id.as_deref(), self.now())
                    .map_err(|_| GroupError::Storage)?;
                book.members
                    .entry(id.to_string())
                    .or_default()
                    .insert(principal.to_string());
            }
        }
        self.group_view(id, viewer)
    }

    /// The founder (or the Master) removes a person. Access to the group's
    /// locas ends at once: davets, davet sessions, live sockets.
    pub fn remove_group_member(
        &self,
        id: &str,
        principal: &str,
        viewer: &RequestAuthority,
    ) -> Result<GroupView, GroupError> {
        let removed = {
            // Takes turns with a loca move (see `set_loca_group`): a move that
            // ran first has already mirrored the loca into its group, so the
            // cut below finds this person's davet there; a move that runs
            // after sees them outside the group and ends the davet itself.
            let _lifecycle = self.member_lifecycle.lock_or_recover();
            let mut book = self.groups.lock_or_recover();
            let g = book.groups.get(id).ok_or(GroupError::NotFound)?;
            if !book.visible_to(g, viewer) {
                return Err(GroupError::NotFound);
            }
            if !manages(g, viewer) {
                return Err(GroupError::NotOwner);
            }
            if g.owner.as_deref() == Some(principal) {
                return Err(GroupError::FounderStays);
            }
            if book.has_member(id, principal) {
                self.store
                    .remove_group_member(id, principal, viewer.principal_id.as_deref(), self.now())
                    .map_err(|_| GroupError::Storage)?;
                if let Some(members) = book.members.get_mut(id) {
                    members.remove(principal);
                }
                true
            } else {
                false
            }
        };
        if removed {
            self.cut_group_access(id, principal);
            self.end_titles_outside_group(id, principal);
        }
        self.group_view(id, viewer)
    }

    /// A person left a group: a title they held in its locas (operator,
    /// lead) must not hang on someone those locas no longer admit.
    fn end_titles_outside_group(&self, group: &str, principal: &str) {
        let rooms = self.groups.lock_or_recover().rooms_of(group);
        let name = self
            .store
            .active_principal(principal)
            .map(|p| p.display_name);
        for room in rooms {
            if self.group_gate(&room, &[principal]) == Some(true) {
                continue;
            }
            if self
                .loca_operator(&room)
                .is_some_and(|op| op.principal_id == principal)
            {
                if let Err(error) = self.end_loca_operator_by_rule(&room) {
                    tracing::warn!(room, ?error, "could not end a removed member's appointment");
                }
            }
            if name.is_some() && self.settings(&room).lead == name {
                let _ = self.set_lead(&room, None, "group-removal");
            }
        }
    }

    /// Someone left a group (or the group went away): end what they held in
    /// its locas now. Davets for those locas are revoked (which also kills
    /// the sessions minted from them and closes their sockets), and any live
    /// socket they hold there by another credential is closed. Every socket
    /// re-asks the door before each frame, so nothing more reaches them.
    fn cut_group_access(&self, group: &str, principal: &str) {
        let rooms = self.groups.lock_or_recover().rooms_of(group);
        let name = self
            .store
            .active_principal(principal)
            .map(|p| p.display_name);
        for room in rooms {
            self.revoke_davets_outside_group(&room);
            if let Some(name) = name.as_deref() {
                if self.group_gate(&room, &[principal]) == Some(false) {
                    self.close_connections(&room, name);
                }
            }
        }
    }

    /// Who needs a new davet to keep their seat when `room` leaves `previous`
    /// for the default group, and how many davets the loca can still take.
    /// Nobody for any other move. The preview and the move both read this, so
    /// the preview states exactly what the move will do.
    fn group_move_carry(
        &self,
        room: &str,
        previous: Option<&str>,
        target: Option<&str>,
    ) -> GroupMoveCarry {
        let active = self
            .invites
            .lock_or_recover()
            .values()
            .filter(|i| i.room == room)
            .count();
        let free = self.loca_capacity().saturating_sub(active);
        let (Some(old), None) = (previous, target) else {
            return GroupMoveCarry {
                needs: Vec::new(),
                free,
            };
        };
        let principals: Vec<String> = self
            .groups
            .lock_or_recover()
            .members
            .get(old)
            .map(|m| m.iter().cloned().collect())
            .unwrap_or_default();
        let mut needs: Vec<protocol::Membership> = principals
            .iter()
            // Only Building members get a davet; ranks reach a default loca
            // by rank and are not carried.
            .filter_map(|principal| self.member_token_of_principal(principal))
            .filter(|token| !self.has_active_invite(room, token))
            // A suspended member holds no davet (`member_of` answers `None`).
            .filter_map(|token| self.member_of(Some(&token)))
            .collect();
        needs.sort_by(|a, b| a.name.cmp(&b.name));
        GroupMoveCarry { needs, free }
    }

    /// The live davets of `room` whose holder the `target` group does not
    /// admit. None when the target is the default group.
    fn davets_outside_target_group(&self, room: &str, target: Option<&str>) -> Vec<Invite> {
        let Some(target) = target else {
            return Vec::new();
        };
        let holders: Vec<(Invite, Option<String>)> = self
            .invites_of(room)
            .into_iter()
            .map(|invite| {
                let principal = self.member_principal(&invite.member);
                (invite, principal)
            })
            .collect();
        let book = self.groups.lock_or_recover();
        holders
            .into_iter()
            .filter(|(_, principal)| {
                !principal
                    .as_deref()
                    .is_some_and(|p| book.has_member(target, p))
            })
            .map(|(invite, _)| invite)
            .collect()
    }

    /// Revoke every davet for `room` whose holder is not a member of the
    /// room's group. A no-op for a default-group loca.
    fn revoke_davets_outside_group(&self, room: &str) {
        for invite in self.invites_of(room) {
            let principal = self.member_principal(&invite.member);
            let member = principal
                .as_deref()
                .is_some_and(|p| self.group_gate(room, &[p]) == Some(true));
            if !member && self.group_gate(room, &[]).is_some() {
                if let Err(error) = self.revoke_invite(&invite.token) {
                    tracing::error!(room, %error, "could not revoke a davet outside the group");
                }
            }
        }
    }

    /// Close the sockets seated in `room` under a name that belongs to no
    /// member of its group (the Master's seat stays).
    fn close_non_member_seats(&self, room: &str) {
        let Some(group) = self.group_of(room) else {
            return;
        };
        let member_names: HashSet<String> = {
            let ids: Vec<String> = self
                .groups
                .lock_or_recover()
                .members
                .get(&group)
                .map(|m| m.iter().cloned().collect())
                .unwrap_or_default();
            ids.iter()
                .filter_map(|id| self.store.active_principal(id))
                .map(|p| p.display_name)
                .collect()
        };
        let seated: Vec<(String, String)> = self
            .rooms
            .lock_or_recover()
            .get(room)
            .map(|r| {
                r.members
                    .iter()
                    .map(|(identity, (label, _, _))| (identity.clone(), label.clone()))
                    .collect()
            })
            .unwrap_or_default();
        for (identity, label) in seated {
            if identity != "@master" && !member_names.contains(&label) {
                self.close_connections(room, &label);
            }
        }
    }

    /// `PUT /rooms/:id/group` — the Master alone moves a loca between the
    /// default group (`None`) and a group, or between groups.
    pub fn set_loca_group(
        &self,
        room: &str,
        group: Option<&str>,
        viewer: &RequestAuthority,
    ) -> Result<Option<String>, GroupError> {
        if !viewer.is_master() {
            return Err(GroupError::MasterOnly);
        }
        if self.is_birebir(room)
            || room.starts_with(Self::BIREBIR_PREFIX)
            || self.is_reserved_room(room)
            || room == self.home_room()
        {
            return Err(GroupError::Excluded);
        }
        if !self.room_exists(room) || self.is_deleted(room) {
            return Err(GroupError::NoSuchLoca);
        }
        // The move is all-or-nothing. Who may reach the loca afterwards — its
        // group, the davets that carry people over, the davets the new group
        // no longer admits — is decided under the davet lock and written in
        // one transaction, so no davet is minted or ended half-way through
        // and a failed write leaves the loca exactly where it was.
        let (target_name, carried, ended) = {
            let _lifecycle = self.member_lifecycle.lock_or_recover();
            let previous = self.group_of(room);
            let target_name = {
                let book = self.groups.lock_or_recover();
                if let Some(group) = group {
                    let g = book.live(group).ok_or(GroupError::NotFound)?;
                    if g.archived_at.is_some() {
                        return Err(GroupError::Archived);
                    }
                }
                group
                    .and_then(|g| book.groups.get(g))
                    .map(|g| g.name.clone())
            };
            if previous.as_deref() == group {
                return Ok(group.map(str::to_string));
            }
            // Moving to the default group, the people who reached the loca
            // through the group it is leaving keep a seat by davet instead.
            // A davet IS a seat, so all of them must fit: the move is refused
            // rather than seating some and silently dropping the rest — who
            // stays is the Master's decision, never iteration order.
            let plan = self.group_move_carry(room, previous.as_deref(), group);
            if plan.needs.len() > plan.free {
                return Err(GroupError::CarryOverCapacity);
            }
            let now = self.now();
            let carried: Vec<Invite> = plan
                .needs
                .into_iter()
                .map(|member| Invite {
                    token: self.new_invite_token(),
                    room: room.to_string(),
                    member: member.token,
                    name: member.name,
                    kind: member.kind,
                    issued_at: now,
                    // Only the Master moves a loca: this davet is the
                    // Master's own decision and ends only from above.
                    issued_by: "group-move".to_string(),
                })
                .collect();
            let ended = self.davets_outside_target_group(room, group);
            let ended_tokens: Vec<String> = ended.iter().map(|i| i.token.clone()).collect();
            self.store
                .move_room_group(
                    room,
                    group,
                    viewer.principal_id.as_deref(),
                    now,
                    &carried,
                    &ended_tokens,
                    (self.now_ms)(),
                )
                .map_err(|_| GroupError::Storage)?;
            // Committed: nothing fallible remains before the mirrors.
            {
                let mut book = self.groups.lock_or_recover();
                match group {
                    Some(group) => {
                        book.rooms.insert(room.to_string(), group.to_string());
                    }
                    None => {
                        book.rooms.remove(room);
                    }
                }
            }
            {
                let mut invites = self.invites.lock_or_recover();
                for invite in &ended {
                    invites.remove(&invite.token);
                }
                for invite in &carried {
                    invites.insert(invite.token.clone(), invite.clone());
                }
            }
            (target_name, carried, ended)
        };
        // Whoever no longer belongs at this table leaves it now.
        for invite in &ended {
            self.revoke_sessions_for_davet(&invite.room, &invite.member);
            self.close_connections(&invite.room, &invite.name);
        }
        self.close_non_member_seats(room);
        // Wake an already-running agent in the Lobby; a listener that is not
        // connected right now replays the persisted davet when it returns.
        for invite in carried {
            let _ = self.lobby_tx.send(LobbyEvent::Called {
                member: invite.member,
                room: invite.room,
                token: invite.token,
            });
        }
        // A title must not hang on someone the loca no longer admits.
        if group.is_some() {
            let lead = self.settings(room).lead;
            if let Some(lead) = lead {
                let admitted = self
                    .principal_named(&lead)
                    .is_some_and(|p| self.group_gate(room, &[p.as_str()]) == Some(true));
                if !admitted {
                    let _ = self.set_lead(room, None, "group-move");
                }
            }
            if let Some(op) = self.loca_operator(room) {
                let master = self
                    .store
                    .active_principal(&op.principal_id)
                    .is_some_and(|p| p.role == BuildingRole::Master);
                if !master && self.group_gate(room, &[op.principal_id.as_str()]) != Some(true) {
                    if let Err(error) = self.revoke_loca_operator(room, viewer) {
                        tracing::warn!(
                            room,
                            ?error,
                            "could not end an outside operator's appointment"
                        );
                    }
                }
            }
        }
        let where_to = target_name.unwrap_or_else(|| "the default group".into());
        let _ = self.append_journal(
            room,
            "Master".into(),
            SenderType::User,
            format!("moved this loca to {where_to}"),
        );
        Ok(group.map(str::to_string))
    }

    /// What moving `room` to `group` would do, without doing it: who loses
    /// access, who keeps it, who is carried over by davet, and which titles
    /// end. The UI shows this before the Master confirms.
    pub fn loca_group_move_preview(
        &self,
        room: &str,
        group: Option<&str>,
        viewer: &RequestAuthority,
    ) -> Result<GroupMovePreview, GroupError> {
        if !viewer.is_master() {
            return Err(GroupError::MasterOnly);
        }
        if self.is_birebir(room)
            || room.starts_with(Self::BIREBIR_PREFIX)
            || self.is_reserved_room(room)
            || room == self.home_room()
        {
            return Err(GroupError::Excluded);
        }
        if !self.room_exists(room) || self.is_deleted(room) {
            return Err(GroupError::NoSuchLoca);
        }
        let (previous, target_members, previous_members) = {
            let book = self.groups.lock_or_recover();
            if let Some(g) = group {
                let rec = book.live(g).ok_or(GroupError::NotFound)?;
                if rec.archived_at.is_some() {
                    return Err(GroupError::Archived);
                }
            }
            let previous = book.rooms.get(room).cloned();
            let set = |g: Option<&str>| -> HashSet<String> {
                g.and_then(|g| book.members.get(g))
                    .cloned()
                    .unwrap_or_default()
            };
            (previous.clone(), set(group), set(previous.as_deref()))
        };
        // Everyone who reaches the loca now: davet holders, live seats, and
        // the members of its current group. The Master is never in question.
        let mut people: HashMap<String, Option<String>> = HashMap::new();
        for invite in self.invites_of(room) {
            people.insert(invite.name.clone(), self.member_principal(&invite.member));
        }
        let seated: Vec<String> = self
            .rooms
            .lock_or_recover()
            .get(room)
            .map(|r| {
                r.members
                    .iter()
                    .filter(|(identity, _)| identity.as_str() != "@master")
                    .map(|(_, (label, _, _))| label.clone())
                    .collect()
            })
            .unwrap_or_default();
        for name in seated {
            let principal = self.principal_named(&name);
            people.entry(name).or_insert(principal);
        }
        for principal in &previous_members {
            if let Some(p) = self.store.active_principal(principal) {
                people
                    .entry(p.display_name)
                    .or_insert(Some(principal.clone()));
            }
        }
        let mut preview = GroupMovePreview::default();
        for (name, principal) in people {
            let principal = principal.as_deref();
            let is_master = principal.is_some_and(|p| {
                self.store
                    .active_principal(p)
                    .is_some_and(|x| x.role == BuildingRole::Master)
            });
            if is_master {
                continue;
            }
            let keeps = match group {
                Some(_) => principal.is_some_and(|p| target_members.contains(p)),
                None => {
                    // Only Building members get a davet; ranks reach a default
                    // loca by rank and are not "carried".
                    let carried = principal.is_some_and(|p| {
                        previous_members.contains(p) && self.member_token_of_principal(p).is_some()
                    });
                    if carried && previous.is_some() {
                        preview.carried_by_davet.push(name.clone());
                    }
                    true
                }
            };
            if keeps {
                preview.keeping.push(name);
            } else {
                preview.losing.push(name);
            }
        }
        if group.is_some() {
            if let Some(lead) = self.settings(room).lead {
                let ok = self
                    .principal_named(&lead)
                    .is_some_and(|p| target_members.contains(&p));
                if !ok {
                    preview.lead_ends = Some(lead);
                }
            }
            if let Some(op) = self.loca_operator(room) {
                let master = self
                    .store
                    .active_principal(&op.principal_id)
                    .is_some_and(|p| p.role == BuildingRole::Master);
                if !master && !target_members.contains(&op.principal_id) {
                    preview.operator_ends = Some(op.display_name);
                }
            }
        }
        let carry = self.group_move_carry(room, previous.as_deref(), group);
        preview.seats_needed = carry.needs.len();
        preview.seats_free = carry.free;
        preview.blocked = preview.seats_needed > preview.seats_free;
        preview.losing.sort();
        preview.keeping.sort();
        preview.carried_by_davet.sort();
        Ok(preview)
    }

    /// The single active principal wearing this display name, if any.
    fn principal_named(&self, name: &str) -> Option<String> {
        match self.store.active_principals_named(name) {
            Ok(list) if list.len() == 1 => Some(list[0].id.clone()),
            _ => None,
        }
    }

    /// The membership (mb_) token behind a principal, when it is a member.
    fn member_token_of_principal(&self, principal: &str) -> Option<String> {
        let tokens: Vec<String> = self.members.lock_or_recover().keys().cloned().collect();
        tokens
            .into_iter()
            .find(|t| self.member_principal(t).as_deref() == Some(principal))
    }

    /// May `viewer` open a loca in this group? The Master, or a Smaster who
    /// is a member (the founder included). Checked by `create_loca`.
    pub(super) fn may_create_in_group(
        &self,
        group: &str,
        viewer: &RequestAuthority,
    ) -> Result<(), GroupError> {
        let book = self.groups.lock_or_recover();
        let g = book.groups.get(group).ok_or(GroupError::NotFound)?;
        if !book.visible_to(g, viewer) {
            return Err(GroupError::NotFound);
        }
        if g.archived_at.is_some() {
            return Err(GroupError::Archived);
        }
        let member_smaster = viewer.building_role == Some(BuildingRole::Smaster)
            && viewer
                .principal_id
                .as_deref()
                .is_some_and(|me| book.has_member(group, me));
        if viewer.is_master() || member_smaster {
            Ok(())
        } else {
            Err(GroupError::NotOwner)
        }
    }

    /// Record a freshly created group loca in the mirror. Called by
    /// `create_loca` while it holds `rooms` (rooms → groups, the one order).
    pub(super) fn bind_new_group_loca(&self, room: &str, group: &str) {
        self.groups
            .lock_or_recover()
            .rooms
            .insert(room.to_string(), group.to_string());
    }

    /// A founding Smaster was revoked: every group it owned passes to the
    /// Master, recorded, and it leaves those groups.
    pub(super) fn pass_groups_to_master(&self, principal: &str) {
        let owned: Vec<String> = {
            let mut book = self.groups.lock_or_recover();
            let ids: Vec<String> = book
                .groups
                .values()
                .filter(|g| g.owner.as_deref() == Some(principal))
                .map(|g| g.id.clone())
                .collect();
            let now = self.now();
            for id in &ids {
                let Some(mut g) = book.groups.get(id).cloned() else {
                    continue;
                };
                g.owner = None;
                if let Err(error) =
                    self.store
                        .save_group(&g, "owner_passed", Some(principal), None, now)
                {
                    tracing::error!(group = %id, %error, "could not pass a group to the Master");
                    continue;
                }
                book.groups.insert(id.clone(), g);
            }
            ids
        };
        for id in owned {
            let member = self.groups.lock_or_recover().has_member(&id, principal);
            if member {
                if let Err(error) = self
                    .store
                    .remove_group_member(&id, principal, None, self.now())
                {
                    tracing::error!(group = %id, %error, "could not drop a revoked founder");
                    continue;
                }
                if let Some(members) = self.groups.lock_or_recover().members.get_mut(&id) {
                    members.remove(principal);
                }
                self.cut_group_access(&id, principal);
            }
        }
    }

    /// The stable principal behind a membership (mb_) token. The binding
    /// never changes, so a positive answer is cached: the door asks for every
    /// agent davet at a group loca, on every frame.
    pub fn member_principal(&self, member_token: &str) -> Option<String> {
        if member_token.is_empty() {
            return None;
        }
        if let Some(found) = self.member_principals.lock_or_recover().get(member_token) {
            return Some(found.clone());
        }
        let found = self.store.principal_id_for_member_record(member_token)?;
        self.member_principals
            .lock_or_recover()
            .insert(member_token.to_string(), found.clone());
        Some(found)
    }
}
