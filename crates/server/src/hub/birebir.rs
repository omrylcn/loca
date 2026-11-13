//! Birebir: a private loca for exactly two people (PRINCIPLES "Birebir").
//!
//! A birebir is an ordinary loca underneath — history, attachments, reactions
//! and live delivery travel the same path — but its door belongs to the pair:
//! `enter_decision` admits only the two principals (or the davet minted for an
//! agent side), never a rank. The Master learns only that it exists, and reads
//! it only after one side calls the Master in.

use protocol::{Message, SenderType, ServerFrame};

use super::{Hub, Room, SessionIdentity};
use crate::store::Birebir;
use crate::sync::RecoverMutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BirebirError {
    /// Only a human opens a birebir; an agent answers in one.
    NotHuman,
    /// The caller has no stable identity to bind the birebir to.
    NoIdentity,
    /// No active person carries that name.
    PeerNotFound,
    /// One cannot open a birebir with oneself.
    SelfPair,
    /// A member may open one only with someone at a shared table.
    NoSharedTable,
    /// The Master sealed this pair's birebir; it does not come back.
    Sealed,
    /// The caller is not one of the two.
    NotParty,
    /// Only the side that called the Master in may send the Master out.
    NotCaller,
    /// The Master is not in this birebir.
    MasterNotIn,
    Storage,
}

impl BirebirError {
    pub fn message(self) -> &'static str {
        match self {
            Self::NotHuman => "only a person opens a birebir",
            Self::NoIdentity => "sign in with your own identity to open a birebir",
            Self::PeerNotFound => "nobody by that name is in the building",
            Self::SelfPair => "a birebir needs two people",
            Self::NoSharedTable => {
                "you can open a birebir only with someone you sit with in a loca"
            }
            Self::Sealed => "this birebir was sealed and cannot be reopened",
            Self::NotParty => "not found",
            Self::NotCaller => "only the one who called the Master in can send the Master out",
            Self::MasterNotIn => "the Master is not in this birebir",
            Self::Storage => "could not save — try again",
        }
    }
}

/// What the Master may know about a birebir: that it exists, between whom,
/// and when it was last active. Never its words.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BirebirOverview {
    pub room: String,
    pub between: [String; 2],
    pub opened_at: u64,
    pub last_at: u64,
    pub archived: bool,
    pub master_in: bool,
    pub history_open: bool,
}

/// What a party sees about its own birebir.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BirebirView {
    pub room: String,
    pub peer: String,
    pub peer_kind: SenderType,
    pub master_in: bool,
    pub master_called_by_me: bool,
    pub master_since: Option<u64>,
    pub history_allowed_by_me: bool,
    pub history_open: bool,
}

impl Hub {
    /// Every birebir room id starts with this; no ordinary loca may.
    pub const BIREBIR_PREFIX: &'static str = "bb-";

    pub fn birebir(&self, room: &str) -> Option<Birebir> {
        self.birebirs.lock_or_recover().get(room).cloned()
    }

    pub fn is_birebir(&self, room: &str) -> bool {
        self.birebirs.lock_or_recover().contains_key(room)
    }

    /// The pair's room id. It carries no name, so a rename never moves it and
    /// a URL never reveals who talks to whom.
    pub fn birebir_room_id(a: &str, b: &str) -> String {
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        crate::store::hashed_id(Self::BIREBIR_PREFIX, &format!("{a}|{b}"))
    }

    fn principal_name(&self, principal: &str) -> Option<(String, SenderType)> {
        self.store
            .active_principal(principal)
            .map(|p| (p.display_name, p.kind))
    }

    /// Names seated at a table: live seats plus held davets (a davet is a
    /// seat, PRINCIPLES "Model A"). Birebirs and İye are not shared tables.
    fn shares_a_table(&self, a: &str, b: &str) -> bool {
        let birebirs = self.birebirs.lock_or_recover().clone();
        let mut seated: std::collections::HashMap<String, std::collections::HashSet<String>> =
            std::collections::HashMap::new();
        for inv in self.invites.lock_or_recover().values() {
            seated
                .entry(inv.room.clone())
                .or_default()
                .insert(inv.name.clone());
        }
        for (room, r) in self.rooms.lock_or_recover().iter() {
            for (name, _, _) in r.members.values() {
                seated.entry(room.clone()).or_default().insert(name.clone());
            }
        }
        seated.iter().any(|(room, names)| {
            !birebirs.contains_key(room)
                && !self.is_deleted(room)
                && names.contains(a)
                && names.contains(b)
        })
    }

    /// Open (or return) the birebir between the caller and `peer_name`.
    /// Returns the birebir and whether it was created now.
    pub fn open_birebir(
        &self,
        opener: &SessionIdentity,
        peer_name: &str,
    ) -> Result<(Birebir, bool), BirebirError> {
        if opener.kind != SenderType::User {
            return Err(BirebirError::NotHuman);
        }
        let me = opener
            .principal_id
            .clone()
            .ok_or(BirebirError::NoIdentity)?;
        let peers = self
            .store
            .active_principals_named(peer_name.trim())
            .map_err(|_| BirebirError::Storage)?;
        let [peer] = peers.as_slice() else {
            return Err(BirebirError::PeerNotFound);
        };
        if peer.id == me {
            return Err(BirebirError::SelfPair);
        }
        let room = Self::birebir_room_id(&me, &peer.id);
        if self.is_deleted(&room) {
            return Err(BirebirError::Sealed);
        }
        if let Some(existing) = self.birebir(&room) {
            self.ensure_agent_davet(&existing, peer);
            return Ok((existing, false));
        }
        // Never adopt a room that already exists as something else.
        if self.rooms.lock_or_recover().contains_key(&room) {
            return Err(BirebirError::Storage);
        }
        // A rank may open one with anyone it could already call; a member
        // only with someone at a shared table. Checked on opening only.
        let rank = matches!(
            opener.building_role,
            crate::store::BuildingRole::Master | crate::store::BuildingRole::Smaster
        );
        if !rank && !self.shares_a_table(&opener.name, &peer.display_name) {
            return Err(BirebirError::NoSharedTable);
        }
        let (a, b) = if me <= peer.id {
            (me.clone(), peer.id.clone())
        } else {
            (peer.id.clone(), me.clone())
        };
        let mut settings = self.default_settings.clone();
        // Nothing leaves a birebir: no care reminders, no live broadcast.
        settings.care_goal_secs = 0;
        settings.care_task_secs = 0;
        settings.care_silence_secs = 0;
        settings.live = false;
        settings.lead = None;
        settings.operators = Vec::new();
        // A birebir is conversation: it has no extensions (PRINCIPLES "Loca
        // extensions"). `Hub::extension_enabled` also refuses by room kind, so
        // a birebir saved before this field existed has none either.
        settings.extensions = Some(Vec::new());
        let fresh = Birebir {
            room: room.clone(),
            a,
            b,
            opened_by: me,
            opened_at: self.now(),
            master_called_by: None,
            master_since: None,
            history_a: false,
            history_b: false,
        };
        let saved = self
            .store
            .create_birebir(&fresh, &settings)
            .map_err(|_| BirebirError::Storage)?;
        self.rooms
            .lock_or_recover()
            .entry(saved.room.clone())
            .or_insert_with(|| Room::with(protocol::ChatMode::Free, settings.clone(), 1));
        self.birebirs
            .lock_or_recover()
            .insert(saved.room.clone(), saved.clone());
        self.ensure_agent_davet(&saved, peer);
        Ok((saved, true))
    }

    /// An agent side reaches a birebir with a davet delivered over its Lobby
    /// line, exactly like a call. A human side needs none: its own session
    /// identity opens the door.
    fn ensure_agent_davet(&self, b: &Birebir, peer: &crate::store::PrincipalIdentity) {
        if peer.kind != SenderType::Agent {
            return;
        }
        if let Some(member) = self.member_by_name(&peer.display_name) {
            if !self.has_active_invite(&b.room, &member.token) {
                let _ = self.invite_member_to_room(&member.token, &b.room, "birebir");
            }
        }
    }

    /// The two names of a birebir, as they are worn now.
    pub fn birebir_names(&self, b: &Birebir) -> [String; 2] {
        let name = |p: &str| {
            self.principal_name(p)
                .map(|(n, _)| n)
                .unwrap_or_else(|| "former member".into())
        };
        [name(&b.a), name(&b.b)]
    }

    /// Is either side gone? A birebir whose side left the building is
    /// read-only (PRINCIPLES "Birebir", lifecycle).
    pub fn birebir_read_only(&self, b: &Birebir) -> bool {
        self.principal_name(&b.a).is_none() || self.principal_name(&b.b).is_none()
    }

    /// The other side's current name, for a message posted by `sender`.
    pub(super) fn birebir_peer_of(&self, room: &str, sender: &str) -> Option<String> {
        let b = self.birebir(room)?;
        let [a, bn] = self.birebir_names(&b);
        if a == sender {
            Some(bn)
        } else if bn == sender {
            Some(a)
        } else {
            None
        }
    }

    pub fn birebir_view(&self, room: &str, principal: &str) -> Option<BirebirView> {
        let b = self.birebir(room)?;
        let other = b.other(principal)?.to_string();
        let (peer, peer_kind) = self
            .principal_name(&other)
            .unwrap_or_else(|| ("former member".into(), SenderType::User));
        let mine_is_a = b.a == principal;
        Some(BirebirView {
            room: b.room.clone(),
            peer,
            peer_kind,
            master_in: b.master_since.is_some(),
            master_called_by_me: b.master_called_by.as_deref() == Some(principal),
            master_since: b.master_since,
            history_allowed_by_me: if mine_is_a { b.history_a } else { b.history_b },
            history_open: b.history_open(),
        })
    }

    /// The birebirs this principal is one side of.
    pub fn birebirs_of(&self, principal: &str) -> Vec<BirebirView> {
        let rooms: Vec<String> = self
            .birebirs
            .lock_or_recover()
            .values()
            .filter(|b| b.has(principal))
            .map(|b| b.room.clone())
            .collect();
        rooms
            .iter()
            .filter(|room| !self.is_deleted(room))
            .filter_map(|room| self.birebir_view(room, principal))
            .collect()
    }

    fn update_birebir(
        &self,
        room: &str,
        principal: &str,
        change: impl FnOnce(&mut Birebir) -> Result<(), BirebirError>,
    ) -> Result<Birebir, BirebirError> {
        let mut map = self.birebirs.lock_or_recover();
        let current = map.get(room).ok_or(BirebirError::NotParty)?;
        if !current.has(principal) || self.is_deleted(room) {
            return Err(BirebirError::NotParty);
        }
        let mut next = current.clone();
        change(&mut next)?;
        self.store
            .save_birebir_access(&next)
            .map_err(|_| BirebirError::Storage)?;
        map.insert(room.to_string(), next.clone());
        drop(map);
        if let Some(r) = self.rooms.lock_or_recover().get(room) {
            let _ = r.tx.send(ServerFrame::Control {
                cmd: "birebir-access".into(),
            });
        }
        Ok(next)
    }

    /// One side calls the Master in. No consent from the other side is
    /// needed; both see it. The Master reads from this moment on.
    pub fn birebir_call_master(
        &self,
        room: &str,
        principal: &str,
    ) -> Result<Birebir, BirebirError> {
        let now = self.now();
        self.update_birebir(room, principal, |b| {
            if b.master_since.is_none() {
                b.master_called_by = Some(principal.to_string());
                b.master_since = Some(now);
            }
            Ok(())
        })
    }

    /// The side that called the Master in sends the Master out. Any history
    /// permission ends with the visit.
    pub fn birebir_dismiss_master(
        &self,
        room: &str,
        principal: &str,
    ) -> Result<Birebir, BirebirError> {
        self.update_birebir(room, principal, |b| {
            if b.master_since.is_none() {
                return Err(BirebirError::MasterNotIn);
            }
            if b.master_called_by.as_deref() != Some(principal) {
                return Err(BirebirError::NotCaller);
            }
            b.master_called_by = None;
            b.master_since = None;
            b.history_a = false;
            b.history_b = false;
            Ok(())
        })
    }

    /// One side allows the called-in Master to read earlier history. It opens
    /// only when both sides have allowed it.
    pub fn birebir_allow_history(
        &self,
        room: &str,
        principal: &str,
    ) -> Result<Birebir, BirebirError> {
        self.update_birebir(room, principal, |b| {
            if b.master_since.is_none() {
                return Err(BirebirError::MasterNotIn);
            }
            if b.a == principal {
                b.history_a = true;
            } else {
                b.history_b = true;
            }
            Ok(())
        })
    }

    /// The Master's existence-only view (PRINCIPLES: so the Building does not
    /// turn into a dump). Names, times, state — never words.
    pub fn birebir_overview(&self) -> Vec<BirebirOverview> {
        let all: Vec<Birebir> = self.birebirs.lock_or_recover().values().cloned().collect();
        let mut list: Vec<BirebirOverview> = all
            .into_iter()
            .filter(|b| !self.is_deleted(&b.room))
            .map(|b| {
                let (last_at, archived) = self
                    .rooms
                    .lock_or_recover()
                    .get(&b.room)
                    .map(|r| (r.last_msg_ms, r.settings.archived))
                    .unwrap_or((0, false));
                BirebirOverview {
                    between: self.birebir_names(&b),
                    room: b.room.clone(),
                    opened_at: b.opened_at,
                    last_at,
                    archived,
                    master_in: b.master_since.is_some(),
                    history_open: b.history_open(),
                }
            })
            .collect();
        list.sort_by_key(|b| std::cmp::Reverse(b.last_at));
        list
    }

    /// What a called-in Master may read: from the call on, or everything once
    /// both sides allowed it. `None` when the Master is not in.
    pub fn birebir_witness_messages(&self, room: &str) -> Option<Vec<Message>> {
        let b = self.birebir(room)?;
        let since = b.master_since?;
        let rooms = self.rooms.lock_or_recover();
        let r = rooms.get(room)?;
        Some(
            r.history
                .iter()
                .filter(|m| b.history_open() || m.ts >= since)
                .cloned()
                .collect(),
        )
    }
}
