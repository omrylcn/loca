//! Mentat rooms (`protocol::MentatRoom`): a loca born as the place where one
//! expert agent, the mentat, is consulted in turn. Asking opens a request;
//! requests wait in a line in the order they came; one consultation runs at
//! a time. Askers who want the same thing join one request and go in
//! together. The line orders access to one expert: it assigns no work and
//! creates no task.
//!
//! A mentat room is a kind of loca, like a birebir. It is one from its birth
//! and stays one; it has no Operator seat. The Master, or the Smaster who
//! founded it, appoints its mentat and sets its limits; the mentat runs the
//! line (ends the consultation inside, merges waiting requests, pauses and
//! resumes). A room may wait without a mentat: then nobody can ask.
//!
//! The line owns the room's chat mode: while a consultation runs, only the
//! mentat and that request's askers may write (`ChatMode::Restricted`); in
//! between, only the mentat. No rank writes past the line here (`post` does
//! not let `is_admin` through in a mentat room): a rank who wants to ask
//! opens a request like anyone else. A pause freezes the line; any other
//! mode resumes it. The server enforces every limit (`MentatLimits`) from
//! the 15-second sweep (`expire_mentat`) and from the messages themselves
//! (`mentat_after_post`). The lines that call askers name them with `@name`,
//! which is what wakes a mentions-filtered agent; topics are never part of a
//! line, so an `@name` in a topic wakes nobody.

use std::sync::atomic::Ordering;

use protocol::{
    ChatMode, MentatAnswered, MentatEndReason, MentatLimits, MentatListing, MentatRequest,
    MentatRequestStatus, MentatRoom, MentatState, Message, NewMentatRoom, SenderType, ServerFrame,
    SetMentatRoom,
};

use super::{CreateLocaError, Hub, IdentityEdit, RequestAuthority, Room, HISTORY_LIMIT};
use crate::sync::RecoverMutex;

/// A topic is one line: of a room (what its expert knows) or of a request
/// (what its askers want to ask).
pub const MENTAT_TOPIC_MIN_CHARS: usize = 10;
pub const MENTAT_TOPIC_MAX_CHARS: usize = 200;
/// The most requests one line holds at once.
pub const MENTAT_LINE_MAX: usize = 50;
/// How many closed requests `GET /rooms/:id/mentat/history` answers.
pub const MENTAT_HISTORY_LIMIT: usize = 50;
/// The longest any limit may be set to: a day, or 100 questions.
const MENTAT_LIMIT_MAX_SECS: u32 = 86_400;
const MENTAT_QUESTIONS_MAX: u32 = 100;
/// Consultations in a row the mentat wrote nothing in before the line
/// pauses itself: a mentat that is connected but does not answer must not
/// use up everybody's turn.
const MENTAT_SILENT_PAUSE: u8 = 2;
/// The extensions a mentat room is born with, and the ones it never takes:
/// it is a place to consult, not to hand out work.
const MENTAT_EXTENSIONS: [&str; 2] = ["wiki", "vault"];
pub const MENTAT_NO_EXTENSIONS: [&str; 3] = ["board", "focus", "live"];

/// A mentat room as the hub keeps it, on its `Room`.
#[derive(Debug, Clone)]
pub(in crate::hub) struct MentatLive {
    pub room: MentatRoom,
    /// The consultation called or under way.
    pub current: Option<MentatRequest>,
    /// The line, oldest first.
    pub waiting: Vec<MentatRequest>,
    pub next_id: u64,
    /// Who says it is replying right now (`Hub::typing`). While the mentat
    /// or an asker inside does, silence is not counted. Not stored.
    replying: Vec<String>,
    /// Consultations in a row that ended with no word from the mentat.
    silent: u8,
}

impl MentatLive {
    pub(in crate::hub) fn new(room: MentatRoom, open: Vec<MentatRequest>, last_id: u64) -> Self {
        let mut current = None;
        let mut waiting = Vec::new();
        for request in open {
            match request.status {
                MentatRequestStatus::Called | MentatRequestStatus::Active if current.is_none() => {
                    current = Some(request)
                }
                _ => waiting.push(request),
            }
        }
        Self {
            room,
            current,
            waiting,
            next_id: last_id + 1,
            replying: Vec::new(),
            silent: 0,
        }
    }

    fn is_mentat(&self, name: &str) -> bool {
        self.room.mentat.as_deref() == Some(name)
    }

    /// Is `name` an asker of an open request (in the line or inside)?
    fn holds_request(&self, name: &str) -> bool {
        self.current
            .iter()
            .chain(self.waiting.iter())
            .any(|r| r.owners.iter().any(|o| o == name))
    }

    /// Who may write now: the mentat, and the askers inside unless the
    /// consultation is closing.
    fn allow(&self) -> Vec<String> {
        let mut allow: Vec<String> = self.room.mentat.iter().cloned().collect();
        if let Some(current) = &self.current {
            if !current.closing {
                for owner in &current.owners {
                    if !allow.contains(owner) {
                        allow.push(owner.clone());
                    }
                }
            }
        }
        allow
    }

    /// Does anyone inside say it is replying?
    fn someone_replying(&self) -> bool {
        self.current.as_ref().is_some_and(|current| {
            self.replying
                .iter()
                .any(|name| self.is_mentat(name) || current.owners.contains(name))
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MentatError {
    /// This loca is not a mentat room.
    NotMentatRoom,
    /// The room waits for a mentat: nobody can ask yet.
    NoMentat,
    BadLimits,
    EmptyTopic,
    TopicTooShort,
    TopicTooLong,
    /// A character that hides itself or breaks a line.
    InvalidTopic,
    /// Nobody in the Building wears that name.
    NoSuchAgent,
    /// The name is a person's: a mentat is an agent.
    NotAnAgent,
    /// More than one member wears that name.
    AmbiguousName,
    /// The agent is already the mentat of another room.
    MentatElsewhere,
    /// The mentat does not consult itself.
    IsMentat,
    /// The asker already has a request in the line or inside.
    AlreadyInLine,
    LineFull,
    NoSuchRequest,
    /// Only a request still in the line can be joined, merged or withdrawn.
    NotWaiting,
    /// Nobody is inside.
    NoConsultation,
    /// Only its askers, the mentat or a rank who manages the room.
    NotYours,
    /// A request cannot be merged into itself.
    SameRequest,
    Storage,
}

impl MentatError {
    pub fn message(&self) -> String {
        match self {
            Self::NotMentatRoom => "this loca is not a mentat room".into(),
            Self::NoMentat => {
                "this room has no mentat yet: nobody can ask until the Master or the Smaster who \
                 founded it appoints one"
                    .into()
            }
            Self::BadLimits => format!(
                "limits are whole seconds up to {MENTAT_LIMIT_MAX_SECS} and at most \
                 {MENTAT_QUESTIONS_MAX} questions"
            ),
            Self::EmptyTopic => "say what it is about, in one line".into(),
            Self::TopicTooShort => {
                format!(
                    "a topic says what it is about: at least {MENTAT_TOPIC_MIN_CHARS} characters"
                )
            }
            Self::TopicTooLong => {
                format!("a topic is one line: at most {MENTAT_TOPIC_MAX_CHARS} characters")
            }
            Self::InvalidTopic => {
                "a topic must be plain text, with no control or invisible formatting characters"
                    .into()
            }
            Self::NoSuchAgent => "no agent in this Building wears that name".into(),
            Self::NotAnAgent => "a mentat is an agent: that name is a person's".into(),
            Self::AmbiguousName => "more than one member wears that name".into(),
            Self::MentatElsewhere => {
                "that agent is already the mentat of another room: one room each".into()
            }
            Self::IsMentat => "the mentat does not consult itself".into(),
            Self::AlreadyInLine => {
                "you already have a request in this line; wait for it, end it or withdraw it".into()
            }
            Self::LineFull => {
                format!("{MENTAT_LINE_MAX} requests already wait here; try again later")
            }
            Self::NoSuchRequest => "no such request in this line".into(),
            Self::NotWaiting => {
                "only a request still waiting in the line can be joined, merged or withdrawn".into()
            }
            Self::NoConsultation => "nobody is consulting the mentat right now".into(),
            Self::NotYours => {
                "only its askers, the mentat, the Master or the Smaster who founded this room"
                    .into()
            }
            Self::SameRequest => "a request cannot be merged into itself".into(),
            Self::Storage => "could not save the line — try again".into(),
        }
    }
}

/// Founding a mentat room failed: as a loca, or as a mentat room.
#[derive(Debug)]
pub enum FoundMentatError {
    Loca(CreateLocaError),
    Mentat(MentatError),
}

/// The topic as kept, or why it cannot be: one plain line.
fn clean_topic(topic: &str) -> Result<String, MentatError> {
    let topic = topic.split_whitespace().collect::<Vec<_>>().join(" ");
    if topic.is_empty() {
        return Err(MentatError::EmptyTopic);
    }
    let chars = topic.chars().count();
    if chars < MENTAT_TOPIC_MIN_CHARS {
        return Err(MentatError::TopicTooShort);
    }
    if chars > MENTAT_TOPIC_MAX_CHARS {
        return Err(MentatError::TopicTooLong);
    }
    if topic
        .chars()
        .any(|c| c.is_control() || matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}'))
    {
        return Err(MentatError::InvalidTopic);
    }
    Ok(topic)
}

fn checked_limits(limits: MentatLimits) -> Result<MentatLimits, MentatError> {
    let limits = limits.filled();
    let secs = [
        limits.session_secs,
        limits.idle_secs,
        limits.call_secs,
        limits.wait_secs,
        limits.close_secs,
    ];
    if secs.iter().any(|s| *s > MENTAT_LIMIT_MAX_SECS) || limits.questions > MENTAT_QUESTIONS_MAX {
        return Err(MentatError::BadLimits);
    }
    Ok(limits)
}

fn reason_words(reason: MentatEndReason) -> &'static str {
    match reason {
        MentatEndReason::Asker => "its asker ended it",
        MentatEndReason::Mentat => "the mentat ended it",
        MentatEndReason::Rank => "ended by a rank",
        MentatEndReason::Time => "its time ran out",
        MentatEndReason::Questions => "its questions were used up",
        MentatEndReason::Idle => "nobody wrote for too long",
        MentatEndReason::Closed => "the room has no mentat now",
        MentatEndReason::Merged => "it was joined to another request",
    }
}

fn after(since: Option<u64>, secs: u32, now: u64) -> bool {
    since.is_some_and(|t| now.saturating_sub(t) > u64::from(secs) * 1000)
}

impl Hub {
    /// The line of `room`, or `None` when it is not a mentat room.
    pub fn mentat_state(&self, room: &str) -> Option<MentatState> {
        let rooms = self.rooms.lock_or_recover();
        let r = rooms.get(room)?;
        Self::state_of(r)
    }

    fn state_of(r: &Room) -> Option<MentatState> {
        let live = r.mentat.as_ref()?;
        Some(MentatState {
            mentat: live.room.clone(),
            paused: matches!(r.mode, ChatMode::Paused),
            current: live.current.clone(),
            waiting: live.waiting.clone(),
        })
    }

    /// Is `room` a mentat room (with or without a mentat)?
    pub fn is_mentat_room(&self, room: &str) -> bool {
        let rooms = self.rooms.lock_or_recover();
        rooms.get(room).is_some_and(|r| r.mentat.is_some())
    }

    /// Is `name` the mentat of `room`? The mentat reads its whole room, as a
    /// Lead does, so the askers need not name it in every question.
    pub fn is_mentat(&self, room: &str, name: &str) -> bool {
        let rooms = self.rooms.lock_or_recover();
        rooms
            .get(room)
            .and_then(|r| r.mentat.as_ref())
            .is_some_and(|live| live.is_mentat(name))
    }

    /// Does `name` hear `message` in mentat room `room` without being named?
    /// The mentat hears what people write in its room, and the askers of the
    /// consultation inside hear it too, so a question and its answer need no
    /// `@`. The server's own lines wake only whom they address.
    pub fn mentat_hears(&self, room: &str, name: &str, message: &Message) -> bool {
        if message.sender == "loca" {
            return false;
        }
        let rooms = self.rooms.lock_or_recover();
        rooms
            .get(room)
            .and_then(|r| r.mentat.as_ref())
            .is_some_and(|live| {
                live.is_mentat(name)
                    || live
                        .current
                        .as_ref()
                        .is_some_and(|c| c.owners.iter().any(|o| o == name))
            })
    }

    /// May `actor` manage mentat room `room` (appoint its mentat, set its
    /// topic and limits, end a consultation as a rank)? The Master always;
    /// a Smaster only in a room it founded.
    pub fn may_manage_mentat(&self, room: &str, actor: &RequestAuthority) -> bool {
        if actor.is_master() {
            return true;
        }
        if !actor.is_building_admin() {
            return false;
        }
        let rooms = self.rooms.lock_or_recover();
        let founder = rooms
            .get(room)
            .and_then(|r| r.mentat.as_ref())
            .and_then(|live| live.room.founder.clone());
        founder.is_some() && founder == actor.principal_id
    }

    /// Every mentat room of the Building, as its directory lists them: the
    /// room, its topic, its mentat and how long the line is. Nothing of what
    /// is said inside.
    pub fn mentat_directory(&self) -> Vec<MentatListing> {
        let rooms = self.rooms.lock_or_recover();
        let mut list: Vec<MentatListing> = rooms
            .iter()
            .filter(|(_, r)| !r.settings.archived)
            .filter_map(|(name, r)| {
                let live = r.mentat.as_ref()?;
                let online = live.room.mentat.as_deref().is_some_and(|mentat| {
                    r.members
                        .values()
                        .any(|(seat, _, conns)| seat == mentat && !conns.is_empty())
                });
                Some(MentatListing {
                    room: name.clone(),
                    topic: live.room.topic.clone(),
                    mentat: live.room.mentat.clone(),
                    online,
                    paused: matches!(r.mode, ChatMode::Paused),
                    busy: live.current.is_some(),
                    waiting: live.waiting.len(),
                })
            })
            .collect();
        list.sort_by(|a, b| a.room.cmp(&b.room));
        list
    }

    /// The requests of `room` that are over, newest first.
    pub fn mentat_history(&self, room: &str) -> Result<Vec<MentatRequest>, MentatError> {
        if !self.is_mentat_room(room) {
            return Err(MentatError::NotMentatRoom);
        }
        self.store
            .closed_mentat_requests(room, MENTAT_HISTORY_LIMIT)
            .map_err(|_| MentatError::Storage)
    }

    /// Who `name` is as a mentat: the one agent wearing it. `None` where
    /// nobody can be verified (the open sandbox, a memory-only server).
    fn mentat_principal(&self, name: &str) -> Result<Option<String>, MentatError> {
        let identities = self
            .store
            .active_principals_named(name)
            .map_err(|_| MentatError::Storage)?;
        match identities.as_slice() {
            [one] if one.kind == SenderType::Agent => Ok(Some(one.id.clone())),
            [_] => Err(MentatError::NotAnAgent),
            [] if !self.store.is_persistent() || self.admin_open() => Ok(None),
            [] => Err(MentatError::NoSuchAgent),
            _ => Err(MentatError::AmbiguousName),
        }
    }

    /// Is this agent already the mentat of a room other than `room`?
    fn mentat_elsewhere(&self, room: &str, name: &str, principal: Option<&str>) -> bool {
        let rooms = self.rooms.lock_or_recover();
        rooms.iter().any(|(other, r)| {
            other != room
                && r.mentat.as_ref().is_some_and(|live| {
                    match (principal, live.room.mentat_principal.as_deref()) {
                        (Some(mine), Some(theirs)) => mine == theirs,
                        _ => live.is_mentat(name),
                    }
                })
        })
    }

    /// Found a mentat room: a new loca, born with its topic and, when one is
    /// named, its mentat. It gets no Operator.
    pub fn found_mentat_room(
        &self,
        room: &str,
        actor: &RequestAuthority,
        group: Option<&str>,
        identity: &IdentityEdit,
        asked: &NewMentatRoom,
    ) -> Result<MentatState, FoundMentatError> {
        let topic = clean_topic(&asked.topic).map_err(FoundMentatError::Mentat)?;
        let mentat = asked
            .mentat
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty());
        let principal = match mentat {
            Some(name) => {
                let principal = self
                    .mentat_principal(name)
                    .map_err(FoundMentatError::Mentat)?;
                if self.mentat_elsewhere(room, name, principal.as_deref()) {
                    return Err(FoundMentatError::Mentat(MentatError::MentatElsewhere));
                }
                principal
            }
            None => None,
        };
        let by = actor
            .display_name
            .clone()
            .unwrap_or_else(|| "master".to_string());
        let record = MentatRoom {
            room: room.to_string(),
            topic,
            mentat: mentat.map(str::to_string),
            mentat_principal: principal,
            founder: actor.principal_id.clone(),
            limits: MentatLimits::default(),
            set_by: by.clone(),
            set_at: (self.now_ms)(),
        };
        self.open_loca(
            room,
            actor,
            Some(MENTAT_EXTENSIONS.iter().map(|id| id.to_string()).collect()),
            group,
            identity,
            Some(record),
        )
        .map_err(FoundMentatError::Loca)?;
        let mut rooms = self.rooms.lock_or_recover();
        let r = rooms
            .get_mut(room)
            .ok_or(FoundMentatError::Mentat(MentatError::NotMentatRoom))?;
        self.apply_mentat(r, room);
        if let Some(name) = mentat {
            self.announce_line(
                r,
                room,
                Some(name),
                format!("{by} founded this mentat room; @{name} is its mentat."),
            );
        }
        Self::state_of(r).ok_or(FoundMentatError::Mentat(MentatError::NotMentatRoom))
    }

    /// Change a mentat room: appoint its mentat, or set its topic or its
    /// limits. Absent fields stay as they are; its line is kept. A new mentat
    /// ends the consultation the old one was in.
    pub fn set_mentat_room(
        &self,
        room: &str,
        asked: &SetMentatRoom,
        by: &str,
    ) -> Result<MentatState, MentatError> {
        let topic = asked.topic.as_deref().map(clean_topic).transpose()?;
        let limits = asked.limits.map(checked_limits).transpose()?;
        let mentat = asked
            .mentat
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty());
        let principal = match mentat {
            Some(name) => {
                let principal = self.mentat_principal(name)?;
                if self.mentat_elsewhere(room, name, principal.as_deref()) {
                    return Err(MentatError::MentatElsewhere);
                }
                principal
            }
            None => None,
        };
        let now = (self.now_ms)();
        let mut rooms = self.rooms.lock_or_recover();
        let r = rooms.get_mut(room).ok_or(MentatError::NotMentatRoom)?;
        let live = r.mentat.as_ref().ok_or(MentatError::NotMentatRoom)?;
        let mut record = live.room.clone();
        let appointed = mentat.filter(|name| !live.is_mentat(name));
        if let Some(topic) = topic {
            record.topic = topic;
        }
        if let Some(limits) = limits {
            record.limits = limits;
        }
        if let Some(name) = appointed {
            record.mentat = Some(name.to_string());
            record.mentat_principal = principal;
        }
        record.set_by = by.to_string();
        record.set_at = now;
        self.store
            .save_mentat_room(&record)
            .map_err(|_| MentatError::Storage)?;
        if appointed.is_some() {
            // The consultation inside was with the mentat before.
            self.finish_current(
                r,
                room,
                MentatRequestStatus::Ended,
                MentatEndReason::Closed,
                None,
                now,
            );
        }
        if let Some(live) = r.mentat.as_mut() {
            live.room = record;
            if appointed.is_some() {
                live.silent = 0;
                live.replying.clear();
            }
        }
        if let Some(name) = appointed {
            self.mirror_mentat(r, room);
            self.announce_line(
                r,
                room,
                Some(name),
                format!("{by} appointed @{name} this room's mentat."),
            );
        }
        self.advance_mentat(r, room, now);
        self.apply_mentat(r, room);
        Self::state_of(r).ok_or(MentatError::NotMentatRoom)
    }

    /// Take the mentat's appointment back. The room stays a mentat room and
    /// waits for one: the consultation inside ends, the line is kept, and
    /// nobody can ask until a mentat is appointed.
    pub fn clear_mentat(&self, room: &str, by: &str) -> Result<MentatState, MentatError> {
        let now = (self.now_ms)();
        let mut rooms = self.rooms.lock_or_recover();
        let r = rooms.get_mut(room).ok_or(MentatError::NotMentatRoom)?;
        let live = r.mentat.as_ref().ok_or(MentatError::NotMentatRoom)?;
        if live.room.mentat.is_none() {
            return Err(MentatError::NoMentat);
        }
        let mut record = live.room.clone();
        record.mentat = None;
        record.mentat_principal = None;
        record.set_by = by.to_string();
        record.set_at = now;
        self.store
            .save_mentat_room(&record)
            .map_err(|_| MentatError::Storage)?;
        self.finish_current(
            r,
            room,
            MentatRequestStatus::Ended,
            MentatEndReason::Closed,
            None,
            now,
        );
        if let Some(live) = r.mentat.as_mut() {
            live.room = record;
            live.replying.clear();
            live.silent = 0;
        }
        self.mirror_mentat(r, room);
        self.announce_line(
            r,
            room,
            None,
            format!("{by} took the mentat's appointment back; the line waits for a new mentat."),
        );
        self.apply_mentat(r, room);
        Self::state_of(r).ok_or(MentatError::NotMentatRoom)
    }

    /// `asker` asks to consult the mentat about `topic`.
    pub fn open_mentat_request(
        &self,
        room: &str,
        asker: &str,
        topic: &str,
    ) -> Result<MentatRequest, MentatError> {
        let topic = clean_topic(topic)?;
        let now = (self.now_ms)();
        let mut rooms = self.rooms.lock_or_recover();
        let r = rooms.get_mut(room).ok_or(MentatError::NotMentatRoom)?;
        let live = r.mentat.as_mut().ok_or(MentatError::NotMentatRoom)?;
        if live.room.mentat.is_none() {
            return Err(MentatError::NoMentat);
        }
        if live.is_mentat(asker) {
            return Err(MentatError::IsMentat);
        }
        if live.holds_request(asker) {
            return Err(MentatError::AlreadyInLine);
        }
        if live.waiting.len() >= MENTAT_LINE_MAX {
            return Err(MentatError::LineFull);
        }
        let request = MentatRequest {
            id: live.next_id,
            room: room.to_string(),
            topic,
            owners: vec![asker.to_string()],
            status: MentatRequestStatus::Waiting,
            opened_by: asker.to_string(),
            opened_at: now,
            called_at: None,
            started_at: None,
            ended_at: None,
            questions: 0,
            answers: 0,
            awaiting: false,
            last_activity: None,
            closing: false,
            closing_at: None,
            ended_by: None,
            end_reason: None,
            answered: None,
        };
        self.store
            .save_mentat_request(&request)
            .map_err(|_| MentatError::Storage)?;
        live.next_id += 1;
        live.waiting.push(request.clone());
        self.advance_mentat(r, room, now);
        self.apply_mentat(r, room);
        let opened = Self::state_of(r)
            .and_then(|s| {
                s.current
                    .into_iter()
                    .chain(s.waiting)
                    .find(|q| q.id == request.id)
            })
            .unwrap_or(request);
        Ok(opened)
    }

    /// `asker` joins waiting request `id`, to go in with its askers.
    pub fn join_mentat_request(
        &self,
        room: &str,
        asker: &str,
        id: u64,
    ) -> Result<MentatRequest, MentatError> {
        let mut rooms = self.rooms.lock_or_recover();
        let r = rooms.get_mut(room).ok_or(MentatError::NotMentatRoom)?;
        let live = r.mentat.as_mut().ok_or(MentatError::NotMentatRoom)?;
        if live.is_mentat(asker) {
            return Err(MentatError::IsMentat);
        }
        if live.current.as_ref().is_some_and(|c| c.id == id) {
            return Err(MentatError::NotWaiting);
        }
        if live.holds_request(asker) {
            return Err(MentatError::AlreadyInLine);
        }
        let request = live
            .waiting
            .iter_mut()
            .find(|q| q.id == id)
            .ok_or(MentatError::NoSuchRequest)?;
        let mut joined = request.clone();
        joined.owners.push(asker.to_string());
        self.store
            .save_mentat_request(&joined)
            .map_err(|_| MentatError::Storage)?;
        *request = joined.clone();
        self.apply_mentat(r, room);
        Ok(joined)
    }

    /// `asker` takes its place in waiting request `id` back. The last asker
    /// to leave withdraws the request.
    pub fn withdraw_mentat_request(
        &self,
        room: &str,
        asker: &str,
        id: u64,
    ) -> Result<MentatRequest, MentatError> {
        let now = (self.now_ms)();
        let mut rooms = self.rooms.lock_or_recover();
        let r = rooms.get_mut(room).ok_or(MentatError::NotMentatRoom)?;
        let live = r.mentat.as_mut().ok_or(MentatError::NotMentatRoom)?;
        if live.current.as_ref().is_some_and(|c| c.id == id) {
            return Err(MentatError::NotWaiting);
        }
        let at = live
            .waiting
            .iter()
            .position(|q| q.id == id)
            .ok_or(MentatError::NoSuchRequest)?;
        if !live.waiting[at].owners.iter().any(|o| o == asker) {
            return Err(MentatError::NotYours);
        }
        let mut request = live.waiting[at].clone();
        request.owners.retain(|o| o != asker);
        if request.owners.is_empty() {
            request.owners.push(asker.to_string());
            request.status = MentatRequestStatus::Withdrawn;
            request.ended_at = Some(now);
            request.ended_by = Some(asker.to_string());
        }
        self.store
            .save_mentat_request(&request)
            .map_err(|_| MentatError::Storage)?;
        if request.status == MentatRequestStatus::Withdrawn {
            live.waiting.remove(at);
        } else {
            live.waiting[at] = request.clone();
        }
        self.apply_mentat(r, room);
        Ok(request)
    }

    /// Join waiting request `from` to waiting request `into`: its askers go
    /// in with the other's. The mentat does this, or a rank who manages the
    /// room (`rank`).
    pub fn merge_mentat_requests(
        &self,
        room: &str,
        from: u64,
        into: u64,
        by: &str,
        rank: bool,
    ) -> Result<MentatRequest, MentatError> {
        if from == into {
            return Err(MentatError::SameRequest);
        }
        let now = (self.now_ms)();
        let mut rooms = self.rooms.lock_or_recover();
        let r = rooms.get_mut(room).ok_or(MentatError::NotMentatRoom)?;
        let live = r.mentat.as_mut().ok_or(MentatError::NotMentatRoom)?;
        if !rank && !live.is_mentat(by) {
            return Err(MentatError::NotYours);
        }
        if live
            .current
            .as_ref()
            .is_some_and(|c| c.id == from || c.id == into)
        {
            return Err(MentatError::NotWaiting);
        }
        let from_at = live
            .waiting
            .iter()
            .position(|q| q.id == from)
            .ok_or(MentatError::NoSuchRequest)?;
        let into_at = live
            .waiting
            .iter()
            .position(|q| q.id == into)
            .ok_or(MentatError::NoSuchRequest)?;
        let mut gone = live.waiting[from_at].clone();
        let mut kept = live.waiting[into_at].clone();
        for owner in &gone.owners {
            if !kept.owners.contains(owner) {
                kept.owners.push(owner.clone());
            }
        }
        gone.status = MentatRequestStatus::Ended;
        gone.ended_at = Some(now);
        gone.ended_by = Some(by.to_string());
        gone.end_reason = Some(MentatEndReason::Merged);
        self.store
            .save_mentat_request(&kept)
            .map_err(|_| MentatError::Storage)?;
        self.store
            .save_mentat_request(&gone)
            .map_err(|_| MentatError::Storage)?;
        live.waiting[into_at] = kept.clone();
        live.waiting.remove(from_at);
        let first = gone.owners.first().cloned();
        self.announce_line(
            r,
            room,
            first.as_deref(),
            format!(
                "Request #{} was joined to request #{}: its askers go in together.",
                gone.id, kept.id
            ),
        );
        self.apply_mentat(r, room);
        Ok(kept)
    }

    /// End the consultation inside. `rank` says the caller manages the room;
    /// otherwise only its askers and the mentat may end it. An asker may say
    /// whether it was answered.
    pub fn end_mentat_consultation(
        &self,
        room: &str,
        by: &str,
        rank: bool,
        answered: Option<MentatAnswered>,
    ) -> Result<MentatRequest, MentatError> {
        let now = (self.now_ms)();
        let mut rooms = self.rooms.lock_or_recover();
        let r = rooms.get_mut(room).ok_or(MentatError::NotMentatRoom)?;
        let live = r.mentat.as_mut().ok_or(MentatError::NotMentatRoom)?;
        let mentat = live.is_mentat(by);
        let current = live.current.as_mut().ok_or(MentatError::NoConsultation)?;
        let reason = if mentat {
            MentatEndReason::Mentat
        } else if current.owners.iter().any(|o| o == by) {
            current.answered = answered;
            MentatEndReason::Asker
        } else if rank {
            MentatEndReason::Rank
        } else {
            return Err(MentatError::NotYours);
        };
        let ended = self.finish_current(r, room, MentatRequestStatus::Ended, reason, Some(by), now);
        self.advance_mentat(r, room, now);
        self.apply_mentat(r, room);
        ended.ok_or(MentatError::NoConsultation)
    }

    /// `name` says it is replying, or stopped (`Hub::typing`). While the
    /// mentat or an asker inside is replying, silence is not counted: a
    /// model that thinks for three minutes has not gone silent.
    pub(in crate::hub) fn mentat_typing(&self, r: &mut Room, name: &str, on: bool) {
        let now = (self.now_ms)();
        let Some(live) = r.mentat.as_mut() else {
            return;
        };
        live.replying.retain(|n| n != name);
        if on {
            live.replying.push(name.to_string());
        }
        let inside = live.is_mentat(name)
            || live
                .current
                .as_ref()
                .is_some_and(|c| c.owners.iter().any(|o| o == name));
        if let Some(current) = live.current.as_mut() {
            if inside && current.status == MentatRequestStatus::Active {
                // Silence is counted from when the reply stopped.
                current.last_activity = Some(now);
            }
        }
    }

    /// After an accepted message in a mentat room. A called request starts
    /// with its askers' first message. What the askers write before the
    /// mentat answers is one question. Once time or questions are used up
    /// the consultation ends with the mentat's next message.
    pub(in crate::hub) fn mentat_after_post(&self, r: &mut Room, room: &str, msg: &Message) {
        let Some(live) = r.mentat.as_mut() else {
            return;
        };
        let mentat = live.is_mentat(&msg.sender);
        let questions = live.room.limits.questions;
        live.replying.retain(|n| n != &msg.sender);
        let Some(current) = live.current.as_mut() else {
            return;
        };
        let now = msg.ts;
        if current.owners.iter().any(|o| o == &msg.sender) {
            if current.closing {
                return;
            }
            if current.status == MentatRequestStatus::Called {
                current.status = MentatRequestStatus::Active;
                current.started_at = Some(now);
            }
            if !current.awaiting {
                current.questions += 1;
                current.awaiting = true;
            }
            current.last_activity = Some(now);
            let saved = current.clone();
            self.save_request(room, &saved);
            self.apply_mentat(r, room);
        } else if mentat {
            if current.status != MentatRequestStatus::Active {
                return;
            }
            current.answers += 1;
            current.awaiting = false;
            current.last_activity = Some(now);
            if current.closing {
                let reason = current.end_reason.unwrap_or(MentatEndReason::Questions);
                self.finish_current(r, room, MentatRequestStatus::Ended, reason, None, now);
                self.advance_mentat(r, room, now);
            } else if current.questions >= questions {
                // That was the answer to the last question.
                self.finish_current(
                    r,
                    room,
                    MentatRequestStatus::Ended,
                    MentatEndReason::Questions,
                    None,
                    now,
                );
                self.advance_mentat(r, room, now);
            } else {
                let saved = current.clone();
                self.save_request(room, &saved);
            }
            self.apply_mentat(r, room);
        }
    }

    /// The 15-second sweep: skip a called request nobody came to, close a
    /// consultation past its time or silent too long, drop requests that
    /// waited too long, and call the next. A paused room does not move, and
    /// neither does one that waits for a mentat.
    pub fn expire_mentat(&self) {
        let now = (self.now_ms)();
        let mut rooms = self.rooms.lock_or_recover();
        let names: Vec<String> = rooms
            .iter()
            .filter(|(_, r)| r.mentat.is_some() && !matches!(r.mode, ChatMode::Paused))
            .map(|(name, _)| name.clone())
            .collect();
        for room in names {
            let Some(r) = rooms.get_mut(&room) else {
                continue;
            };
            let Some(live) = r.mentat.as_mut() else {
                continue;
            };
            let limits = live.room.limits;
            let mut changed = false;
            let (stale, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut live.waiting)
                .into_iter()
                .partition(|q| after(Some(q.opened_at), limits.wait_secs, now));
            live.waiting = kept;
            for mut request in stale {
                request.status = MentatRequestStatus::Expired;
                request.ended_at = Some(now);
                self.save_request(&room, &request);
                let text = format!(
                    "Request #{} left the mentat line after waiting {} minutes; open a new one to \
                     ask.",
                    request.id,
                    limits.wait_secs / 60
                );
                let first = request.owners.first().cloned();
                self.announce_line(r, &room, first.as_deref(), text);
                changed = true;
            }
            changed |= self.expire_current(r, &room, limits, now);
            changed |= self.advance_mentat(r, &room, now);
            if changed {
                self.apply_mentat(r, &room);
            }
        }
    }

    /// The sweep's look at the consultation inside. Returns whether it moved.
    fn expire_current(&self, r: &mut Room, room: &str, limits: MentatLimits, now: u64) -> bool {
        enum Step {
            End(MentatRequestStatus, MentatEndReason),
            Close(MentatEndReason),
        }
        let Some(live) = r.mentat.as_mut() else {
            return false;
        };
        let replying = live.someone_replying();
        let Some(c) = live.current.as_mut() else {
            return false;
        };
        let step = match c.status {
            MentatRequestStatus::Called if after(c.called_at, limits.call_secs, now) => {
                Step::End(MentatRequestStatus::Skipped, MentatEndReason::Idle)
            }
            MentatRequestStatus::Active if c.closing => {
                if after(c.closing_at, limits.close_secs, now) {
                    let reason = c.end_reason.unwrap_or(MentatEndReason::Time);
                    Step::End(MentatRequestStatus::Ended, reason)
                } else {
                    return false;
                }
            }
            MentatRequestStatus::Active if after(c.started_at, limits.session_secs, now) => {
                if c.awaiting {
                    // The mentat owes an answer: it gets to give it.
                    Step::Close(MentatEndReason::Time)
                } else {
                    Step::End(MentatRequestStatus::Ended, MentatEndReason::Time)
                }
            }
            MentatRequestStatus::Active
                if !replying && after(c.last_activity, limits.idle_secs, now) =>
            {
                Step::End(MentatRequestStatus::Ended, MentatEndReason::Idle)
            }
            _ => return false,
        };
        match step {
            Step::Close(reason) => {
                c.closing = true;
                c.closing_at = Some(now);
                c.end_reason = Some(reason);
                let saved = c.clone();
                self.save_request(room, &saved);
                let text = format!(
                    "Consultation #{}: {}. The mentat may give its last answer within {} seconds.",
                    saved.id,
                    reason_words(reason),
                    limits.close_secs
                );
                self.announce_line(r, room, None, text);
            }
            Step::End(status, reason) => {
                self.finish_current(r, room, status, reason, None, now);
            }
        }
        true
    }

    /// Close the consultation inside with `status` and say so. Returns it.
    /// Two consultations in a row with no word from the mentat pause the
    /// line: a mentat that does not answer must not use up every turn.
    fn finish_current(
        &self,
        r: &mut Room,
        room: &str,
        status: MentatRequestStatus,
        reason: MentatEndReason,
        by: Option<&str>,
        now: u64,
    ) -> Option<MentatRequest> {
        let live = r.mentat.as_mut()?;
        let mut request = live.current.take()?;
        let limits = live.room.limits;
        let was_active = request.status == MentatRequestStatus::Active;
        request.status = status;
        request.ended_at = Some(now);
        request.ended_by = by.map(str::to_string);
        request.end_reason = Some(reason);
        request.closing = false;
        request.awaiting = false;
        live.replying.clear();
        let unanswered = was_active
            && request.answers == 0
            && matches!(reason, MentatEndReason::Idle | MentatEndReason::Time);
        live.silent = if unanswered { live.silent + 1 } else { 0 };
        let pause = live.silent >= MENTAT_SILENT_PAUSE;
        if pause {
            live.silent = 0;
        }
        self.save_request(room, &request);
        let text = if status == MentatRequestStatus::Skipped {
            format!(
                "Request #{} was skipped: nobody wrote within {} seconds of being called.",
                request.id, limits.call_secs
            )
        } else {
            format!(
                "Consultation #{} ended: {}.",
                request.id,
                reason_words(reason)
            )
        };
        let first = request.owners.first().cloned();
        self.announce_line(r, room, first.as_deref(), text);
        if pause {
            self.put_mode(r, room, ChatMode::Paused);
            self.announce_line(
                r,
                room,
                None,
                format!(
                    "The line is paused: the mentat did not answer {MENTAT_SILENT_PAUSE} \
                     consultations in a row. The mentat, the Master or the Smaster who founded \
                     this room resumes it."
                ),
            );
        }
        Some(request)
    }

    /// Call the next request in when nobody is inside, the room has a
    /// mentat and is not paused. Returns whether one was called.
    fn advance_mentat(&self, r: &mut Room, room: &str, now: u64) -> bool {
        if matches!(r.mode, ChatMode::Paused) {
            return false;
        }
        let Some(live) = r.mentat.as_mut() else {
            return false;
        };
        let Some(mentat) = live.room.mentat.clone() else {
            return false;
        };
        if live.current.is_some() || live.waiting.is_empty() {
            return false;
        }
        let mut next = live.waiting.remove(0);
        next.status = MentatRequestStatus::Called;
        next.called_at = Some(now);
        let limits = live.room.limits;
        live.current = Some(next.clone());
        self.save_request(room, &next);
        let askers = next
            .owners
            .iter()
            .map(|o| format!("@{o}"))
            .collect::<Vec<_>>()
            .join(" ");
        let text = format!(
            "{askers}: your turn with the mentat, {mentat} (request #{}). Write your question \
             here within {} seconds. Limits: {} minutes, {} questions.",
            next.id,
            limits.call_secs,
            limits.session_secs / 60,
            limits.questions
        );
        let first = next.owners.first().cloned();
        self.announce_line(r, room, first.as_deref(), text);
        true
    }

    /// Bring the room's mode in line with the line, and tell the browsers.
    pub(in crate::hub) fn apply_mentat(&self, r: &mut Room, room: &str) {
        let Some(live) = r.mentat.as_ref() else {
            return;
        };
        if !matches!(r.mode, ChatMode::Paused) {
            let mode = ChatMode::Restricted {
                allow: live.allow(),
            };
            if r.mode != mode {
                self.put_mode(r, room, mode);
            }
        }
        if let Some(state) = Self::state_of(r) {
            let _ = r.tx.send(ServerFrame::Mentat { state });
        }
    }

    /// The mode a mode change asks for, as a mentat room takes it: `Paused`
    /// freezes the line; anything else resumes it, with the clocks of the
    /// consultation inside restarted so the pause does not count against it.
    pub(in crate::hub) fn mentat_mode_request(r: &mut Room, asked: ChatMode, now: u64) -> ChatMode {
        let Some(live) = r.mentat.as_mut() else {
            return asked;
        };
        if matches!(asked, ChatMode::Paused) {
            return ChatMode::Paused;
        }
        if matches!(r.mode, ChatMode::Paused) {
            live.silent = 0;
            if let Some(current) = live.current.as_mut() {
                match current.status {
                    MentatRequestStatus::Called => current.called_at = Some(now),
                    _ if current.closing => current.closing_at = Some(now),
                    _ => current.last_activity = Some(now),
                }
            }
        }
        ChatMode::Restricted {
            allow: live.allow(),
        }
    }

    /// After a mode change in a mentat room: persist restarted clocks, call
    /// the next request if the room resumed, and tell the browsers.
    pub(in crate::hub) fn mentat_after_mode(&self, r: &mut Room, room: &str) {
        let Some(current) = r.mentat.as_ref().and_then(|live| live.current.clone()) else {
            if r.mentat.is_some() {
                let now = (self.now_ms)();
                self.advance_mentat(r, room, now);
                self.apply_mentat(r, room);
            }
            return;
        };
        self.save_request(room, &current);
        self.apply_mentat(r, room);
    }

    /// Keep the settings' mirror of the mentat room (`RoomSettings::mentat`,
    /// `mentat_room`) in step, and tell every reader of the settings.
    fn mirror_mentat(&self, r: &mut Room, room: &str) {
        let mentat = r.mentat.as_ref().and_then(|live| live.room.mentat.clone());
        let is_room = r.mentat.is_some();
        if r.settings.mentat == mentat && r.settings.mentat_room == is_room {
            return;
        }
        r.settings.mentat = mentat;
        r.settings.mentat_room = is_room;
        if let Err(error) = self.store.save_room(room, &r.mode, &r.settings) {
            tracing::error!(room, %error, "could not persist the mentat room's settings");
        }
        let settings = self.shown_settings(r.settings.clone());
        let _ = r.tx.send(ServerFrame::Settings { settings });
    }

    fn put_mode(&self, r: &mut Room, room: &str, mode: ChatMode) {
        if let Err(error) = self.store.save_room(room, &mode, &r.settings) {
            tracing::error!(room, %error, "could not persist the mentat room's mode");
        }
        r.mode = mode.clone();
        let _ = r.tx.send(ServerFrame::Mode { mode });
    }

    /// The request change has been decided; its record must not hold the
    /// line up. A failed write is logged, and the line moves on.
    fn save_request(&self, room: &str, request: &MentatRequest) {
        if let Err(error) = self.store.save_mentat_request(request) {
            tracing::error!(room, request = request.id, %error, "could not persist a mentat request");
        }
    }

    /// A line from the server itself, shown as an announcement and addressed
    /// to `target` (`None` wakes nobody). The change it reports has already
    /// been decided; a failed write is logged and the line is still shown.
    pub(in crate::hub) fn announce_line(
        &self,
        r: &mut Room,
        room: &str,
        target: Option<&str>,
        text: String,
    ) {
        let msg = Message {
            attachments: Vec::new(),
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            room: room.to_string(),
            sender: "loca".into(),
            sender_type: protocol::SenderType::User,
            target: target.map(str::to_string),
            text,
            reply_to: None,
            reply_to_sender: None,
            sender_principal: None,
            kind: protocol::MessageKind::Announce,
            ts: (self.now_ms)(),
        };
        if let Err(error) = self.store.insert_message(&msg, None) {
            tracing::error!(%error, room, "could not persist a server line");
        }
        r.history.push(msg.clone());
        if r.history.len() > HISTORY_LIMIT {
            let overflow = r.history.len() - HISTORY_LIMIT;
            r.history.drain(0..overflow);
        }
        let _ = r.tx.send(ServerFrame::Msg { message: msg });
    }
}

impl Hub {
    /// At boot: put each mentat room's line back on its loca. A sealed loca
    /// is not live and keeps its record untouched.
    pub(in crate::hub) fn load_mentat_rooms(&self) {
        let records = self
            .store
            .mentat_rooms()
            .expect("persistent mentat rooms are unreadable; refusing to start");
        let mut rooms = self.rooms.lock_or_recover();
        // The mirror follows the records: a loca with none is no mentat room.
        for r in rooms.values_mut() {
            r.settings.mentat = None;
            r.settings.mentat_room = false;
        }
        for record in records {
            let Some(r) = rooms.get_mut(&record.room) else {
                continue;
            };
            let open = self
                .store
                .open_mentat_requests(&record.room)
                .expect("persistent mentat requests are unreadable; refusing to start");
            let last_id = self
                .store
                .last_mentat_request_id(&record.room)
                .expect("persistent mentat requests are unreadable; refusing to start");
            r.settings.mentat = record.mentat.clone();
            r.settings.mentat_room = true;
            r.mentat = Some(MentatLive::new(record, open, last_id));
        }
    }
}
