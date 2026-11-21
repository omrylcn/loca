//! The Master Agent rank: an automation the Master appoints, switched on and
//! off for the whole Building by one Master-only setting (off by default).
//!
//! This module holds what the Master does with the rank (the setting, the
//! appointment, the revocation) and the questions every door asks about it.
//! What a Master Agent may do is decided at two places only: the route fence
//! (`routes/master_agent.rs`), which lets a Master Agent reach a short list
//! of routes and nothing else, and `Hub::enter_decision`, which decides its
//! loca door before any other rule.

use super::{BuildingRole, EnterDecision, Hub, NameError, RequestAuthority};
use crate::store::{MasterAgentLimits, MasterAgentRecord, MasterAgentSetting, SettingChange};
use crate::sync::RecoverMutex;

/// Why a Master Agent request or a Master's change of the rank was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MasterAgentError {
    /// Only the Master manages Master Agents and their setting.
    MasterOnly,
    /// The Building's Master Agent switch is off.
    TurnedOff,
    /// A Master Agent needs a persistent Building.
    NotPersistent,
    Name(NameError),
    /// Someone already wears that name.
    NameWorn,
    /// A designated floor that is not a live floor of this Building.
    NoSuchFloor,
    /// A limit outside 1 ..= `MasterAgentLimits::CEILING`.
    LimitOutOfRange,
    /// A Master Agent opens a loca only on a live floor the Master named.
    FloorNotDesignated,
    /// It already has as many locas on this floor as the Master allows.
    LocasPerFloor,
    /// It opened as many locas in the last hour as the Master allows.
    LocasPerHour,
    NotFound,
    Storage,
}

/// Who a recorded change was made on behalf of, when a Master Agent made it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct OnBehalfOf {
    pub master_agent: String,
    pub appointed_by: String,
    pub appointed_at: u64,
}

impl MasterAgentError {
    pub fn message(self) -> &'static str {
        match self {
            Self::MasterOnly => "only the Master manages Master Agents",
            Self::TurnedOff => Hub::MASTER_AGENT_OFF,
            Self::NotPersistent => "a Master Agent needs a persistent Building (DB_PATH)",
            Self::Name(NameError::Invalid) => {
                "a name is 1-64 ASCII letters, digits, '-', '_' or '.', so it can be @mentioned"
            }
            Self::Name(NameError::Reserved) => {
                "that word is a title or keyword, not a name — use the automation's own name"
            }
            Self::NameWorn => "someone already wears that name",
            Self::NoSuchFloor => "a designated floor must be a live floor of this Building",
            Self::LimitOutOfRange => "each Master Agent limit is a whole number from 1 to 1000",
            Self::FloorNotDesignated => {
                "a Master Agent opens a loca only on a floor the Master designated for it"
            }
            Self::LocasPerFloor => {
                "this Master Agent has as many locas on this floor as the Master allows"
            }
            Self::LocasPerHour => {
                "this Master Agent opened as many locas in the last hour as the Master allows"
            }
            Self::NotFound => "no such Master Agent",
            Self::Storage => "could not save — try again",
        }
    }
}

impl Hub {
    /// The one sentence every Master Agent credential gets while the switch
    /// is off.
    pub const MASTER_AGENT_OFF: &'static str = "Master Agent is turned off in this Building";

    /// The Master Agent setting (off, with no floors, unless the Master
    /// changed it).
    pub fn master_agent_setting(&self) -> MasterAgentSetting {
        self.store.master_agent_setting()
    }

    /// Is the Building's Master Agent switch on?
    pub fn master_agent_on(&self) -> bool {
        self.master_agent_setting().on
    }

    /// Change the setting. Master only; every floor named must be a live
    /// floor. The change and who made it are journaled together.
    pub fn set_master_agent_setting(
        &self,
        requested: MasterAgentSetting,
        viewer: &RequestAuthority,
    ) -> Result<MasterAgentSetting, MasterAgentError> {
        if !viewer.is_master() {
            return Err(MasterAgentError::MasterOnly);
        }
        if !self.store.is_persistent() {
            return Err(MasterAgentError::NotPersistent);
        }
        let mut floors: Vec<String> = Vec::new();
        for floor in requested.floors {
            if !self.is_open_floor(&floor) {
                return Err(MasterAgentError::NoSuchFloor);
            }
            if !floors.contains(&floor) {
                floors.push(floor);
            }
        }
        let limits = requested.limits;
        let in_range = |n: u32| (1..=MasterAgentLimits::CEILING).contains(&n);
        if ![
            limits.locas_per_floor,
            limits.locas_per_hour,
            limits.admissions_per_hour,
            limits.live_agents,
        ]
        .into_iter()
        .all(in_range)
        {
            return Err(MasterAgentError::LimitOutOfRange);
        }
        let setting = MasterAgentSetting {
            on: requested.on,
            floors,
            limits,
        };
        self.store
            .save_master_agent_setting(&setting, viewer.principal_id.as_deref(), self.now())
            .map_err(|_| MasterAgentError::Storage)?;
        if !setting.on {
            // Their sockets are open on a door that is now shut.
            self.recheck_master_agent_locas(None);
        }
        Ok(setting)
    }

    /// Every change of the setting, newest first. Master only.
    pub fn master_agent_setting_log(
        &self,
        viewer: &RequestAuthority,
    ) -> Result<Vec<SettingChange>, MasterAgentError> {
        if !viewer.is_master() {
            return Err(MasterAgentError::MasterOnly);
        }
        self.store
            .master_agent_setting_log()
            .map_err(|_| MasterAgentError::Storage)
    }

    /// Appoint a Master Agent. Master only, and only while the switch is on.
    /// The key is returned once, here; the store keeps its hash.
    pub fn appoint_master_agent(
        &self,
        name: &str,
        viewer: &RequestAuthority,
    ) -> Result<(String, MasterAgentRecord), MasterAgentError> {
        if !viewer.is_master() {
            return Err(MasterAgentError::MasterOnly);
        }
        if !self.store.is_persistent() {
            return Err(MasterAgentError::NotPersistent);
        }
        if !self.master_agent_on() {
            return Err(MasterAgentError::TurnedOff);
        }
        let name = super::validate_display_name(name.trim()).map_err(MasterAgentError::Name)?;
        let _claim = self.name_claims.lock_or_recover();
        if self.rank_name_clashes(name) {
            return Err(MasterAgentError::NameWorn);
        }
        let code = self.new_invite_token().replacen("dv_", "ma_", 1);
        let appointed_by = viewer
            .principal_id
            .clone()
            .or_else(|| self.store.active_master_principal().map(|p| p.id))
            .unwrap_or_else(|| "master".into());
        let record = self
            .store
            .add_master_agent(&code, name, &appointed_by, self.now())
            .map_err(|_| MasterAgentError::Storage)?;
        Ok((code, record))
    }

    /// Every Master Agent ever appointed. Master only.
    pub fn master_agents(
        &self,
        viewer: &RequestAuthority,
    ) -> Result<Vec<MasterAgentRecord>, MasterAgentError> {
        if !viewer.is_master() {
            return Err(MasterAgentError::MasterOnly);
        }
        self.store
            .master_agents()
            .map_err(|_| MasterAgentError::Storage)
    }

    /// Revoke a Master Agent: its key, its sessions and its principal end.
    /// Master only. Works whether the switch is on or off.
    pub fn revoke_master_agent(
        &self,
        id: &str,
        viewer: &RequestAuthority,
    ) -> Result<(), MasterAgentError> {
        if !viewer.is_master() {
            return Err(MasterAgentError::MasterOnly);
        }
        match self.store.revoke_master_agent(id, self.now()) {
            Ok(Some(principal)) => {
                self.forget_sessions_of(&principal);
                self.recheck_master_agent_locas(Some(&principal));
                Ok(())
            }
            Ok(None) => Err(MasterAgentError::NotFound),
            Err(_) => Err(MasterAgentError::Storage),
        }
    }

    /// Drop the in-memory sessions of a principal whose sessions the store
    /// has just ended, so none of them is honoured from the cache.
    fn forget_sessions_of(&self, principal: &str) {
        self.sessions
            .lock_or_recover()
            .retain(|_, s| s.principal_id.as_deref() != Some(principal));
    }

    /// A Master Agent at a loca door. It enters only a loca it opened
    /// itself, and only while the Building switch is on: not a loca of a
    /// floor it was given that someone else opened, not the Ground floor,
    /// not Care, not a direct message.
    pub(super) fn master_agent_door(&self, room: &str, principal: Option<&str>) -> EnterDecision {
        match principal {
            Some(me)
                if self.master_agent_on()
                    && !self.is_deleted(room)
                    && self.store.master_agent_loca_opener(room).as_deref() == Some(me) =>
            {
                EnterDecision::Allowed
            }
            _ => EnterDecision::Denied,
        }
    }

    /// Did this authority, a Master Agent's, open `room`? (While the switch
    /// is off nothing a Master Agent opened counts as its.)
    pub fn master_agent_opened(&self, room: &str, authority: &RequestAuthority) -> bool {
        Self::is_master_agent(authority)
            && self.master_agent_door(room, authority.principal_id.as_deref())
                == EnterDecision::Allowed
    }

    /// May a Master Agent open a loca on `group`: the switch is on, the
    /// Master designated the floor, and the floor is live and not archived.
    /// It also stays within the Master's limits: locas open on that floor,
    /// and locas opened in the last hour.
    pub(super) fn master_agent_may_open_in(
        &self,
        group: &str,
        actor: &RequestAuthority,
    ) -> Result<(), MasterAgentError> {
        let setting = self.master_agent_setting();
        if !setting.on {
            return Err(MasterAgentError::TurnedOff);
        }
        if !setting.floors.iter().any(|floor| floor == group) || !self.is_open_floor(group) {
            return Err(MasterAgentError::FloorNotDesignated);
        }
        let Some(me) = actor.principal_id.as_deref() else {
            return Err(MasterAgentError::FloorNotDesignated);
        };
        if self.store.master_agent_locas_on_floor(me, group) >= setting.limits.locas_per_floor {
            return Err(MasterAgentError::LocasPerFloor);
        }
        let hour_ago = self.now().saturating_sub(Self::HOUR_MS);
        if self.store.master_agent_locas_since(me, hour_ago) >= setting.limits.locas_per_hour {
            return Err(MasterAgentError::LocasPerHour);
        }
        Ok(())
    }

    /// The window of the per-hour limits.
    pub const HOUR_MS: u64 = 60 * 60 * 1000;

    /// On whose behalf a recorded change was made, when `principal` is a
    /// Master Agent's (revoked ones included: the record outlives the key).
    pub fn on_behalf_of(&self, principal: &str) -> Option<OnBehalfOf> {
        let record = self.store.master_agent_by_principal(principal)?;
        let appointed_by = self
            .store
            .active_principal(&record.appointed_by)
            .map(|p| p.display_name)
            .unwrap_or_else(|| "Master".into());
        Some(OnBehalfOf {
            master_agent: record.name,
            appointed_by,
            appointed_at: record.appointed_at,
        })
    }

    /// The author a lead announcement names for a Master Agent:
    /// `"<name> … on behalf of Master Agent <name>, appointed by <who>"`
    /// reads as "automaton set @lead envoy on behalf of Master Agent
    /// automaton, appointed by Master."
    pub fn master_agent_lead_suffix(&self, authority: &RequestAuthority) -> Option<String> {
        let principal = authority.principal_id.as_deref()?;
        let agent = self.on_behalf_of(principal)?;
        Some(format!(
            " on behalf of Master Agent {}, appointed by {}",
            agent.master_agent, agent.appointed_by
        ))
    }

    /// Make the live sockets in a Master Agent's locas check their door now
    /// (after the switch went off or a Master Agent was revoked).
    fn recheck_master_agent_locas(&self, principal: Option<&str>) {
        for room in self.store.master_agent_rooms(principal) {
            self.recheck_connections(&room);
        }
    }

    /// Is this authority a Master Agent's?
    pub fn is_master_agent(authority: &RequestAuthority) -> bool {
        authority.building_role == Some(BuildingRole::MasterAgent)
    }
}
