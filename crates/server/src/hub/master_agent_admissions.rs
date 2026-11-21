//! A Master Agent's third act: it admits an agent into a loca it opened, in
//! one call — the agent member is created, put on the loca's floor and called
//! into the loca, and its key comes back to the Master Agent once (the same
//! claim collects the same key until acknowledged). It may also call an agent
//! it admitted itself into another loca it opened. Never a human, never a
//! loca it did not open, never on a floor taken off the Master's list, never
//! past the Master's limits.
//!
//! Each act still leaves its own record (PRINCIPLES "Membership layer":
//! *admit & invite* "leaves two records"): the membership, the floor's
//! `member_added`, the invite. All of them name the Master Agent.

use super::master_agent::MasterAgentError;
use super::{Hub, InviteError, InviteRecord, MemberRecord, RequestAuthority};
use crate::store::AdmissionClaim;
use crate::sync::RecoverMutex;

/// What a Master Agent gets back from an admission.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Admission {
    pub member_id: String,
    pub principal_id: Option<String>,
    pub name: String,
    /// The member's key; only in this answer, and only while the claim may
    /// still collect it.
    pub key: String,
    pub group: String,
    pub loca: String,
    pub invite_id: Option<String>,
    /// False when the same claim collected its key again.
    #[serde(skip)]
    pub created: bool,
}

/// Why an admission (or a Master Agent's call) was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionError {
    /// Not a loca this Master Agent opened (answered as a loca that is not
    /// there).
    NotItsLoca,
    /// The switch is off, or the floor was taken off the list.
    Refused(MasterAgentError),
    /// Agents only.
    NotAnAgent,
    /// A claim id is required, and has a fixed shape.
    BadClaim,
    /// The name breaks the admission rule.
    Name(&'static str),
    /// Someone already has that name, or the claim was used for another
    /// one, or its key was acknowledged or its window closed.
    Taken,
    /// An agent it did not admit, or one not on that floor.
    NotItsAgent,
    /// The Master's live-agents limit.
    LiveAgents,
    /// The Master's admissions-per-hour limit.
    AdmissionsPerHour,
    /// The loca is full.
    Full,
    Storage,
}

impl AdmissionError {
    pub fn message(&self) -> &'static str {
        match self {
            Self::NotItsLoca => "no such loca",
            Self::Refused(error) => error.message(),
            Self::NotAnAgent => "a Master Agent admits agents only",
            Self::BadClaim => {
                "claim_id must be cl_ followed by 32-128 letters, digits, '-' or '_'"
            }
            Self::Name(rule) => rule,
            Self::Taken => {
                "that name is taken, or this claim's key was already delivered — nothing was admitted"
            }
            Self::NotItsAgent => {
                "a Master Agent calls only agents it admitted itself, on that loca's floor"
            }
            Self::LiveAgents => "this Master Agent has as many live agents as the Master allows",
            Self::AdmissionsPerHour => {
                "this Master Agent admitted as many agents in the last hour as the Master allows"
            }
            Self::Full => "this loca is full — no free seat",
            Self::Storage => "could not save — try again",
        }
    }
}

/// A claim id the caller chose: `cl_` and 32 to 128 URL-safe characters
/// (the shape of a membership claim's), so a key or an id is never taken
/// for one.
fn valid_claim(claim: &str) -> bool {
    claim.strip_prefix("cl_").is_some_and(|body| {
        (32..=128).contains(&body.len())
            && body
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    })
}

impl Hub {
    /// How a Master Agent is named in the records it writes:
    /// `master-agent:<name>`.
    pub fn master_agent_label(authority: &RequestAuthority) -> String {
        format!(
            "master-agent:{}",
            authority.display_name.as_deref().unwrap_or("unknown")
        )
    }

    /// The loca's floor, when `room` is a loca this Master Agent opened on a
    /// floor that is still designated (a floor taken off the list keeps its
    /// locas reachable, but takes no new admission or call).
    fn master_agent_floor_for(
        &self,
        room: &str,
        actor: &RequestAuthority,
    ) -> Result<String, AdmissionError> {
        if !self.master_agent_opened(room, actor) {
            return Err(AdmissionError::NotItsLoca);
        }
        let floor = self.group_of(room).ok_or(AdmissionError::NotItsLoca)?;
        let setting = self.master_agent_setting();
        if !setting.on {
            return Err(AdmissionError::Refused(MasterAgentError::TurnedOff));
        }
        if !setting.floors.contains(&floor) || !self.is_open_floor(&floor) {
            return Err(AdmissionError::Refused(
                MasterAgentError::FloorNotDesignated,
            ));
        }
        Ok(floor)
    }

    /// Admit an agent into a loca this Master Agent opened, or hand the same
    /// claim its key again.
    pub fn master_agent_admit(
        &self,
        room: &str,
        name: &str,
        kind: Option<&str>,
        claim_id: Option<&str>,
        actor: &RequestAuthority,
    ) -> Result<Admission, AdmissionError> {
        let floor = self.master_agent_floor_for(room, actor)?;
        let me = actor
            .principal_id
            .clone()
            .ok_or(AdmissionError::NotItsLoca)?;
        if kind.is_some_and(|k| k != "agent") {
            return Err(AdmissionError::NotAnAgent);
        }
        let claim_id = claim_id
            .filter(|c| valid_claim(c))
            .ok_or(AdmissionError::BadClaim)?;
        let name = name.trim();
        let label = Self::master_agent_label(actor);

        // The same claim again: the same member, its key while the window
        // is open, and the floor and seat put right if a step was missed.
        let held = self
            .store
            .admission_claim(claim_id, self.now())
            .map_err(|_| AdmissionError::Storage)?;
        if let Some(held) = held {
            return self.reissue_claim(held, claim_id, &me, room, name, &floor, &label);
        }

        // Count, admit and record as one step (M1 of the review: without it
        // concurrent admissions all passed the same count).
        let _admitting = self.master_agent_admitting.lock_or_recover();
        if let Some(held) = self
            .store
            .admission_claim(claim_id, self.now())
            .map_err(|_| AdmissionError::Storage)?
        {
            return self.reissue_claim(held, claim_id, &me, room, name, &floor, &label);
        }

        if let Some(rule) = super::admission_name_refusal(name) {
            return Err(AdmissionError::Name(rule));
        }
        if self.member_by_name(name).is_some() {
            return Err(AdmissionError::Taken);
        }
        let limits = self.master_agent_setting().limits;
        // Counted by its principal, not its display name: a later Master
        // Agent that takes a revoked one's name inherits nothing.
        let (live, recent) = self
            .store
            .admissions_by(&me, self.now().saturating_sub(Self::HOUR_MS));
        if live >= limits.live_agents {
            return Err(AdmissionError::LiveAgents);
        }
        if recent >= limits.admissions_per_hour {
            return Err(AdmissionError::AdmissionsPerHour);
        }
        let (member, key) = match self.admit_member_once(name, "agent", &label) {
            Ok((member, Some(key))) => (member, key),
            Ok((_, None)) | Err(super::AdmitError::Worn) => return Err(AdmissionError::Taken),
            Err(super::AdmitError::Rule(rule)) => return Err(AdmissionError::Name(rule)),
            Err(super::AdmitError::Storage) => return Err(AdmissionError::Storage),
        };
        // The claim before anything else, so a retry finds this member.
        self.store
            .record_admission_claim(
                claim_id,
                &me,
                room,
                &member.id,
                &member.name,
                &key,
                self.now(),
            )
            .map_err(|_| AdmissionError::Storage)?;
        let invite_id = self.seat_admitted(&member, room, &floor, &me, &label)?;
        self.store
            .mark_admission_seated(claim_id, self.now())
            .map_err(|_| AdmissionError::Storage)?;
        Ok(Admission {
            member_id: member.id,
            principal_id: member.principal_id,
            name: member.name,
            key,
            group: floor,
            loca: room.to_string(),
            invite_id,
            created: true,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn reissue_claim(
        &self,
        held: AdmissionClaim,
        claim_id: &str,
        me: &str,
        room: &str,
        name: &str,
        floor: &str,
        label: &str,
    ) -> Result<Admission, AdmissionError> {
        if held.principal_id != me || held.room != room || held.name != name {
            return Err(AdmissionError::Taken);
        }
        let Some(key) = held.key else {
            return Err(AdmissionError::Taken);
        };
        let member = self
            .members
            .lock_or_recover()
            .get(&held.member_id)
            .cloned()
            .ok_or(AdmissionError::Taken)?;
        // The floor and seat steps are repeated only if they never completed:
        // replaying a claim must not put back an agent the Master took off
        // the floor or out of the loca since (M3 of the review).
        let invite_id = if held.seated_at.is_none() {
            let invite_id = self.seat_admitted(&member, room, floor, me, label)?;
            self.store
                .mark_admission_seated(claim_id, self.now())
                .map_err(|_| AdmissionError::Storage)?;
            invite_id
        } else {
            self.invites_for_member(&member.id)
                .into_iter()
                .find(|invite| invite.room == room)
                .map(|invite| invite.id)
        };
        Ok(Admission {
            member_id: member.id,
            principal_id: member.principal_id,
            name: member.name,
            key,
            group: floor.to_string(),
            loca: room.to_string(),
            invite_id,
            created: false,
        })
    }

    /// Put an admitted agent on the floor and call it into the loca (both
    /// idempotent). Returns the invite's management id.
    fn seat_admitted(
        &self,
        member: &MemberRecord,
        room: &str,
        floor: &str,
        me: &str,
        label: &str,
    ) -> Result<Option<String>, AdmissionError> {
        let principal = member
            .principal_id
            .as_deref()
            .ok_or(AdmissionError::Storage)?;
        self.put_admitted_agent_on_floor(floor, principal, me)
            .map_err(|_| AdmissionError::Refused(MasterAgentError::FloorNotDesignated))?;
        match self.invite_member_to_room(&member.id, room, label) {
            Ok((invite, _)) => Ok(Some(invite.id)),
            Err(InviteError::AlreadyInvited) => Ok(self
                .invites_for_member(&member.id)
                .into_iter()
                .find(|invite| invite.room == room)
                .map(|invite| invite.id)),
            Err(InviteError::Full) => Err(AdmissionError::Full),
            Err(_) => Err(AdmissionError::Storage),
        }
    }

    /// The Master Agent acknowledged a key it collected.
    pub fn master_agent_ack_admission(
        &self,
        room: &str,
        member_id: &str,
        claim_id: &str,
        actor: &RequestAuthority,
    ) -> Result<(), AdmissionError> {
        if !self.master_agent_opened(room, actor) {
            return Err(AdmissionError::NotItsLoca);
        }
        let me = actor
            .principal_id
            .as_deref()
            .ok_or(AdmissionError::NotItsLoca)?;
        match self
            .store
            .ack_admission_claim(claim_id, me, member_id, self.now())
        {
            Ok(true) => Ok(()),
            Ok(false) => Err(AdmissionError::BadClaim),
            Err(_) => Err(AdmissionError::Storage),
        }
    }

    /// A Master Agent calls an agent it admitted itself into another loca it
    /// opened on the same floor. Returns the new invite and the invite
    /// itself, or `None` when the agent already holds one for this loca.
    pub fn master_agent_call(
        &self,
        room: &str,
        name: &str,
        actor: &RequestAuthority,
    ) -> Result<Option<(InviteRecord, String)>, AdmissionError> {
        let floor = self.master_agent_floor_for(room, actor)?;
        let label = Self::master_agent_label(actor);
        let me = actor
            .principal_id
            .as_deref()
            .ok_or(AdmissionError::NotItsAgent)?;
        // Its own agent by principal, not by the display name it wears: a
        // Master Agent appointed later under a revoked one's name calls none
        // of the revoked one's agents (M2 of the review).
        let member = self
            .member_by_name(name.trim())
            .filter(|m| self.store.admitted_by_principal(&m.id, me))
            .ok_or(AdmissionError::NotItsAgent)?;
        let on_floor = member
            .principal_id
            .as_deref()
            .is_some_and(|p| self.groups_of_principal(p).iter().any(|g| g.id == floor));
        if !on_floor {
            return Err(AdmissionError::NotItsAgent);
        }
        match self.invite_member_to_room(&member.id, room, &label) {
            Ok(minted) => Ok(Some(minted)),
            Err(InviteError::AlreadyInvited) => Ok(None),
            Err(InviteError::Full) => Err(AdmissionError::Full),
            Err(_) => Err(AdmissionError::Storage),
        }
    }
}
