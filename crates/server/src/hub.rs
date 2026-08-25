//! In-memory room state: message history, live broadcast, and member tracking.
//!
//! One [`Hub`] holds every room. Each [`Room`] keeps its backlog and a
//! `tokio::sync::broadcast` channel that every connected client subscribes to.
//! Human membership is reference-counted so the same person may read from two
//! web clients without vanishing from the roster when one closes. Agent
//! runtimes remain single-writer and replace a stale predecessor on reconnect.

mod attention;
mod work;

pub use work::{GoalError, TaskError, WaitError};

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;

use protocol::{
    Attention, AttentionAudience, AttentionStatus, CareReason, CareSignal, ChatMode, CreateGoal,
    CreateNote, CreateTask, Goal, GoalCompletion, GoalStatus, Invite, Member, Message, Note,
    PostMessage, ReminderRecipient, RoomSettings, RoomSummary, SenderType, ServerFrame, SetWait,
    Task, TaskStatus, UpdateGoal, UpdateNote, UpdateTask, WaitState,
};

#[derive(Debug, Clone)]
struct RuntimeRecord {
    wake: String,
    ack: String,
    delivery_id: Option<String>,
    attention_id: Option<String>,
    stored: bool,
    accepted: bool,
    first_response: bool,
    final_response: bool,
    turn_completed: bool,
    seen_at: u64,
    progress_at: u64,
}

use crate::store::{
    BuildingRole, CredentialError, CredentialSummary, LocaOperatorAssignment, LocaOperatorError,
    PrincipalIdentity, Store,
};
use crate::sync::RecoverMutex;

/// Recent messages kept per room and replayed to new joiners.
const HISTORY_LIMIT: usize = 200;
/// Broadcast backlog; slow clients that lag past this get a `Lagged` and
/// resync from history on their next reconnect.
const BROADCAST_CAP: usize = 256;

#[derive(Clone, Copy)]
struct CareMark {
    last_signal_at: u64,
    signal_count: u32,
}

struct CareDraft {
    attention_key: String,
    owner: Option<String>,
    reason: CareReason,
    target: Option<String>,
    participants: Vec<String>,
    subject: String,
    attempt: u32,
    at: u64,
    escalated: bool,
}

struct Room {
    history: Vec<Message>,
    /// Recently accepted operation ids, including memory-only development
    /// servers. SQLite is the durable authority in production; this bounded
    /// mirror closes the same-process race/retry gap when no DB is configured.
    operations: HashMap<(String, String), Message>,
    operation_order: VecDeque<(String, String)>,
    tx: broadcast::Sender<ServerFrame>,
    /// seat identity -> (display name, kind, live connection count).
    /// One key = one seat: the map is keyed by IDENTITY (derived from the
    /// credential at the door — "@master", "mb:…", "sm:…", "st:…", "name:…"),
    /// never by the free-text display name. The same key reconnecting under a
    /// new name takes over its old seat; it cannot sit twice. Two human web
    /// clients under the same identity+name share that one seat by reference
    /// count. The name is just the label the seat currently wears.
    members: HashMap<String, (String, SenderType, usize)>,
    /// Living notes, keyed by `Note::key`.
    notes: HashMap<String, Note>,
    /// What has already been done here, oldest first. Append-only: entries are
    /// pushed and read, never edited or removed.
    journal: Vec<protocol::JournalEntry>,
    next_journal_id: u64,
    /// Per-note monotonic revision counter source.
    next_rev: u64,
    /// How chat is currently gated (admin-controlled, server-enforced).
    mode: ChatMode,
    /// Admin-tunable per-room settings (rate limit, …).
    settings: RoomSettings,
    /// Recent post timestamps per sender, for the sliding-window rate limit.
    post_times: HashMap<String, VecDeque<u64>>,
    /// Names frozen from posting (still connected, still see messages).
    muted: std::collections::HashSet<String>,
    /// Names blocked from joining/posting entirely.
    banned: std::collections::HashSet<String>,
    /// Unix ms of the last accepted message; drives the live idle timeout.
    last_msg_ms: u64,
    /// Declared work (the guest object), keyed by task id.
    tasks: HashMap<u64, Task>,
    next_task_id: u64,
    /// Goal history. At most one entry may be active.
    goals: HashMap<u64, Goal>,
    next_goal_id: u64,
    /// Explicit participant dependency edges, one active edge per waiter.
    waits: HashMap<String, WaitState>,
    /// Bounded reminder counters for goal/task/silence signals.
    care_marks: HashMap<String, CareMark>,
    /// Durable attention lifecycle; delivery ACK is not completion.
    attentions: HashMap<String, Attention>,
}

impl Room {
    fn with(mode: ChatMode, settings: RoomSettings, next_rev: u64) -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAP);
        Room {
            history: Vec::new(),
            operations: HashMap::new(),
            operation_order: VecDeque::new(),
            tx,
            members: HashMap::new(),
            notes: HashMap::new(),
            journal: Vec::new(),
            next_journal_id: 1,
            next_rev,
            mode,
            settings,
            post_times: HashMap::new(),
            muted: Default::default(),
            banned: Default::default(),
            last_msg_ms: 0,
            tasks: HashMap::new(),
            next_task_id: 1,
            goals: HashMap::new(),
            next_goal_id: 1,
            waits: HashMap::new(),
            care_marks: HashMap::new(),
            attentions: HashMap::new(),
        }
    }

    fn member_list(&self) -> Vec<Member> {
        let mut list: Vec<Member> = self
            .members
            .values()
            .map(|(name, kind, _)| Member {
                name: name.clone(),
                kind: *kind,
            })
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }
}

/// The whole server's room state. Cheap to clone (`Arc` inside).
#[derive(Clone)]
pub struct Hub {
    rooms: Arc<Mutex<HashMap<String, Room>>>,
    /// Locas the master has deleted. A create-on-access hub would otherwise let
    /// the next touch — a `watch=1` listener's subscribe, a stray join — quietly
    /// re-create a room the operator just removed. A tombstone here means "this
    /// name is deleted, do not resurrect it"; it is cleared when the master
    /// explicitly opens the loca again.
    deleted: Arc<Mutex<std::collections::HashSet<String>>>,
    next_id: Arc<AtomicU64>,
    /// Monotonic product-state generation. Wall clocks can repeat or move
    /// backwards; Attention identities must still advance after real progress.
    condition_generation: Arc<AtomicU64>,
    now_ms: fn() -> u64,
    /// Shared secret gating admin actions (mode changes). Empty = admin actions
    /// are open (dev default with a loud warning at startup).
    admin_token: Arc<String>,
    /// Shared secret required to connect / post at all. Empty = open (dev).
    /// The admin token also satisfies this (admins are members).
    room_token: Arc<String>,
    /// Write-through persistence (memory-only when not configured).
    store: Arc<Store>,
    /// Default settings seeded into newly created rooms.
    default_settings: RoomSettings,
    /// Server boot id: changes every process start so clients can detect a
    /// restart (and the state reset that comes with memory-only mode).
    epoch: u64,
    /// Live membership keys -> who belongs to the building. Separate from
    /// invites: a davet seats a member in a loca, membership is the identity
    /// itself and survives every room they leave.
    members: Arc<Mutex<HashMap<String, protocol::Membership>>>,
    /// Live legacy Smaster credentials -> the principal holding them. A Smaster is a second
    /// master: same powers, but the master has the last word (see `is_master`).
    smasters: Arc<Mutex<HashMap<String, String>>>,
    /// Issued session tokens -> bound identity. Davet sessions are ephemeral;
    /// browser admin sessions survive a deploy until their normal expiry.
    sessions: Arc<Mutex<HashMap<String, SessionIdentity>>>,
    /// Current one-use browser pairing code and the lifetime chosen for the
    /// admin session it will mint. The root key stays in the server's
    /// environment; the browser exchanges this stand-in.
    admin_pairing: Arc<Mutex<AdminPairing>>,
    // Per-source (peer IP) recent /join-requests create timestamps (ms) for a
    // sliding-window rate-limit — a single abusive source is throttled without
    // affecting anyone else (review re-blocker #3).
    join_create_times: Arc<Mutex<std::collections::HashMap<std::net::IpAddr, Vec<u64>>>>,
    /// When true, posting requires a valid session token: `sender` can no
    /// longer be spoofed via the request body (PRODUCTION.md Aşama 0).
    require_sessions: bool,
    /// When true, an empty room_token means 'davet-only', not 'open house'.
    /// This is how the building key is retired: no key, but the door still
    /// demands a davet for every loca.
    require_invite: bool,
    /// Live davetler, keyed by token. Unlike sessions these are persisted:
    /// a restart must not un-invite anyone.
    invites: Arc<Mutex<HashMap<String, Invite>>>,
    /// Building-lobby signal bus. Lobby sockets never join a loca; they wait
    /// here for a davet addressed to their permanent membership.
    lobby_tx: broadcast::Sender<LobbyEvent>,
    /// Live lobby sockets per membership token. A count (rather than a set)
    /// keeps presence correct while one identity is reconnecting.
    lobby_online: Arc<Mutex<HashMap<String, usize>>>,
    /// Adapter health is ephemeral and deliberately separate from transport
    /// presence. Heartbeats never become membership or room history.
    runtime_health: Arc<Mutex<HashMap<String, RuntimeRecord>>>,
    /// Hot delivery-attempt → stable attention identity map. SQLite is the
    /// restart authority; this mirror keeps memory-only/dev behavior exact.
    care_deliveries: Arc<Mutex<HashMap<String, String>>>,
    /// The building's private governance/caretaker loca (production: `iye`).
    home_room: Arc<String>,
    /// When set, only configured caretakers may receive a davet for this loca;
    /// master and smaster enter by rank and need no davet.
    reserved_room: Arc<String>,
    caretakers: Arc<std::collections::HashSet<String>>,
}

#[derive(Clone)]
struct AdminPairing {
    code: String,
    session_ttl_ms: u64,
    expires_at_ms: u64,
}

/// A private signal delivered only to the lobby socket carrying `member`.
/// The davet remains persisted in `invites`; this bus is merely the immediate
/// wake-up path, and a reconnect receives the persisted snapshot.
#[derive(Debug, Clone)]
pub enum LobbyEvent {
    Called {
        member: String,
        room: String,
        token: String,
    },
    MembershipRevoked {
        member: String,
    },
}

impl LobbyEvent {
    pub fn member(&self) -> &str {
        match self {
            Self::Called { member, .. } | Self::MembershipRevoked { member } => member,
        }
    }
}

/// The outcome of the one door decision (see [`Hub::enter_decision`]). Kept
/// distinct from a bare bool so a ban can carry its own 403, separate from a
/// plain "not allowed" 401.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnterDecision {
    Allowed,
    Denied,
    Banned,
}

impl EnterDecision {
    pub fn is_allowed(self) -> bool {
        matches!(self, EnterDecision::Allowed)
    }
}

/// Why a davet could not be issued (see [`Hub::invite_member_to_room`]).
/// A davet never creates identity — it seats an existing member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InviteError {
    /// The name is not a building member — admit them first.
    MemberNotFound,
    /// This member already holds a live davet for this loca.
    AlreadyInvited,
    /// The loca is full — LOCA_KAPASITE seats are already handed out.
    Full,
    /// The davet could not be persisted.
    Storage,
    /// This is the building's private governance/caretaker loca.
    Reserved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeadError {
    ActiveGoal,
    Storage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatorAssignmentError {
    AuthorityRequired,
    EmptySeatRequired,
    MasterProtected,
    PrincipalNotFound,
    PrincipalMustBeHuman,
    NotFound,
    Conflict,
    Storage,
}

pub struct HubConfig {
    pub admin_token: String,
    pub room_token: String,
    pub require_sessions: bool,
    pub require_invite: bool,
    pub home_room: String,
    pub reserved_room: String,
    pub caretakers: std::collections::HashSet<String>,
}

/// The identity a session token resolves to.
#[derive(Clone)]
pub struct SessionIdentity {
    /// Stable identity. Legacy/memory-only sessions may temporarily lack one;
    /// authorization never derives it from the request body's display name.
    pub principal_id: Option<String>,
    pub building_role: BuildingRole,
    pub name: String,
    pub kind: SenderType,
    /// The loca this session is confined to, when it was taken with a davet.
    /// `None` = taken with the building key, so it reaches as far as that key
    /// does. Without this a davet holder could take a session and walk the
    /// whole building — the davet would mean nothing.
    pub loca: Option<String>,
    /// The building membership (mb_ token) this session proves, when it was
    /// taken with a davet. The seat key derives from this: one member, one
    /// seat, whatever display name the connection claims.
    pub member: Option<String>,
    /// Does this bounded session authenticate the Master principal? It lets
    /// the browser prove Master authority without retaining or repeatedly
    /// transmitting the root/bootstrap/recovery credential. It expires so a
    /// leaked session dies on its own.
    pub admin: bool,
    /// Unix-ms after which this session is no longer valid. `0` = never expires
    /// (davet/building sessions keep their old behaviour). Admin sessions get a
    /// finite TTL so the key's stand-in cannot outlive the sitting.
    pub expires_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestAuthority {
    pub principal_id: Option<String>,
    pub display_name: Option<String>,
    pub kind: Option<SenderType>,
    pub building_role: Option<BuildingRole>,
}

impl RequestAuthority {
    pub fn is_building_admin(&self) -> bool {
        matches!(
            self.building_role,
            Some(BuildingRole::Master | BuildingRole::Smaster)
        )
    }

    pub fn is_master(&self) -> bool {
        self.building_role == Some(BuildingRole::Master)
    }
}

fn authority_from_principal(principal: Option<PrincipalIdentity>) -> Option<RequestAuthority> {
    principal.map(|identity| RequestAuthority {
        principal_id: Some(identity.id),
        display_name: Some(identity.display_name),
        kind: Some(identity.kind),
        building_role: Some(identity.role),
    })
}

fn map_loca_operator_error(error: LocaOperatorError) -> OperatorAssignmentError {
    match error {
        LocaOperatorError::PrincipalNotFound | LocaOperatorError::AppointerNotFound => {
            OperatorAssignmentError::PrincipalNotFound
        }
        LocaOperatorError::PrincipalMustBeHuman => OperatorAssignmentError::PrincipalMustBeHuman,
        LocaOperatorError::AppointerNotAuthorized => OperatorAssignmentError::AuthorityRequired,
        LocaOperatorError::EmptySeatRequired => OperatorAssignmentError::EmptySeatRequired,
        LocaOperatorError::NotFound => OperatorAssignmentError::NotFound,
        LocaOperatorError::Conflict => OperatorAssignmentError::Conflict,
        LocaOperatorError::Storage => OperatorAssignmentError::Storage,
    }
}

#[derive(Debug, Clone)]
pub struct AdminSessionGrant {
    ttl_ms: u64,
    authority: RequestAuthority,
    credential_id: Option<String>,
}

impl Hub {
    /// Full constructor: tokens, a store, default room settings, and a boot
    /// epoch. Loads any persisted rooms from the store on boot.
    pub fn build(
        config: HubConfig,
        store: Arc<Store>,
        default_settings: RoomSettings,
        epoch: u64,
    ) -> Self {
        let HubConfig {
            admin_token,
            room_token,
            require_sessions,
            require_invite,
            home_room,
            reserved_room,
            caretakers,
        } = config;
        let master_name = std::env::var("LOCA_MASTER_NAME")
            .ok()
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "Master".into());
        store
            .ensure_master_principal(&admin_token, &master_name, default_now_ms())
            .expect("bootstrap Master principal");
        let (lobby_tx, _) = broadcast::channel(BROADCAST_CAP);
        let pairing_now = default_now_ms();
        let hub = Hub {
            rooms: Arc::new(Mutex::new(HashMap::new())),
            deleted: Arc::new(Mutex::new(std::collections::HashSet::new())),
            next_id: Arc::new(AtomicU64::new(1)),
            condition_generation: Arc::new(AtomicU64::new(0)),
            now_ms: default_now_ms,
            admin_token: Arc::new(admin_token),
            room_token: Arc::new(room_token),
            store,
            default_settings,
            epoch,
            members: Arc::new(Mutex::new(HashMap::new())),
            smasters: Arc::new(Mutex::new(HashMap::new())),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            admin_pairing: Arc::new(Mutex::new(AdminPairing {
                code: Self::secure_token("pair_", 16),
                session_ttl_ms: Self::ADMIN_SESSION_TTL_MS,
                expires_at_ms: pairing_now.saturating_add(Self::ADMIN_PAIRING_TTL_MS),
            })),
            join_create_times: Arc::new(Mutex::new(std::collections::HashMap::new())),
            require_sessions,
            require_invite,
            invites: Arc::new(Mutex::new(HashMap::new())),
            lobby_tx,
            lobby_online: Arc::new(Mutex::new(HashMap::new())),
            runtime_health: Arc::new(Mutex::new(HashMap::new())),
            care_deliveries: Arc::new(Mutex::new(HashMap::new())),
            home_room: Arc::new(home_room),
            reserved_room: Arc::new(reserved_room),
            caretakers: Arc::new(caretakers),
        };

        // Davetler survive restarts — load them before anyone knocks.
        {
            let mut inv = hub.invites.lock_or_recover();
            for i in hub.store.load_invites() {
                inv.insert(i.token.clone(), i);
            }
        }

        // Restore persisted state.
        let mut max_id = 0u64;
        let mut max_generation = 0u64;
        {
            let mut rooms = hub.rooms.lock_or_recover();
            for snap in hub.store.load() {
                let mut room = Room::with(snap.mode, snap.settings, snap.max_rev + 1);
                room.history = snap.messages;
                max_generation =
                    max_generation.max(room.history.last().map(|message| message.ts).unwrap_or(0));
                for n in snap.notes {
                    room.notes.insert(n.key.clone(), n);
                }
                for t in hub.store.load_tasks(&snap.room) {
                    max_generation = max_generation.max(t.progress_at);
                    room.next_task_id = room.next_task_id.max(t.id + 1);
                    room.tasks.insert(t.id, t);
                }
                for goal in hub.store.load_goals(&snap.room) {
                    max_generation = max_generation.max(goal.progress_at);
                    room.next_goal_id = room.next_goal_id.max(goal.id + 1);
                    room.goals.insert(goal.id, goal);
                }
                for wait in hub.store.load_waits(&snap.room) {
                    max_generation = max_generation.max(wait.since);
                    room.waits.insert(wait.waiter.clone(), wait);
                }
                for (key, last_signal_at, signal_count) in hub.store.load_care_marks(&snap.room) {
                    room.care_marks.insert(
                        key,
                        CareMark {
                            last_signal_at,
                            signal_count,
                        },
                    );
                }
                for attention in hub.store.attentions(&snap.room) {
                    max_generation = max_generation.max(attention.created_at);
                    if let Some(generation) = attention
                        .id
                        .rsplit(':')
                        .next()
                        .and_then(|part| part.parse::<u64>().ok())
                    {
                        max_generation = max_generation.max(generation);
                    }
                    room.attentions.insert(attention.id.clone(), attention);
                }
                room.last_msg_ms = room.history.last().map(|message| message.ts).unwrap_or(0);
                // The journal is the record of what happened; a restart must
                // not be able to forget it.
                room.journal = hub.store.load_journal(&snap.room);
                room.next_journal_id = room.journal.last().map(|e| e.id + 1).unwrap_or(1);
                rooms.insert(snap.room, room);
                max_id = max_id.max(snap.max_msg_id);
            }
        }
        hub.next_id.store(max_id + 1, Ordering::Relaxed);
        hub.condition_generation
            .store(max_generation, Ordering::Relaxed);
        {
            let mut mem = hub.members.lock_or_recover();
            for m in hub.store.load_members() {
                mem.insert(m.token.clone(), m);
            }
        }
        {
            let mut sm = hub.smasters.lock_or_recover();
            for (token, name) in hub.store.load_smasters() {
                sm.insert(token, name);
            }
        }

        // Legacy room settings stored operator display names. Bind only an
        // unambiguous single human label to a principal, then erase the legacy
        // label from settings. Multiple labels, duplicate names, or a missing
        // human are reported and fail closed: migration must never guess who
        // receives authority. Existing principal assignments are authoritative
        // and merely cause the obsolete setting to be cleaned up.
        if hub.store.is_persistent() {
            let master = hub.store.active_master_principal();
            let candidates: Vec<(String, ChatMode, RoomSettings)> = hub
                .rooms
                .lock_or_recover()
                .iter()
                .filter(|(_, room)| !room.settings.operators.is_empty())
                .map(|(name, room)| (name.clone(), room.mode.clone(), room.settings.clone()))
                .collect();
            for (room_name, mode, mut settings) in candidates {
                let existing = hub.store.loca_operator(&room_name);
                let migrated = if existing.is_some() {
                    true
                } else if settings.operators.len() != 1 {
                    tracing::warn!(
                        room = %room_name,
                        legacy_operator_count = settings.operators.len(),
                        "legacy Loca Operator migration needs explicit resolution"
                    );
                    false
                } else {
                    let label = &settings.operators[0];
                    let matches = hub.store.active_human_principals_named(label);
                    if matches.len() != 1 {
                        tracing::warn!(
                            room = %room_name,
                            operator = %label,
                            matching_principals = matches.len(),
                            "legacy Loca Operator label is not uniquely bound; authority withheld"
                        );
                        false
                    } else if let Some(master) = master.as_ref() {
                        match hub.store.appoint_loca_operator(
                            &room_name,
                            &matches[0].id,
                            &master.id,
                            hub.now(),
                        ) {
                            Ok(_) => true,
                            Err(error) => {
                                tracing::error!(
                                    room = %room_name,
                                    operator = %label,
                                    ?error,
                                    "legacy Loca Operator migration failed"
                                );
                                false
                            }
                        }
                    } else {
                        tracing::error!(
                            room = %room_name,
                            "legacy Loca Operator migration has no Master principal"
                        );
                        false
                    }
                };
                if migrated {
                    settings.operators.clear();
                    if let Err(error) = hub.store.save_room(&room_name, &mode, &settings) {
                        tracing::error!(
                            room = %room_name,
                            %error,
                            "failed to clear migrated legacy operator setting"
                        );
                    } else if let Some(room) = hub.rooms.lock_or_recover().get_mut(&room_name) {
                        room.settings = settings;
                    }
                }
            }
        }
        {
            let mut sessions = hub.sessions.lock_or_recover();
            for (token, name, kind, expires_at) in hub.store.load_admin_sessions(hub.now()) {
                let principal = hub.store.active_master_principal();
                if !hub.store.principal_session_exists(&token) {
                    if let (Some(principal), Some(credential)) = (
                        principal.as_ref(),
                        hub.store.credential_id_for_secret(&hub.admin_token),
                    ) {
                        let _ = hub.store.save_principal_session(
                            &token,
                            &principal.id,
                            &credential,
                            hub.now(),
                            expires_at,
                        );
                    }
                }
                sessions.insert(
                    token,
                    SessionIdentity {
                        principal_id: principal.as_ref().map(|identity| identity.id.clone()),
                        building_role: BuildingRole::Master,
                        name,
                        kind,
                        loca: None,
                        member: None,
                        admin: true,
                        expires_at,
                    },
                );
            }
        }

        // Migration: a davet now seats a MEMBER, but legacy davets predate the
        // link (empty `member`). Bind each to the membership carrying its name;
        // when none exists, mint one marked `migration:legacy-invite` — the
        // invite-only identities of the old model become real members, so no
        // one loses a seat they already held. After this pass every live davet
        // points at a membership and the new invariant holds building-wide.
        {
            let mut inv = hub.invites.lock_or_recover();
            let mut mem = hub.members.lock_or_recover();
            let orphans: Vec<String> = inv
                .values()
                .filter(|i| i.member.is_empty())
                .map(|i| i.token.clone())
                .collect();
            for token in orphans {
                let (name, kind) = {
                    let i = &inv[&token];
                    (i.name.clone(), i.kind.clone())
                };
                let member_token = match mem.values().find(|m| m.name == name) {
                    Some(m) => m.token.clone(),
                    None => {
                        let m = protocol::Membership {
                            token: hub.new_invite_token().replacen("dv_", "mb_", 1),
                            name: name.clone(),
                            kind,
                            joined_at: hub.now(),
                            admitted_by: "migration:legacy-invite".into(),
                        };
                        let _ = hub.store.add_member(&m);
                        let t = m.token.clone();
                        mem.insert(t.clone(), m);
                        t
                    }
                };
                if let Some(i) = inv.get_mut(&token) {
                    i.member = member_token;
                    let _ = hub.store.insert_invite(i);
                }
            }
        }

        // The governance loca is not an ordinary project room. Master and
        // smaster enter by rank; only the two building caretakers may hold
        // invitations. Clean legacy seats at boot so a rename into the
        // reserved loca cannot carry ordinary members across with it.
        if !hub.reserved_room.is_empty() {
            let stale: Vec<String> = hub
                .invites
                .lock_or_recover()
                .values()
                .filter(|invite| {
                    invite.room == *hub.reserved_room && !hub.caretakers.contains(&invite.name)
                })
                .map(|invite| invite.token.clone())
                .collect();
            for token in stale {
                hub.store
                    .revoke_invite(&token, hub.now())
                    .expect("persist reserved-loca seat cleanup");
                hub.invites.lock_or_recover().remove(&token);
            }
        }

        // The configured home room always exists.
        // It may be created here rather than restored from a snapshot (a loca
        // with a journal but no messages leaves no snapshot), so its journal
        // has to be loaded either way — otherwise a restart forgets the record.
        {
            let mut rooms = hub.rooms.lock_or_recover();
            let room = rooms
                .entry((*hub.home_room).clone())
                .or_insert_with(|| Room::with(ChatMode::Free, hub.default_settings.clone(), 1));
            if room.journal.is_empty() {
                room.journal = hub.store.load_journal(&hub.home_room);
                room.next_journal_id = room.journal.last().map(|e| e.id + 1).unwrap_or(1);
            }
        }

        // Bans and mutes survive the restart (PRINCIPLES: "restart odayı
        // öldürmez"). Load them into their rooms so the door is exactly as the
        // operator left it — otherwise a banned name's davet reloads and walks
        // back in.
        {
            let mut rooms = hub.rooms.lock_or_recover();
            for (room, name, kind) in hub.store.load_bans() {
                let r = Self::room_mut(&mut rooms, &room, &hub.default_settings);
                match kind.as_str() {
                    "ban" => {
                        r.banned.insert(name);
                    }
                    "mute" => {
                        r.muted.insert(name);
                    }
                    _ => {}
                }
            }
        }

        // Sealed locas stay sealed across a restart: their history is kept on
        // disk but they are re-tombstoned so no subscribe/join/post reopens
        // them (PRINCIPLES: seal not destroy, and restart does not undo it).
        {
            let mut deleted = hub.deleted.lock_or_recover();
            for room in hub.store.sealed_rooms() {
                deleted.insert(room);
            }
        }
        hub
    }

    /// True if `token` authorizes admin actions. When no token is configured,
    /// any request is treated as admin (dev mode).
    /// May this token act — issue davets, run a loca, change settings?
    /// True for the master and for every smaster: a smaster does everything a
    /// master does.
    pub fn is_admin(&self, token: Option<&str>) -> bool {
        self.is_master(token) || self.smaster_name(token).is_some()
    }

    /// True when no ADMIN_TOKEN is configured: a dev server with no host, where
    /// everyone present acts as the operator.
    pub fn admin_open(&self) -> bool {
        self.admin_token.is_empty()
    }

    /// Is this the master themselves? The master has the last word, so this is
    /// what guards the few things a smaster must not do: undoing the master's
    /// own decisions, and minting or revoking smasters.
    pub fn is_master(&self, token: Option<&str>) -> bool {
        self.admin_token.is_empty() || token == Some(self.admin_token.as_str())
    }

    /// Resolve authority from server-held credentials/session state. Client
    /// names and request payload roles are deliberately absent from this API.
    pub fn resolve_authority(
        &self,
        admin_token: Option<&str>,
        session_token: Option<&str>,
    ) -> RequestAuthority {
        if let Some(session) = self.session_identity(session_token) {
            if let Some(principal_id) = session.principal_id.as_deref() {
                return authority_from_principal(self.store.active_principal(principal_id))
                    .unwrap_or(RequestAuthority {
                        principal_id: None,
                        display_name: None,
                        kind: None,
                        building_role: None,
                    });
            }
            return RequestAuthority {
                principal_id: session.principal_id,
                display_name: Some(session.name),
                kind: Some(session.kind),
                building_role: Some(session.building_role),
            };
        }
        let Some(token) = admin_token.filter(|token| !token.is_empty()) else {
            return if self.admin_token.is_empty() {
                RequestAuthority {
                    principal_id: None,
                    display_name: Some("operator".into()),
                    kind: Some(SenderType::User),
                    building_role: Some(BuildingRole::Master),
                }
            } else {
                RequestAuthority {
                    principal_id: None,
                    display_name: None,
                    kind: None,
                    building_role: None,
                }
            };
        };
        if let Some(authority) =
            authority_from_principal(self.store.principal_for_credential(token))
        {
            return authority;
        }
        if self.is_master(Some(token)) {
            return authority_from_principal(self.store.principal_for_credential(token)).unwrap_or(
                RequestAuthority {
                    principal_id: None,
                    display_name: Some("operator".into()),
                    kind: Some(SenderType::User),
                    building_role: Some(BuildingRole::Master),
                },
            );
        }
        if self.smaster_name(Some(token)).is_some() {
            return authority_from_principal(self.store.principal_for_credential(token)).unwrap_or(
                RequestAuthority {
                    principal_id: None,
                    display_name: self.smaster_name(Some(token)),
                    kind: Some(SenderType::User),
                    building_role: Some(BuildingRole::Smaster),
                },
            );
        }
        RequestAuthority {
            principal_id: None,
            display_name: None,
            kind: None,
            building_role: None,
        }
    }

    pub fn admin_session_grant(
        &self,
        credential: Option<&str>,
        ttl_ms: u64,
    ) -> Option<AdminSessionGrant> {
        let authority = self.resolve_authority(credential, None);
        (authority.principal_id.is_some() || authority.is_building_admin()).then(|| {
            AdminSessionGrant {
                ttl_ms,
                credential_id: credential
                    .and_then(|token| self.store.credential_id_for_secret(token)),
                authority,
            }
        })
    }

    pub fn master_pairing_grant(&self, ttl_ms: u64) -> AdminSessionGrant {
        let authority = authority_from_principal(self.store.active_master_principal()).unwrap_or(
            RequestAuthority {
                principal_id: None,
                display_name: Some("operator".into()),
                kind: Some(SenderType::User),
                building_role: Some(BuildingRole::Master),
            },
        );
        AdminSessionGrant {
            ttl_ms,
            credential_id: self.store.credential_id_for_secret(&self.admin_token),
            authority,
        }
    }

    pub fn credentials_for(&self, principal_id: &str) -> Vec<CredentialSummary> {
        self.store.list_credentials(principal_id)
    }

    pub fn credential_id_for_request(
        &self,
        credential: Option<&str>,
        session: Option<&str>,
    ) -> Option<String> {
        session
            .and_then(|secret| self.store.credential_id_for_session(secret, self.now()))
            .or_else(|| credential.and_then(|secret| self.store.credential_id_for_secret(secret)))
    }

    pub fn credential_is_revoked(&self, credential: &str) -> bool {
        self.store.credential_is_revoked(credential)
    }

    pub fn create_profile_credential(
        &self,
        principal_id: &str,
        label: &str,
    ) -> Result<(CredentialSummary, String), CredentialError> {
        let secret = Self::secure_token("ak_", 32);
        let summary = self
            .store
            .create_credential(principal_id, label, &secret, self.now())?;
        Ok((summary, secret))
    }

    pub fn revoke_profile_credential(
        &self,
        principal_id: &str,
        credential_id: &str,
    ) -> Result<(), CredentialError> {
        let legacy = self
            .store
            .legacy_credential_source(principal_id, credential_id);
        self.store
            .revoke_credential(principal_id, credential_id, self.now())?;
        self.sessions
            .lock_or_recover()
            .retain(|session, _| !self.store.session_uses_credential(session, credential_id));
        // v2 authority is DB-backed, but a legacy Smaster key also lives in
        // the in-memory compatibility map. Retire that fallback only after the
        // credential transaction commits, so "revoked" cannot still mean
        // "Smaster through the old path".
        if let Some((role, token)) = legacy {
            if role == "smaster" {
                self.smasters.lock_or_recover().remove(&token);
            }
        }
        Ok(())
    }

    /// The smaster behind this token, if it is a live one.
    pub fn smaster_name(&self, token: Option<&str>) -> Option<String> {
        let token = token?;
        if token.is_empty() {
            return None;
        }
        self.smasters.lock_or_recover().get(token).cloned()
    }

    /// Mint a second master. Only the master may call this — the caller checks.
    pub fn add_smaster(&self, name: &str) -> rusqlite::Result<String> {
        let token = self.new_invite_token().replacen("dv_", "sm_", 1);
        let now = (self.now_ms)();
        self.store.add_smaster(&token, name, now)?;
        self.smasters
            .lock_or_recover()
            .insert(token.clone(), name.to_string());
        Ok(token)
    }

    /// Take one back. The row survives, so "who could act, and when" stays
    /// answerable.
    pub fn revoke_smaster(&self, token: &str) -> rusqlite::Result<bool> {
        let existed = self.smasters.lock_or_recover().contains_key(token);
        if !existed {
            return Ok(false);
        }
        self.store.revoke_smaster(token, (self.now_ms)())?;
        self.smasters.lock_or_recover().remove(token);
        Ok(true)
    }

    pub fn smaster_management_id(&self, token: &str) -> String {
        crate::store::hashed_id("smid_", &format!("smaster:{token}"))
    }

    pub fn revoke_smaster_ref(&self, reference: &str) -> rusqlite::Result<bool> {
        let token = self
            .smasters
            .lock_or_recover()
            .keys()
            .find(|token| {
                token.as_str() == reference || self.smaster_management_id(token) == reference
            })
            .cloned();
        match token {
            Some(token) => self.revoke_smaster(&token),
            None => Ok(false),
        }
    }

    /// Live smasters, as (token, name).
    pub fn smasters(&self) -> Vec<(String, String)> {
        self.smasters
            .lock_or_recover()
            .iter()
            .map(|(t, n)| (t.clone(), n.clone()))
            .collect()
    }

    /// True if `token` may connect / post. Open when no room token is set; the
    /// admin token also counts (admins are members).
    pub fn is_member(&self, token: Option<&str>) -> bool {
        if self.room_token.is_empty() {
            // No building key. In davet-only mode this is not an open house —
            // the door is closed and only a davet (checked per-loca elsewhere)
            // opens it. Being a "member of the building" now means holding the
            // admin token; everyone else needs a davet for the specific loca.
            return !self.require_invite
                || (!self.admin_token.is_empty() && token == Some(self.admin_token.as_str()));
        }
        token == Some(self.room_token.as_str())
            || (!self.admin_token.is_empty() && token == Some(self.admin_token.as_str()))
    }

    /// True if `token` is a davet for THIS loca. A davet opens one loca and no
    /// other — that is the whole point of it (DESIGN §5g).
    /// A davet by its token, whichever loca it belongs to.
    pub fn invite_by_token(&self, token: &str) -> Option<Invite> {
        self.invites.lock_or_recover().get(token).cloned()
    }

    pub fn invite_for(&self, room: &str, token: Option<&str>) -> Option<Invite> {
        let token = token?;
        let inv = self.invites.lock_or_recover().get(token).cloned()?;
        (inv.room == room).then_some(inv)
    }

    /// May this room credential enter `room`? An active per-device credential
    /// inherits the transitional building-key access of its durable Member
    /// record; loca-specific davets remain separate, scoped bearers.
    /// That indirection is important: revoking the original legacy bearer must
    /// close the door, while another credential of the same principal keeps
    /// the principal's inherited building-key access.
    pub fn may_enter(&self, room: &str, token: Option<&str>) -> bool {
        if self.invite_for(room, token).is_some() {
            return true;
        }
        let Some(token) = token else {
            return self.is_member(None);
        };
        if self.store.credential_is_revoked(token) {
            return false;
        }
        if let Some(member) = self.member_for_credential(Some(token)) {
            return self.is_member(Some(&member.token));
        }
        // Memory-only/dev mode and an as-yet unmigrated shared ROOM_TOKEN keep
        // their compatibility behavior. A known revoked credential never
        // reaches this fallback.
        self.is_member(Some(token))
    }

    /// THE single door decision. Every gate — the blanket middleware, the REST
    /// loca check, and the WS handshake — calls this and only this. Before, each
    /// had its own copy of the rules and fixing one broke another; this is the
    /// one place the rules live.
    ///
    /// Inputs are the three credentials a caller can present (HTTP headers on
    /// REST, WebSocket subprotocol credentials on WS):
    ///   - `admin_tok`: the admin key (`x-admin-token`) — master/smaster
    ///   - `davet_tok`: a davet or the building key (`x-room-token`)
    ///   - `session`:   an identity taken earlier (`x-session-token`)
    ///
    /// A `name_for_ban` hint lets the WS door pass the query `?name=` when there
    /// is no session yet; REST passes `None` and the ban is judged from whatever
    /// identity the credentials resolve to.
    pub fn enter_decision(
        &self,
        room: &str,
        admin_tok: Option<&str>,
        davet_tok: Option<&str>,
        session: Option<&SessionIdentity>,
        name_hint: Option<&str>,
    ) -> EnterDecision {
        // Rank first, and rank ignores bans: the master is never banned from
        // their own building. master + smaster, resolved in ONE place — by the
        // raw admin key OR a live admin session (the browser holds the session,
        // not the key). Without the session branch the door turned the master
        // away in davet mode: the admin session's loca is None, so it matched no
        // loca and every room read/WS handshake 401'd.
        let credential_authority = self.resolve_authority(admin_tok, None);
        if credential_authority.is_building_admin() && admin_tok.is_some() {
            return EnterDecision::Allowed;
        }
        if session.map(|s| s.admin).unwrap_or(false) {
            return EnterDecision::Allowed;
        }

        // Whose name is at this door? A session names itself; otherwise a davet
        // carries a name; otherwise the caller's ?name= hint. This is the name a
        // ban is checked against.
        let ban_name = session
            .map(|s| s.name.clone())
            .or_else(|| {
                davet_tok
                    .and_then(|t| self.invite_by_token(t))
                    .map(|i| i.name)
            })
            .or_else(|| credential_authority.display_name.clone())
            .or_else(|| name_hint.map(str::to_string));
        if let Some(n) = &ban_name {
            if self.is_banned(room, n) {
                return EnterDecision::Banned;
            }
        }

        let principal_at_door = session
            .and_then(|identity| identity.principal_id.as_deref())
            .or(credential_authority.principal_id.as_deref());
        if let (Some(principal_id), Some(assignment)) =
            (principal_at_door, self.store.loca_operator(room))
        {
            if assignment.principal_id == principal_id {
                return EnterDecision::Allowed;
            }
        }

        // A davet for THIS loca, or the building key (is_member) — may_enter
        // already folds both. A davet for another loca opens nothing.
        if self.may_enter(room, davet_tok) {
            return EnterDecision::Allowed;
        }

        // A session opens the loca it was minted for.
        if let Some(s) = session {
            match &s.loca {
                Some(l) if l == room => return EnterDecision::Allowed,
                Some(_) => {}
                // A session with no loca came from the building key. What it
                // reaches then depends on the mode:
                //   - building-key mode (ROOM_TOKEN set): it reaches the whole
                //     building, exactly as the key does. The browser relies on
                //     this — it takes a session once and stops sending the key.
                //   - davet-only mode (no key, REQUIRE_INVITE): there is no
                //     building to reach, so a bare loca-less session opens
                //     nothing. This is the hole cyber walked through, now shut.
                // A building-key session reaches the building only while a
                // building key still exists to have reached it. In davet-only
                // mode there is no key, so it opens nothing.
                None if !self.room_token.is_empty() => return EnterDecision::Allowed,
                None => {}
            }
        }

        EnterDecision::Denied
    }

    /// The seat key for a connection — one key = one seat. Derived from the
    /// strongest credential at the door, so the same key entering under two
    /// display names lands on ONE seat (the name is just the label).
    ///
    /// Precedence mirrors `enter_decision`: rank first (the master is the
    /// master even if they also hold a davet), then the davet's member, then
    /// the session, then the bare name.
    ///
    /// In open dev mode (no ADMIN_TOKEN) rank identifies nobody, so identity
    /// falls through to the name — two differently-named anons stay distinct,
    /// exactly as before.
    pub fn seat_identity(
        &self,
        admin_tok: Option<&str>,
        davet_tok: Option<&str>,
        session: Option<&SessionIdentity>,
        session_tok: Option<&str>,
        name: &str,
    ) -> String {
        if let Some(t) = admin_tok {
            if let Some(_smaster) = self.smaster_name(Some(t)) {
                return format!("sm:{t}");
            }
            if !self.admin_token.is_empty() && t == self.admin_token.as_str() {
                // ONE fixed key for the master: whatever name they type, the
                // seat is the same. Per-room, because each roster is per-room.
                return "@master".into();
            }
        }
        if let Some(inv) = davet_tok.and_then(|t| self.invite_by_token(t)) {
            if !inv.member.is_empty() {
                // Two davets of the same member (or the same davet twice) are
                // one person — the membership is the identity.
                return format!("mb:{}", inv.member);
            }
            return format!("dv:{}", inv.token);
        }
        if let Some(member) = self.member_for_credential(davet_tok) {
            return format!("mb:{}", member.token);
        }
        if let Some(s) = session {
            if s.admin {
                // Every session minted for the one Master principal occupies the
                // same per-loca seat, regardless of the display name used
                // while taking the session.
                return "@master".into();
            }
            if let Some(m) = &s.member {
                return format!("mb:{m}");
            }
            if let Some(t) = session_tok {
                return format!("st:{t}");
            }
        }
        format!("name:{name}")
    }

    /// Admit somebody to the building. The founding act: it creates the
    /// identity every davet later refers to.
    /// Admit somebody to the building. Legacy admission remains idempotent by
    /// name, so admitting an existing member returns the existing principal
    /// rather than minting a duplicate membership. Authority still resolves
    /// from principal identity, never from display name or credential text.
    /// The two-step "admit & invite" flow can therefore be replayed safely.
    pub fn admit_member(
        &self,
        name: &str,
        kind: &str,
        by: &str,
    ) -> rusqlite::Result<protocol::Membership> {
        if let Some(existing) = self.member_by_name(name) {
            return Ok(existing);
        }
        let m = protocol::Membership {
            token: self.new_invite_token().replacen("dv_", "mb_", 1),
            name: name.to_string(),
            kind: kind.to_string(),
            joined_at: (self.now_ms)(),
            admitted_by: by.to_string(),
        };
        self.store.add_member(&m)?;
        self.members
            .lock_or_recover()
            .insert(m.token.clone(), m.clone());
        Ok(m)
    }

    /// End a membership. Their davets are not touched here — losing the
    /// building is a decision of its own, and the record of where they sat
    /// stays readable.
    /// End a membership — and everything it authorized. PRINCIPLES: when a
    /// parent credential is revoked, every child dies with it. Losing the
    /// building means losing every seat (davet), every proof (session), and
    /// every open connection at once — otherwise "you no longer belong here"
    /// would leave the ejected person still reading and speaking.
    pub fn revoke_member(&self, token: &str) -> rusqlite::Result<bool> {
        if !self.members.lock_or_recover().contains_key(token) {
            return Ok(false);
        }
        let now = (self.now_ms)();
        self.store.revoke_member_cascade(token, now)?;
        let davets: Vec<Invite> = self
            .invites
            .lock_or_recover()
            .values()
            .filter(|i| i.member == token)
            .cloned()
            .collect();
        {
            let mut invites = self.invites.lock_or_recover();
            for inv in &davets {
                invites.remove(&inv.token);
            }
        }
        for inv in davets {
            self.revoke_sessions_for_davet(&inv.room, &inv.member);
            self.close_connections(&inv.room, &inv.name);
        }
        // Any session still bound to this member (e.g. a building-key path)
        // dies too — the identity behind it is gone.
        self.revoke_sessions_for_member(token);
        self.members.lock_or_recover().remove(token);
        let _ = self.lobby_tx.send(LobbyEvent::MembershipRevoked {
            member: token.to_string(),
        });
        Ok(true)
    }

    pub fn member_management_id(&self, token: &str) -> String {
        crate::store::hashed_id("mbid_", &format!("member:{token}"))
    }

    pub fn revoke_member_ref(&self, reference: &str) -> rusqlite::Result<bool> {
        let token = self
            .members
            .lock_or_recover()
            .keys()
            .find(|token| {
                token.as_str() == reference || self.member_management_id(token) == reference
            })
            .cloned();
        match token {
            Some(token) => self.revoke_member(&token),
            None => Ok(false),
        }
    }

    /// The member behind this key, if it is a live one.
    pub fn member_of(&self, token: Option<&str>) -> Option<protocol::Membership> {
        let token = token?;
        self.members.lock_or_recover().get(token).cloned()
    }

    /// Resolve an active per-device/member credential to its durable Building
    /// membership. Unlike `member_of`, this is safe at an HTTP authentication
    /// boundary: revoking the legacy bearer makes that bearer stop working,
    /// while another credential for the same principal still finds the same
    /// membership record.
    pub fn member_for_credential(&self, token: Option<&str>) -> Option<protocol::Membership> {
        if !self.store.is_persistent() {
            return self.member_of(token);
        }
        let principal = self.store.principal_for_credential(token?)?;
        if principal.role != BuildingRole::Member {
            return None;
        }
        self.members
            .lock_or_recover()
            .values()
            .find(|member| {
                self.store
                    .principal_id_for_member_record(&member.token)
                    .as_deref()
                    == Some(principal.id.as_str())
            })
            .cloned()
    }

    pub fn memberships(&self) -> Vec<protocol::Membership> {
        self.members.lock_or_recover().values().cloned().collect()
    }

    /// The membership carrying this name, if the building knows them.
    pub fn member_by_name(&self, name: &str) -> Option<protocol::Membership> {
        self.members
            .lock_or_recover()
            .values()
            .find(|m| m.name == name)
            .cloned()
    }

    /// Does this member already hold a live davet for this loca?
    pub fn has_active_invite(&self, room: &str, member: &str) -> bool {
        self.invites
            .lock_or_recover()
            .values()
            .any(|i| i.room == room && i.member == member)
    }

    const RUNTIME_SEEN_TTL_MS: u64 = 20_000;
    const RUNTIME_WAKE_GRACE_MS: u64 = 10_000;
    const RUNTIME_ACK_GRACE_MS: u64 = 360_000;

    /// Which loca does this davet open, if it is one?
    pub fn invite_room(&self, token: &str) -> Option<String> {
        self.invites
            .lock_or_recover()
            .get(token)
            .map(|i| i.room.clone())
    }

    /// Is this token a live davet at all — for any loca? Used by the blanket
    /// gate, which cannot know the target loca; the per-loca decision is
    /// `may_enter`.
    pub fn is_invite_token(&self, token: Option<&str>) -> bool {
        token.is_some_and(|t| self.invites.lock_or_recover().contains_key(t))
    }

    /// Current time by the hub's own clock (tests can swap it).
    pub fn now(&self) -> u64 {
        (self.now_ms)()
    }

    fn next_condition_generation(&self, now: u64, previous: u64) -> u64 {
        let floor = now.max(previous.saturating_add(1));
        loop {
            let current = self.condition_generation.load(Ordering::Relaxed);
            let next = floor.max(current.saturating_add(1));
            if self
                .condition_generation
                .compare_exchange(current, next, Ordering::SeqCst, Ordering::Relaxed)
                .is_ok()
            {
                return next;
            }
        }
    }

    /// Mint a davet token from the operating system CSPRNG. Davets, memberships
    /// and smaster credentials are long-lived bearer authority, so a seeded
    /// hash/clock/counter construction is not an acceptable source of entropy.
    pub fn new_invite_token(&self) -> String {
        Self::secure_token("dv_", 32)
    }

    /// THE one way a davet comes to exist. Both doors — the terminal
    /// (`POST /invites` via invite.sh) and the UI (`+ call` via
    /// `POST /rooms/:id/call`) — land here, so the rules cannot drift apart
    /// again: a davet seats an EXISTING member (it never creates identity),
    /// and one member holds at most one live davet per loca.
    ///
    /// Capacity stays a door rule (`join`), not an invite rule: "a full loca
    /// refuses the eighth even with a davet" is the tested behaviour — more
    /// davets than seats may exist, the seats decide.
    pub fn invite_member_to_room(
        &self,
        member_token: &str,
        room: &str,
        issued_by: &str,
    ) -> Result<Invite, InviteError> {
        let Some(member) = self.member_of(Some(member_token)) else {
            return Err(InviteError::MemberNotFound);
        };
        if room == self.reserved_room.as_str() && !self.caretakers.contains(&member.name) {
            return Err(InviteError::Reserved);
        }
        if self.has_active_invite(room, member_token) {
            return Err(InviteError::AlreadyInvited);
        }
        // Model A: the seat IS the davet (PRINCIPLES: "davetli — bir locada
        // koltuğu vardır"). A loca seats at most LOCA_KAPASITE, so no more than
        // that many live davets may exist for it — the cap is checked here at
        // mint, not only when someone connects. (The join-time guard stays as
        // defence in depth.)
        let active = self
            .invites
            .lock_or_recover()
            .values()
            .filter(|i| i.room == room)
            .count();
        if active >= Self::LOCA_KAPASITE {
            return Err(InviteError::Full);
        }
        let inv = Invite {
            token: self.new_invite_token(),
            room: room.to_string(),
            member: member.token.clone(),
            // Snapshot for the audit trail — identity lives in the membership.
            name: member.name.clone(),
            kind: member.kind.clone(),
            issued_at: self.now(),
            issued_by: issued_by.to_string(),
        };
        self.add_invite(inv.clone())
            .map_err(|_| InviteError::Storage)?;
        // Wake an already-running agent in the building lobby. Failure only
        // means nobody is listening at this instant; the persisted invite is
        // replayed when their lobby socket reconnects.
        let _ = self.lobby_tx.send(LobbyEvent::Called {
            member: member.token,
            room: inv.room.clone(),
            token: inv.token.clone(),
        });
        Ok(inv)
    }

    /// Subscribe before taking the invite snapshot, so a call racing the
    /// handshake is observed either in the snapshot or on the bus (possibly
    /// both; clients de-duplicate by loca/token).
    pub fn subscribe_lobby(&self) -> broadcast::Receiver<LobbyEvent> {
        self.lobby_tx.subscribe()
    }

    pub fn home_room(&self) -> &str {
        &self.home_room
    }

    pub fn is_reserved_room(&self, room: &str) -> bool {
        !self.reserved_room.is_empty() && room == self.reserved_room.as_str()
    }

    pub fn is_caretaker(&self, name: &str) -> bool {
        self.caretakers.contains(name)
    }

    pub fn caretaker_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.caretakers.iter().cloned().collect();
        names.sort();
        names
    }

    pub fn lobby_join(&self, member: &str) {
        let mut online = self.lobby_online.lock_or_recover();
        *online.entry(member.to_string()).or_insert(0) += 1;
    }

    pub fn lobby_leave(&self, member: &str) {
        let mut online = self.lobby_online.lock_or_recover();
        if let Some(count) = online.get_mut(member) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                online.remove(member);
            }
        }
    }

    fn lobby_is_online(&self, member: &str) -> bool {
        self.lobby_online
            .lock_or_recover()
            .get(member)
            .copied()
            .unwrap_or(0)
            > 0
    }

    /// Record a davet. The master issues; we only persist and index it.
    pub fn add_invite(&self, inv: Invite) -> rusqlite::Result<()> {
        // Issuing a davet for a loca is a deliberate act of opening it, so it
        // clears any tombstone — a master inviting someone into a room they
        // once deleted means they want it back.
        self.store.insert_invite(&inv)?;
        self.revive(&inv.room);
        self.invites
            .lock_or_recover()
            .insert(inv.token.clone(), inv);
        Ok(())
    }

    /// End a davet. The row survives with `revoked_at` so "who was let in"
    /// stays answerable — revoking is not forgetting.
    ///
    /// Revoking cascades: a davet that dies takes with it any session minted
    /// from it and closes the connection it opened. Otherwise the door would be
    /// shut on paper while a live socket (and a still-valid session token) kept
    /// working — the audit's revoke-does-nothing hole.
    pub fn revoke_invite(&self, token: &str) -> rusqlite::Result<bool> {
        let Some(inv) = self.invites.lock_or_recover().get(token).cloned() else {
            return Ok(false);
        };
        self.store.revoke_invite(token, (self.now_ms)())?;
        self.invites.lock_or_recover().remove(token);
        // Kill sessions bound to this davet's (loca, member) and close the
        // live connection in that loca for that name.
        self.revoke_sessions_for_davet(&inv.room, &inv.member);
        self.close_connections(&inv.room, &inv.name);
        Ok(true)
    }

    pub fn invite_management_id(&self, token: &str) -> String {
        crate::store::hashed_id("ivid_", &format!("invite:{token}"))
    }

    pub fn invite_by_ref(&self, reference: &str) -> Option<Invite> {
        self.invites
            .lock_or_recover()
            .values()
            .find(|invite| {
                invite.token == reference || self.invite_management_id(&invite.token) == reference
            })
            .cloned()
    }

    pub fn revoke_invite_ref(&self, reference: &str) -> rusqlite::Result<bool> {
        match self.invite_by_ref(reference) {
            Some(invite) => self.revoke_invite(&invite.token),
            None => Ok(false),
        }
    }

    /// Drop every session bound to a member (any loca). Used when a MEMBERSHIP
    /// is revoked — the identity itself is gone, so no session proving it may
    /// survive.
    fn revoke_sessions_for_member(&self, member_token: &str) {
        self.sessions
            .lock_or_recover()
            .retain(|_st, idy| idy.member.as_deref() != Some(member_token));
    }

    /// Drop every session bound to one davet's (loca, member). Used when a
    /// single DAVET is revoked — sessions for that member in OTHER locas (other
    /// davets) survive, only this seat's proof dies.
    fn revoke_sessions_for_davet(&self, room: &str, member_token: &str) {
        if member_token.is_empty() {
            return;
        }
        self.sessions.lock_or_recover().retain(|_st, idy| {
            !(idy.member.as_deref() == Some(member_token) && idy.loca.as_deref() == Some(room))
        });
    }

    /// Force any live socket in `room` held under `name` to close, by
    /// broadcasting the same `Kicked` frame moderation uses. This is the only
    /// way to reach into an already-open ws connection (authorization is
    /// checked at the handshake, not per-frame), so a cascade that must eject
    /// someone reuses it.
    fn close_connections(&self, room: &str, name: &str) {
        let rooms = self.rooms.lock_or_recover();
        if let Some(r) = rooms.get(room) {
            let _ = r.tx.send(ServerFrame::Kicked {
                name: name.to_string(),
                banned: false,
            });
        }
    }

    /// End every live davet a name holds for one loca. This is how "çıkar"
    /// (kick) and "bırak" (release) stop the davet — without it, the ejected
    /// name's token would open the door right back (PRINCIPLES: kick stops the
    /// davet; release takes back the seat). The membership is untouched.
    pub fn revoke_invites_for(&self, room: &str, name: &str) -> rusqlite::Result<()> {
        let matches: Vec<Invite> = self
            .invites
            .lock_or_recover()
            .values()
            .filter(|i| i.room == room && i.name == name)
            .cloned()
            .collect();
        if matches.is_empty() {
            return Ok(());
        }
        self.store.revoke_invites_for(room, name, (self.now_ms)())?;
        {
            let mut invites = self.invites.lock_or_recover();
            for inv in &matches {
                invites.remove(&inv.token);
            }
        }
        for inv in matches {
            self.revoke_sessions_for_davet(&inv.room, &inv.member);
            self.close_connections(&inv.room, &inv.name);
        }
        Ok(())
    }

    /// Every live davet held by one member, across all locas — for "who am I"
    /// and member views.
    pub fn invites_for_member(&self, member_token: &str) -> Vec<Invite> {
        self.invites
            .lock_or_recover()
            .values()
            .filter(|i| i.member == member_token)
            .cloned()
            .collect()
    }

    /// Live davetler for one loca (master view).
    pub fn invites_of(&self, room: &str) -> Vec<Invite> {
        let mut v: Vec<Invite> = self
            .invites
            .lock_or_recover()
            .values()
            .filter(|i| i.room == room)
            .cloned()
            .collect();
        v.sort_by_key(|i| i.issued_at);
        v
    }

    /// A unique id for one WS session (used for last-writer-wins eviction).
    pub fn next_session_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Server boot epoch (for client restart detection).
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Whether posts must carry a valid session token (`REQUIRE_SESSIONS`).
    pub fn require_sessions(&self) -> bool {
        self.require_sessions
    }

    /// Default lifetime when no duration was selected (legacy API / boot code).
    pub const ADMIN_SESSION_TTL_MS: u64 = 12 * 60 * 60 * 1000;
    pub const MIN_ADMIN_SESSION_TTL_MS: u64 = 60 * 60 * 1000;
    pub const MAX_ADMIN_SESSION_TTL_MS: u64 = 365 * 24 * 60 * 60 * 1000;
    /// A pairing code is only the bridge into an admin session. Keeping an
    /// unused bearer alive longer than a few minutes widens the log/screen
    /// shoulder-surfing window without helping the operator.
    pub const ADMIN_PAIRING_TTL_MS: u64 = 5 * 60 * 1000;

    fn secure_token(prefix: &str, bytes: usize) -> String {
        use std::fmt::Write as _;

        use rand::{rngs::OsRng, RngCore};

        let mut random = vec![0_u8; bytes];
        OsRng.fill_bytes(&mut random);
        let mut token = String::with_capacity(prefix.len() + random.len() * 2);
        token.push_str(prefix);
        for byte in random {
            write!(&mut token, "{byte:02x}").expect("writing to a String cannot fail");
        }
        token
    }

    /// The boot-time pairing code is printed only by the server process. It is
    /// a one-use stand-in, never the root key itself.
    pub fn admin_pairing_code(&self) -> Option<String> {
        if self.admin_token.is_empty() {
            None
        } else {
            let current = self.admin_pairing.lock_or_recover();
            ((self.now_ms)() < current.expires_at_ms).then(|| current.code.clone())
        }
    }

    /// Rotate a pairing code and bind the operator-selected session lifetime
    /// to that exact one-use code. Validation belongs at the HTTP boundary;
    /// clamping here is defence in depth for internal callers.
    pub fn rotate_admin_pairing_for(&self, session_ttl_ms: u64) -> Option<(String, u64)> {
        if self.admin_token.is_empty() {
            return None;
        }
        let expires_at_ms = (self.now_ms)().saturating_add(Self::ADMIN_PAIRING_TTL_MS);
        let mut current = self.admin_pairing.lock_or_recover();
        *current = AdminPairing {
            code: Self::secure_token("pair_", 16),
            session_ttl_ms: session_ttl_ms.clamp(
                Self::MIN_ADMIN_SESSION_TTL_MS,
                Self::MAX_ADMIN_SESSION_TTL_MS,
            ),
            expires_at_ms,
        };
        Some((current.code.clone(), current.expires_at_ms))
    }

    /// Consume the current browser pairing code exactly once. Comparison walks
    /// the full fixed-length value so a remote caller gets no useful prefix
    /// timing signal.
    pub fn consume_admin_pairing(&self, candidate: &str) -> Option<u64> {
        if self.admin_token.is_empty() {
            return None;
        }
        let mut current = self.admin_pairing.lock_or_recover();
        let now = (self.now_ms)();
        if now >= current.expires_at_ms {
            *current = AdminPairing {
                code: Self::secure_token("pair_", 16),
                session_ttl_ms: Self::ADMIN_SESSION_TTL_MS,
                expires_at_ms: now.saturating_add(Self::ADMIN_PAIRING_TTL_MS),
            };
            return None;
        }
        let a = current.code.as_bytes();
        let b = candidate.as_bytes();
        let mut difference = a.len() ^ b.len();
        for index in 0..a.len().max(b.len()) {
            difference |= usize::from(
                a.get(index).copied().unwrap_or(0) ^ b.get(index).copied().unwrap_or(0),
            );
        }
        if difference != 0 {
            return None;
        }
        let session_ttl_ms = current.session_ttl_ms;
        *current = AdminPairing {
            code: Self::secure_token("pair_", 16),
            session_ttl_ms: Self::ADMIN_SESSION_TTL_MS,
            expires_at_ms: now.saturating_add(Self::ADMIN_PAIRING_TTL_MS),
        };
        tracing::info!("master browser paired; next one-use pairing code is ready");
        Some(session_ttl_ms)
    }

    /// A Master pre-mints `count` single-use, time-limited Lobby-admission
    /// rights. Returns `(total_ever, available_now)`. Master authority is
    /// enforced at the route (`is_master_req`); `minted_by` is the Master
    /// principal. The right ids are server-generated CSPRNG tokens and are NOT
    /// returned — a right is delivered only when the join-request approve step
    /// consumes it.
    pub fn mint_admission_stock(&self, count: u32, ttl_ms: u64) -> (u64, u64, u64) {
        let now = (self.now_ms)();
        let expires_at = now.saturating_add(ttl_ms);
        let minted_by = self
            .store
            .active_master_principal()
            .map(|principal| principal.id)
            .unwrap_or_else(|| "master".to_string());
        let ids: Vec<String> = (0..count).map(|_| Self::secure_token("adm_", 24)).collect();
        let minted = self
            .store
            .mint_admission_rights(&ids, &minted_by, now, expires_at)
            .unwrap_or(0) as u64;
        let (total, available) = self.store.admission_stock_counts(now);
        (minted, total, available)
    }

    /// `(total_ever, available_now)` admission-stock summary for the Master view.
    pub fn admission_stock_summary(&self) -> (u64, u64) {
        self.store.admission_stock_counts((self.now_ms)())
    }

    /// Max undecided join requests at once — a DoS guard on the authless create
    /// endpoint (a full per-source rate-limiter is a documented hardening
    /// follow-up). A request grants no authority, so the only abuse is table
    /// growth, which this bounds.
    const MAX_PENDING_JOIN_REQUESTS: usize = 200;
    const JOIN_CREATE_WINDOW_MS: u64 = 60_000;
    const JOIN_CREATE_MAX_IN_WINDOW: usize = 30;

    /// Per-source (peer IP) sliding-window limit on join-request creation (review
    /// re-blocker #3): one abusive source is throttled on its own bucket without
    /// affecting anyone else. Returns true, and records the attempt, when a create
    /// is allowed for `ip`. Empty buckets are reaped so the map cannot grow
    /// unbounded.
    fn join_request_rate_ok(&self, ip: std::net::IpAddr) -> bool {
        let now = (self.now_ms)();
        let mut by_ip = self.join_create_times.lock_or_recover();
        by_ip.retain(|_, times| {
            times.retain(|&t| now.saturating_sub(t) < Self::JOIN_CREATE_WINDOW_MS);
            !times.is_empty()
        });
        let times = by_ip.entry(ip).or_default();
        if times.len() >= Self::JOIN_CREATE_MAX_IN_WINDOW {
            return false;
        }
        times.push(now);
        true
    }

    /// An outside agent asks to join, picking its own name. The plaintext secret
    /// is produced exactly once here and only its hash is stored, so only the
    /// requester can poll or bootstrap it. Grants NOTHING until a Master approves.
    /// The name must be free — neither an existing member nor an already-pending
    /// request — which (with the approve-time atomic re-check) closes the identity
    /// takeover and the two-same-name race (review blocker #1). Rate-limited per
    /// source IP.
    pub fn create_join_request(
        &self,
        name: &str,
        kind: &str,
        ip: std::net::IpAddr,
    ) -> JoinRequestCreate {
        if !self.join_request_rate_ok(ip) {
            return JoinRequestCreate::BacklogFull;
        }
        if self.member_by_name(name).is_some() || self.store.has_pending_join_request_named(name) {
            return JoinRequestCreate::NameTaken;
        }
        if self.store.list_pending_join_requests().len() >= Self::MAX_PENDING_JOIN_REQUESTS {
            return JoinRequestCreate::BacklogFull;
        }
        let request_id = Self::secure_token("jr_", 12);
        let request_secret = Self::secure_token("jrs_", 24);
        let _ = self.store.create_join_request(
            &request_id,
            &request_secret,
            name,
            kind,
            (self.now_ms)(),
        );
        JoinRequestCreate::Created {
            request_id,
            request_secret,
        }
    }

    /// Poll a join request with its secret: `(status, name, bootstrap_ready)`.
    pub fn join_request_view(&self, id: &str, secret: &str) -> Option<(String, String, bool)> {
        self.store.join_request_view(id, secret)
    }

    /// The Master's pending-request review list.
    pub fn list_pending_join_requests(&self) -> Vec<(String, String, String, u64)> {
        self.store.list_pending_join_requests()
    }

    /// Approve a pending join request (Master action). EXACTLY-ONCE: atomically
    /// claims the request first; a repeated or racing approve that finds it
    /// already decided returns `Approve::AlreadyDecided` and consumes NO stock.
    /// On a fresh claim it consumes one admission-stock right and issues a Lobby
    /// membership (`mb_`, kind `agent` per the model) bound to the requested name;
    /// if the stock is empty it releases the claim and returns `Approve::NoStock`.
    pub fn approve_join_request(&self, id: &str, by: &str) -> Approve {
        // The ENTIRE approval — pending-guard, name-free check, stock consume,
        // member insert, request-finalize — happens in ONE store transaction
        // (`approve_join_request_atomic`). This closes the identity-takeover race
        // (no `/members` create can slip between the name check and the insert:
        // the single conn Mutex is held throughout) AND removes every partial
        // state: a failure at any step leaves the request pending, the name free,
        // and the stock intact — so there is no compensating refund/release to
        // orchestrate here, and no way to strand a request or burn a right.
        // We only mint the fresh `mb_` token up front (needs the Hub's CSPRNG).
        let mb_token = self.new_invite_token().replacen("dv_", "mb_", 1);
        match self
            .store
            .approve_join_request_atomic(id, &mb_token, by, (self.now_ms)())
        {
            crate::store::ApproveTxn::Committed(member) => {
                // Mirror the committed DB row into the in-memory member cache.
                self.members
                    .lock_or_recover()
                    .insert(member.token.clone(), member);
                Approve::Approved
            }
            crate::store::ApproveTxn::AlreadyDecided => Approve::AlreadyDecided,
            crate::store::ApproveTxn::NameTaken => Approve::NameTaken,
            crate::store::ApproveTxn::NoStock => Approve::NoStock,
            crate::store::ApproveTxn::Failed => Approve::Failed,
        }
    }

    /// Deny a pending join request (Master action). True iff it was pending.
    pub fn deny_join_request(&self, id: &str, by: &str) -> bool {
        self.store.deny_join_request(id, by, (self.now_ms)())
    }

    /// Deliver an approved request's `mb_` exactly once (requester bootstrap).
    pub fn claim_join_request_bootstrap(&self, id: &str, secret: &str) -> Option<String> {
        self.store
            .claim_join_request_bootstrap(id, secret, (self.now_ms)())
    }

    /// Same, but taken with a davet: the session is confined to the davet's
    /// loca AND its identity comes from the davet's member — the session is
    /// proof of who the davet seats, not of whatever name the request body
    /// claims. (Before this, alice's davet could mint a "bob" session.)
    ///
    /// `admin_grant` is resolved server-side from a principal credential (or
    /// the one-use root pairing). It binds both role and credential ownership;
    /// request-body names never manufacture authority.
    pub fn create_session_scoped(
        &self,
        req: protocol::CreateSession,
        davet: Option<&Invite>,
        admin_grant: Option<AdminSessionGrant>,
    ) -> rusqlite::Result<protocol::SessionInfo> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        // A session is a bearer credential. Generate it directly from the
        // operating system CSPRNG; timestamps, counters and SipHash output are
        // not suitable secrets.
        let t = Self::secure_token("st_", 32);
        let identity = match davet {
            Some(inv) => {
                // The membership is the identity; the invite's snapshot is the
                // fallback if the member record has since been revoked.
                let (name, kind) = match self.member_of(Some(inv.member.as_str())) {
                    Some(m) => (m.name, m.kind),
                    None => (inv.name.clone(), inv.kind.clone()),
                };
                let kind = match kind.as_str() {
                    "user" => SenderType::User,
                    _ => SenderType::Agent,
                };
                SessionIdentity {
                    principal_id: self
                        .store
                        .principal_for_credential(&inv.member)
                        .map(|identity| identity.id),
                    building_role: BuildingRole::Member,
                    name,
                    kind,
                    loca: Some(inv.room.clone()),
                    member: Some(inv.member.clone()),
                    admin: false,
                    expires_at: 0,
                }
            }
            // No davet: this is a building-key session. There is no member
            // behind a building key, so `member` is None and the name comes
            // from the body. In davet-only mode (prod: REQUIRE_INVITE=1, no
            // ROOM_TOKEN) the only building key is the admin token, so this
            // path is the master/browser — a body name here is the master
            // naming themselves, not a spoof. If a shared ROOM_TOKEN is ever
            // reintroduced this branch would need a member/role binding.
            None => SessionIdentity {
                principal_id: admin_grant
                    .as_ref()
                    .and_then(|grant| grant.authority.principal_id.clone()),
                building_role: admin_grant
                    .as_ref()
                    .and_then(|grant| grant.authority.building_role)
                    .unwrap_or(BuildingRole::Member),
                name: admin_grant
                    .as_ref()
                    .and_then(|grant| grant.authority.display_name.clone())
                    .unwrap_or_else(|| req.name.clone()),
                kind: admin_grant
                    .as_ref()
                    .and_then(|grant| grant.authority.kind)
                    .unwrap_or_else(|| req.kind.unwrap_or(SenderType::Agent)),
                loca: None,
                member: None,
                admin: admin_grant
                    .as_ref()
                    .map(|grant| grant.authority.is_building_admin())
                    .unwrap_or(false),
                expires_at: admin_grant
                    .as_ref()
                    .map(|grant| (self.now_ms)().saturating_add(grant.ttl_ms))
                    .unwrap_or(0),
            },
        };
        let name = identity.name.clone();
        let session_admin = identity.admin;
        let expires_at = (identity.expires_at != 0).then_some(identity.expires_at);
        if identity.admin {
            self.store.save_admin_session(
                &t,
                &identity.name,
                identity.kind,
                identity.expires_at,
            )?;
        }
        let credential_id = match davet {
            Some(inv) => self.store.credential_id_for_secret(&inv.member),
            None => admin_grant.and_then(|grant| grant.credential_id),
        };
        if let (Some(principal_id), Some(credential_id)) =
            (identity.principal_id.as_deref(), credential_id.as_deref())
        {
            self.store.save_principal_session(
                &t,
                principal_id,
                credential_id,
                self.now(),
                identity.expires_at,
            )?;
        }
        self.sessions.lock_or_recover().insert(t.clone(), identity);
        Ok(protocol::SessionInfo {
            participant_id: format!("p_{id}"),
            session_token: t,
            name,
            admin: session_admin,
            expires_at,
        })
    }

    /// Resolve a session token to its bound identity. An expired session (admin
    /// TTL passed) resolves to nothing — and is dropped — so a stale token is
    /// as good as no token.
    pub fn session_identity(&self, token: Option<&str>) -> Option<SessionIdentity> {
        let token = token?;
        let mut sessions = self.sessions.lock_or_recover();
        let idy = sessions.get(token)?.clone();
        if idy.principal_id.is_some()
            && self.store.is_persistent()
            && !self.store.principal_session_active(token, (self.now_ms)())
        {
            sessions.remove(token);
            return None;
        }
        if idy.expires_at != 0 && idy.expires_at <= (self.now_ms)() {
            sessions.remove(token);
            if idy.admin {
                let _ = self.store.delete_admin_session(token);
            }
            return None;
        }
        Some(idy)
    }

    /// Does this request carry admin authority — either the raw admin/master
    /// key in the header, OR a live admin session token? This is the one place
    /// the two are treated as equal, so the browser can hold a short-lived
    /// session instead of the raw key (PRINCIPLES: the key never leaves .env).
    pub fn is_admin_session(&self, session_token: Option<&str>) -> bool {
        self.resolve_authority(None, session_token)
            .is_building_admin()
    }

    /// Revoke one browser/agent session immediately. Logout must end the
    /// server-side bearer credential, not merely forget it in the client.
    pub fn revoke_session(&self, token: &str) -> rusqlite::Result<bool> {
        let admin = self
            .sessions
            .lock_or_recover()
            .get(token)
            .is_some_and(|identity| identity.admin);
        if admin {
            self.store.delete_admin_session(token)?;
        }
        self.store.revoke_principal_session(token, self.now())?;
        Ok(self.sessions.lock_or_recover().remove(token).is_some())
    }

    /// Get or create a room, seeding new rooms with the hub's default settings.
    fn room_mut<'a>(
        rooms: &'a mut HashMap<String, Room>,
        name: &str,
        defaults: &RoomSettings,
    ) -> &'a mut Room {
        rooms
            .entry(name.to_string())
            .or_insert_with(|| Room::with(ChatMode::Free, defaults.clone(), 1))
    }

    /// A subscription to a room's live frames, creating the room if needed.
    /// Returns the receiver plus the current history to replay to the joiner.
    pub fn subscribe(&self, room: &str) -> (broadcast::Receiver<ServerFrame>, Vec<Message>) {
        // Subscribing to a deleted loca must NOT bring it back. A watch=1
        // listener touches subscribe before join, so this was the main way a
        // removed room reappeared. Give it a throwaway channel and empty
        // history instead of resurrecting the room.
        if self.is_deleted(room) {
            let (tx, rx) = broadcast::channel(1);
            drop(tx);
            return (rx, Vec::new());
        }
        let mut rooms = self.rooms.lock_or_recover();
        let r = Self::room_mut(&mut rooms, room, &self.default_settings);
        (r.tx.subscribe(), r.history.clone())
    }

    /// Register a member on connect and broadcast the updated roster.
    ///
    /// Agent runtimes are last-writer-wins: a fresh listener evicts a stale
    /// predecessor so a ghost cannot shadow delivery. Human web readers are
    /// different: the same identity+name may be open on multiple devices, all
    /// sharing one roster entry and one seat by reference count.
    /// A loca seats at most this many. The number is not arbitrary: a loca is
    /// by definition a small, private room — more than this and it is a salon,
    /// a different kind of place (DESIGN §5g). Watchers do not take a seat, so
    /// they are not counted here (they never call `join`).
    pub const LOCA_KAPASITE: usize = 7;

    /// Seat a member. Returns false when the loca is full — the caller must
    /// refuse the connection.
    ///
    /// One key = one seat: `identity` is the seat key (derived from the
    /// credential at the door), `name` is only the label the seat wears. The
    /// same identity reconnecting still occupies its old seat; only a NEW
    /// identity can overflow the loca. Agent reconnects and identity renames
    /// replace the previous holder. Same-name human readers coexist. This is
    /// what makes two operator screens ONE person instead of two seats.
    #[must_use]
    pub fn join(
        &self,
        room: &str,
        identity: &str,
        name: &str,
        kind: SenderType,
        session: u64,
    ) -> bool {
        // A join to a deleted loca does not resurrect it — the master must open
        // it again first (which clears the tombstone). Refuse the seat.
        if self.is_deleted(room) {
            return false;
        }
        let mut rooms = self.rooms.lock_or_recover();
        let r = Self::room_mut(&mut rooms, room, &self.default_settings);
        // Capacity counts identities, not names: an identity rejoining under
        // any name evicts itself, never overflows.
        if !r.members.contains_key(identity) && r.members.len() >= Self::LOCA_KAPASITE {
            return false;
        }
        let additional_human_reader =
            r.members
                .get(identity)
                .is_some_and(|(existing_name, existing_kind, _)| {
                    existing_name == name
                        && *existing_kind == SenderType::User
                        && kind == SenderType::User
                });
        if additional_human_reader {
            // A laptop and a phone are two readers of the same human seat.
            // Keep both sockets alive; leave() removes the roster entry only
            // after the final reader disconnects.
            if let Some(entry) = r.members.get_mut(identity) {
                entry.2 = entry.2.saturating_add(1);
            }
        } else if r.members.contains_key(identity) {
            // Tell older connections on this identity to close; `session` is
            // the new agent/renamed holder, so it recognises the frame as its
            // own and stays. Other human readers never take this path.
            // The frame carries the identity because the old connection may be
            // sitting under a DIFFERENT display name — name alone would miss it.
            let _ = r.tx.send(ServerFrame::Evicted {
                name: name.to_string(),
                identity: identity.to_string(),
                session,
            });
            // Take the connection count back to exactly one: the evicted
            // session is gone whether or not its reader ever runs leave(). A
            // dead reader never reaches leave(), so accumulating here would
            // leak the count upward and stick the seat in the roster forever
            // (the ghost the operator kept seeing). Reset, don't accumulate.
            if let Some(entry) = r.members.get_mut(identity) {
                entry.0 = name.to_string();
                entry.1 = kind;
                entry.2 = 1;
            }
        } else {
            r.members
                .insert(identity.to_string(), (name.to_string(), kind, 1));
        }
        let members = r.member_list();
        let _ = r.tx.send(ServerFrame::Members { members });
        true
    }

    /// Deregister a seat on disconnect; frees it at zero connections.
    pub fn leave(&self, room: &str, identity: &str) {
        let mut rooms = self.rooms.lock_or_recover();
        let Some(r) = rooms.get_mut(room) else { return };
        if let Some(entry) = r.members.get_mut(identity) {
            entry.2 = entry.2.saturating_sub(1);
            if entry.2 == 0 {
                r.members.remove(identity);
            }
        }
        let members = r.member_list();
        let _ = r.tx.send(ServerFrame::Members { members });
    }

    /// Append a posted message, trim history, and broadcast it. The current
    /// chat mode is enforced here: `is_admin` bypasses gating (so the admin can
    /// always speak, e.g. while paused). Returns `Err(PostReject)` if the mode
    /// forbids this sender.
    pub fn post(
        &self,
        room: &str,
        body: PostMessage,
        is_admin: bool,
        principal: &str,
    ) -> Result<Message, PostReject> {
        // A sealed (deleted) loca does not come back on a post. Without this,
        // `room_mut` (get-or-create) would resurrect a tombstoned room the
        // moment anyone with a stale davet posted to it — the room the master
        // shut would silently reopen. The master reopens it deliberately (which
        // clears the tombstone), never a stray REST call.
        if self.is_deleted(room) {
            return Err(PostReject::Deleted);
        }
        let op_id = body.op_id.filter(|id| !id.is_empty());
        if let Some(id) = op_id.as_deref() {
            match self.store.message_by_operation(room, principal, id) {
                Ok(Some(message)) => return Ok(message),
                Ok(None) => {}
                Err(_) => return Err(PostReject::Storage),
            }
        }
        let now = (self.now_ms)();
        let mut rooms = self.rooms.lock_or_recover();
        let home_tx = (room != self.home_room.as_str())
            .then(|| {
                rooms
                    .get(self.home_room.as_str())
                    .map(|home| home.tx.clone())
            })
            .flatten();
        let r = Self::room_mut(&mut rooms, room, &self.default_settings);
        if let Some(id) = op_id.as_deref() {
            let key = (principal.to_string(), id.to_string());
            if let Some(message) = r.operations.get(&key) {
                return Ok(message.clone());
            }
        }

        // Archived rooms are read-only for everyone, admin included: closing a
        // room is a deliberate state, not a moderation nudge.
        if r.settings.archived {
            return Err(PostReject::Archived);
        }

        // ---- per-participant moderation (admin bypasses) ----
        if !is_admin {
            if r.banned.contains(&body.sender) {
                return Err(PostReject::Banned);
            }
            if r.muted.contains(&body.sender) {
                return Err(PostReject::Muted);
            }
        }

        // ---- mode enforcement (admin bypasses) ----
        if !is_admin {
            match &r.mode {
                ChatMode::Free => {}
                ChatMode::Paused => return Err(PostReject::Paused),
                ChatMode::Restricted { allow } => {
                    if !allow.contains(&body.sender) {
                        return Err(PostReject::NotAllowed);
                    }
                }
                ChatMode::RoundRobin { order, turn } => match order.get(*turn) {
                    Some(cur) if cur == &body.sender => {}
                    _ => {
                        return Err(PostReject::NotYourTurn {
                            whose: order.get(*turn).cloned(),
                        })
                    }
                },
            }
        }

        // ---- rate limit (sliding window; admin bypasses; 0 = disabled) ----
        let record_rate = !is_admin && r.settings.rate_limit > 0;
        if record_rate {
            let window_ms = r.settings.rate_window_secs as u64 * 1000;
            let cutoff = now.saturating_sub(window_ms);
            let times = r.post_times.entry(body.sender.clone()).or_default();
            while times.front().is_some_and(|&t| t <= cutoff) {
                times.pop_front();
            }
            if times.len() as u32 >= r.settings.rate_limit {
                let retry_ms = times
                    .front()
                    .map(|&t| (t + window_ms).saturating_sub(now))
                    .unwrap_or(window_ms);
                return Err(PostReject::RateLimited {
                    retry_after_secs: retry_ms.div_ceil(1000),
                });
            }
        }

        let msg = Message {
            kind: body.kind,
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            room: room.to_string(),
            sender: body.sender,
            sender_type: body.sender_type,
            target: body.target.filter(|t| !t.is_empty()),
            text: body.text,
            reply_to: body.reply_to,
            ts: self.next_condition_generation(now, r.last_msg_ms),
        };
        let caretaker_summons: Vec<CareSignal> = if room == self.home_room.as_str() {
            Vec::new()
        } else {
            self.addressed_caretakers(&msg)
                .into_iter()
                .map(|owner| CareSignal {
                    id: format!("summon:{room}:{}:{owner}", msg.id),
                    attention_id: format!("attention:{room}:summon:{}:{owner}", msg.id),
                    // Re-home the envelope onto the caretaker's home loca (where
                    // it is delivered) so signal.room matches the receiving
                    // socket. The true origin travels in source_room; the
                    // attention_id and the ledger stay keyed to the source loca.
                    room: self.home_room.as_ref().clone(),
                    source_room: room.to_string(),
                    reason: CareReason::DirectSummon,
                    audience: AttentionAudience::Person {
                        name: owner.clone(),
                    },
                    owner: Some(owner.clone()),
                    target: Some(owner.clone()),
                    participants: vec![msg.sender.clone(), owner],
                    subject: format!("{} directly summoned a caretaker", msg.sender),
                    created_by: msg.sender.clone(),
                    context: vec![msg.clone()],
                    attempt: 1,
                    at: now,
                    escalated: false,
                    state: protocol::ReminderState::Running,
                })
                .collect()
        };

        // P0#1: an explicit direct reply re-wakes exactly the waiter it
        // answers. The trigger is deliberately strict and structured — no
        // progress inference: an ordinary turn (`Say`) whose `target` names a
        // participant who is *currently waiting for this very sender*. `target`
        // must be that structured address, never `None`, never the "all"
        // broadcast, and we do NOT parse a free-text `@mention`. The owner is
        // always the waiter, so the sender can never wake itself, and only the
        // addressed waiter is touched (other waiters-on-sender are left alone).
        // Kept mutually exclusive with a caretaker summon: when the same turn
        // already summons the addressed caretaker, that summon does the waking.
        let wait_wake: Option<(WaitState, CareSignal)> = (msg.kind == protocol::MessageKind::Say
            && caretaker_summons.is_empty())
        .then(|| {
            let target = msg.target.as_deref()?;
            if target.eq_ignore_ascii_case("all") || target == msg.sender {
                return None;
            }
            let existing = r.waits.get(target)?;
            if existing.waiting_for != msg.sender {
                return None;
            }
            let new_since = self.next_condition_generation(now, existing.since);
            let updated = WaitState {
                room: room.to_string(),
                waiter: target.to_string(),
                waiting_for: existing.waiting_for.clone(),
                reason: existing.reason.clone(),
                since: new_since,
                last_signal_at: None,
                signal_count: 0,
            };
            let wake = CareSignal {
                id: Self::secure_token("delivery_", 16),
                attention_id: format!("attention:{room}:wait-reply:{target}:{new_since}"),
                room: room.to_string(),
                // Same-room wake: origin == delivery room, so source_room stays empty.
                source_room: String::new(),
                reason: CareReason::WaitReplied,
                audience: AttentionAudience::Person {
                    name: target.to_string(),
                },
                owner: Some(target.to_string()),
                target: Some(target.to_string()),
                participants: vec![msg.sender.clone(), target.to_string()],
                subject: format!("{} replied to {}", msg.sender, target),
                created_by: msg.sender.clone(),
                context: vec![msg.clone()],
                attempt: 1,
                at: now,
                escalated: false,
                state: protocol::ReminderState::Running,
            };
            Some((updated, wake))
        })
        .flatten();

        // (Lead is no longer set by parsing chat. "Konuşmak yan etki üretmez":
        // naming a lead is an explicit act — POST /rooms/:id/lead — not a
        // message the server secretly interprets. See `set_lead`.)

        // Advance the round-robin turn on an accepted non-admin (or admin) post
        // by the current speaker, so the baton actually moves.
        let mut previous_turn = None;
        if let ChatMode::RoundRobin { order, turn } = &mut r.mode {
            if !order.is_empty() && order.get(*turn) == Some(&msg.sender) {
                previous_turn = Some(*turn);
                *turn = (*turn + 1) % order.len();
            }
        }

        // Persist BEFORE broadcast: a message is not "sent" until it is
        // durable. If the write fails we refuse rather than broadcast a line
        // that a restart would forget (PRINCIPLES: "mesaj kaybolmaz" — a
        // successful reply means the word landed). Memory-only mode returns Ok.
        let operation = op_id.as_deref().map(|id| (principal, id));
        let persisted = if let Some((updated_wait, wake)) = wait_wake.as_ref() {
            // The waiter is a member of this room, so the wake is delivered
            // here (no home-room hop). Message + overdue suppression + wait
            // generation advance + durable wake all commit as one.
            self.store.insert_message_with_wait_wake(
                &msg,
                previous_turn.is_some().then_some((&r.mode, &r.settings)),
                operation,
                room,
                &updated_wait.waiter,
                now,
                updated_wait,
                wake,
            )
        } else if !caretaker_summons.is_empty() {
            self.store.insert_message_with_care(
                &msg,
                previous_turn.is_some().then_some((&r.mode, &r.settings)),
                operation,
                self.home_room.as_str(),
                &caretaker_summons,
            )
        } else if previous_turn.is_some() {
            self.store
                .insert_message_with_room(&msg, &r.mode, &r.settings, operation)
        } else {
            self.store.insert_message(&msg, operation)
        };
        if persisted.is_err() {
            if let (Some(old), ChatMode::RoundRobin { turn, .. }) = (previous_turn, &mut r.mode) {
                *turn = old;
            }
            return Err(PostReject::Storage);
        }
        if record_rate {
            r.post_times
                .entry(msg.sender.clone())
                .or_default()
                .push_back(now);
        }
        if let Some(id) = op_id.as_deref() {
            let key = (principal.to_string(), id.to_string());
            r.operations.insert(key.clone(), msg.clone());
            r.operation_order.push_back(key);
            while r.operation_order.len() > HISTORY_LIMIT {
                if let Some(oldest) = r.operation_order.pop_front() {
                    r.operations.remove(&oldest);
                }
            }
        }
        if previous_turn.is_some() {
            let _ = r.tx.send(ServerFrame::Mode {
                mode: r.mode.clone(),
            });
        }
        // An accepted message is explicit progress for room-silence care.
        // Reset the durable generation in the same transaction as the message
        // (Store), then mirror and broadcast that lifecycle only after commit.
        r.care_marks.remove("silence");
        Self::resolve_condition_attention(r, room, "silence", msg.ts);
        r.last_msg_ms = msg.ts;
        r.history.push(msg.clone());
        // Hot/audit split: memory keeps the tail (context), the DB keeps
        // EVERYTHING (the room's memory — searchable history is direction 3).
        if r.history.len() > HISTORY_LIMIT {
            let overflow = r.history.len() - HISTORY_LIMIT;
            r.history.drain(0..overflow);
        }
        let _ = r.tx.send(ServerFrame::Msg {
            message: msg.clone(),
        });
        // A caretaker sits only in the private maintenance loca, but an exact
        // cross-loca call must survive the caretaker being offline. Persist a
        // one-message, privacy-bounded care envelope in the existing ACKed
        // outbox. Copying the source message into Iye chat history would leak
        // room history; a best-effort broadcast used to lose the call.
        if room != self.home_room.as_str() {
            for signal in caretaker_summons {
                let attention = self
                    .store
                    .attention(&signal.attention_id)
                    .unwrap_or_else(|| Attention {
                        id: signal.attention_id.clone(),
                        // The ledger is filed under the source loca (origin),
                        // even though the envelope is delivered in the home loca.
                        room: signal.origin_room().to_string(),
                        reason: signal.reason,
                        subject: signal.subject.clone(),
                        audience: signal.audience.clone(),
                        owner: signal.owner.clone(),
                        participants: signal.participants.clone(),
                        created_by: signal.created_by.clone(),
                        created_at: signal.at,
                        attempt: signal.attempt,
                        escalated: signal.escalated,
                        status: AttentionStatus::Open,
                        delivered_at: None,
                        claimed_by: None,
                        claimed_at: None,
                        resolved_at: None,
                    });
                r.attentions.insert(attention.id.clone(), attention.clone());
                let _ = r.tx.send(ServerFrame::Attention { attention });
                self.care_deliveries
                    .lock_or_recover()
                    .insert(signal.id.clone(), signal.attention_id.clone());
                if let Some(home_tx) = home_tx.as_ref() {
                    let _ = home_tx.send(ServerFrame::Care { signal });
                }
            }
        }
        // Mirror the durable wake into memory and broadcast it (twin of the
        // caretaker block above). The waiter, live, gets the Care immediately;
        // offline, `pending_care` replays the same delivery id on reconnect and
        // `replayed_care_ids` dedups it. The wait row stays — only its stale
        // overdue is retired — and the reply's own generation bump resets age.
        if let Some((updated_wait, wake)) = wait_wake {
            let waiter = updated_wait.waiter.clone();
            r.waits.insert(waiter.clone(), updated_wait.clone());
            Self::resolve_wait_attentions(r, room, &waiter, now);
            let attention = self
                .store
                .attention(&wake.attention_id)
                .unwrap_or_else(|| Attention {
                    id: wake.attention_id.clone(),
                    room: wake.room.clone(),
                    reason: wake.reason,
                    subject: wake.subject.clone(),
                    audience: wake.audience.clone(),
                    owner: wake.owner.clone(),
                    participants: wake.participants.clone(),
                    created_by: wake.created_by.clone(),
                    created_at: wake.at,
                    attempt: wake.attempt,
                    escalated: wake.escalated,
                    status: AttentionStatus::Open,
                    delivered_at: None,
                    claimed_by: None,
                    claimed_at: None,
                    resolved_at: None,
                });
            r.attentions.insert(attention.id.clone(), attention.clone());
            self.care_deliveries
                .lock_or_recover()
                .insert(wake.id.clone(), wake.attention_id.clone());
            let _ = r.tx.send(ServerFrame::Care { signal: wake });
            let _ = r.tx.send(ServerFrame::Attention { attention });
            let _ = r.tx.send(ServerFrame::Wait {
                waiter,
                wait: Some(updated_wait),
            });
        }
        Ok(msg)
    }

    pub fn reactions(&self, room: &str) -> Vec<protocol::MessageReaction> {
        self.store.message_reactions(room).unwrap_or_default()
    }

    pub fn set_reaction(
        &self,
        room: &str,
        message_id: u64,
        principal: &str,
        reactor: &str,
        emoji: &str,
        active: bool,
    ) -> Result<protocol::MessageReactionEvent, ReactionReject> {
        const ALLOWED: [&str; 4] = ["✓", "✦", "!", "♥"];
        if !ALLOWED.contains(&emoji) {
            return Err(ReactionReject::InvalidEmoji);
        }
        if !self.is_writable(room) {
            return Err(ReactionReject::ReadOnly);
        }
        let owner = self
            .store
            .message_owner(room, message_id)
            .map_err(|_| ReactionReject::Storage)?
            .or_else(|| {
                self.rooms
                    .lock_or_recover()
                    .get(room)
                    .and_then(|r| r.history.iter().find(|m| m.id == message_id))
                    .map(|m| m.sender.clone())
            })
            .ok_or(ReactionReject::NotFound)?;
        if owner == reactor {
            return Err(ReactionReject::OwnMessage);
        }
        let at = (self.now_ms)();
        let reactions = self
            .store
            .set_message_reaction(room, message_id, principal, reactor, emoji, active, at)
            .map_err(|_| ReactionReject::Storage)?;
        let actors = reactions
            .into_iter()
            .find(|r| r.message_id == message_id && r.emoji == emoji)
            .map(|r| r.actors)
            .unwrap_or_else(|| {
                if active {
                    vec![reactor.to_string()]
                } else {
                    Vec::new()
                }
            });
        let event = protocol::MessageReactionEvent {
            message_id,
            emoji: emoji.to_string(),
            actors,
            owner,
            reactor: reactor.to_string(),
            active,
            ts: at,
        };
        if let Some(r) = self.rooms.lock_or_recover().get(room) {
            let _ = r.tx.send(ServerFrame::Reaction {
                reaction: event.clone(),
            });
        }
        Ok(event)
    }

    /// Name (or clear) this loca's lead — an explicit operator action, not a
    /// chat side effect. Mutates settings, persists, broadcasts the new
    /// settings, AND emits an announcement so everyone in the loca learns of it
    /// "in the same breath" (PRINCIPLES: the action is explicit; the room sees
    /// its result). Naming targets the announcement at the new lead: ordinary
    /// clients still see the public announcement, while a mentions-only agent
    /// receives an immediate direct wake. Ending the title remains an untargeted
    /// room announcement. Returns the settings after the change.
    pub fn set_lead(
        &self,
        room: &str,
        lead: Option<String>,
        by: &str,
    ) -> Result<RoomSettings, LeadError> {
        if self.is_deleted(room) {
            return Ok(self.settings(room));
        }
        let mut rooms = self.rooms.lock_or_recover();
        let (mode, mut settings) = rooms
            .get(room)
            .map(|r| (r.mode.clone(), r.settings.clone()))
            .unwrap_or_else(|| (ChatMode::Free, self.default_settings.clone()));
        if lead.is_none()
            && rooms.get(room).is_some_and(|room| {
                room.goals
                    .values()
                    .any(|goal| goal.status == GoalStatus::Active)
            })
        {
            return Err(LeadError::ActiveGoal);
        }
        settings.lead = lead.clone();
        let text = match &lead {
            Some(name) => format!("{by} set @lead {name}."),
            None => format!("{by} set @lead none."),
        };
        let msg = Message {
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            room: room.to_string(),
            sender: "loca".into(),
            sender_type: SenderType::User,
            // Announcements do not normally wake filtered agents. A lead
            // assignment is also a direct summons to the person who must now
            // watch the whole room, so address that one announcement to them.
            target: lead.clone().or_else(|| Some("all".into())),
            text,
            reply_to: None,
            kind: protocol::MessageKind::Announce,
            ts: (self.now_ms)(),
        };
        self.store
            .insert_message_with_room(&msg, &mode, &settings, None)
            .map_err(|_| LeadError::Storage)?;
        let r = Self::room_mut(&mut rooms, room, &self.default_settings);
        r.settings = settings.clone();
        r.history.push(msg.clone());
        let _ = r.tx.send(ServerFrame::Settings {
            settings: settings.clone(),
        });
        let _ = r.tx.send(ServerFrame::Msg { message: msg });
        Ok(settings)
    }

    /// Set the chat mode (admin action) and broadcast it. Normalizes an
    /// out-of-range round-robin `turn` back to 0.
    pub fn set_mode(&self, room: &str, mut mode: ChatMode) -> rusqlite::Result<ChatMode> {
        // A sealed loca is not reopened by a mode change — only a deliberate
        // reopen clears the tombstone (see `post`/`join`/`subscribe`).
        if self.is_deleted(room) {
            return Ok(self.mode(room));
        }
        if let ChatMode::RoundRobin { order, turn } = &mut mode {
            if order.is_empty() || *turn >= order.len() {
                *turn = 0;
            }
        }
        let mut rooms = self.rooms.lock_or_recover();
        let r = Self::room_mut(&mut rooms, room, &self.default_settings);
        self.store.save_room(room, &mode, &r.settings)?;
        r.mode = mode.clone();
        let _ = r.tx.send(ServerFrame::Mode { mode: mode.clone() });
        Ok(mode)
    }

    pub fn mode(&self, room: &str) -> ChatMode {
        let rooms = self.rooms.lock_or_recover();
        rooms.get(room).map(|r| r.mode.clone()).unwrap_or_default()
    }

    /// Whether the room is in "live" (WhatsApp) mode right now.
    pub fn is_live(&self, room: &str) -> bool {
        let rooms = self.rooms.lock_or_recover();
        rooms.get(room).map(|r| r.settings.live).unwrap_or(false)
    }

    /// Whether `name` currently wears this loca's lead title.
    ///
    /// A mentions-filtered lead still receives the whole room: seeing the
    /// conversation is the role, not a temporary live-mode side effect.
    pub fn is_lead(&self, room: &str, name: &str) -> bool {
        let rooms = self.rooms.lock_or_recover();
        rooms.get(room).and_then(|r| r.settings.lead.as_deref()) == Some(name)
    }

    /// Current admin-tunable settings for a room.
    pub fn settings(&self, room: &str) -> RoomSettings {
        let rooms = self.rooms.lock_or_recover();
        rooms
            .get(room)
            .map(|r| r.settings.clone())
            .unwrap_or_else(|| self.default_settings.clone())
    }

    /// Update settings (admin action), broadcast, and persist.
    #[allow(clippy::too_many_arguments)]
    pub fn set_settings(
        &self,
        room: &str,
        rate_limit: Option<u32>,
        window: Option<u32>,
        live: Option<bool>,
        archived: Option<bool>,
        live_timeout: Option<u32>,
        operators: Option<Vec<String>>,
        turn_max_messages: Option<u32>,
        turn_idle_ms: Option<u32>,
        turn_max_wait_ms: Option<u32>,
        care_wait_secs: Option<u32>,
        care_cooldown_secs: Option<u32>,
        care_max_attempts: Option<u32>,
        care_context_messages: Option<u32>,
        care_recipient: Option<ReminderRecipient>,
        care_goal_secs: Option<u32>,
        care_task_secs: Option<u32>,
        care_silence_secs: Option<u32>,
    ) -> rusqlite::Result<RoomSettings> {
        // A sealed loca does not come back on a settings change.
        if self.is_deleted(room) {
            return Ok(self.settings(room));
        }
        let mut rooms = self.rooms.lock_or_recover();
        let r = Self::room_mut(&mut rooms, room, &self.default_settings);
        let mut s = r.settings.clone();
        let was_archived = r.settings.archived;
        if let Some(rl) = rate_limit {
            s.rate_limit = rl;
        }
        if let Some(w) = window {
            s.rate_window_secs = w.max(1);
        }
        if let Some(l) = live {
            s.live = l;
        }
        if let Some(a) = archived {
            s.archived = a;
        }
        if let Some(t) = live_timeout {
            s.live_timeout_secs = t;
        }
        if let Some(ops) = operators {
            s.operators = ops;
        }
        if let Some(max) = turn_max_messages {
            s.turn_max_messages = max.clamp(1, 16);
        }
        if let Some(idle) = turn_idle_ms {
            s.turn_idle_ms = idle.clamp(100, 30_000);
        }
        if let Some(max_wait) = turn_max_wait_ms {
            s.turn_max_wait_ms = max_wait.clamp(100, 60_000);
        }
        // A hard maximum below the quiet window would make the advertised
        // quiet-window contract impossible. Keep the pair internally valid.
        s.turn_max_wait_ms = s.turn_max_wait_ms.max(s.turn_idle_ms);
        if let Some(value) = care_wait_secs {
            s.care_wait_secs = value;
        }
        if let Some(value) = care_cooldown_secs {
            s.care_cooldown_secs = value;
        }
        if let Some(value) = care_max_attempts {
            s.care_max_attempts = value.clamp(1, 10);
        }
        if let Some(value) = care_context_messages {
            s.care_context_messages = value.clamp(0, 20);
        }
        if let Some(value) = care_recipient {
            s.care_recipient = value;
        }
        if let Some(value) = care_goal_secs {
            s.care_goal_secs = value;
        }
        if let Some(value) = care_task_secs {
            s.care_task_secs = value;
        }
        if let Some(value) = care_silence_secs {
            s.care_silence_secs = value;
        }
        self.store.save_room(room, &r.mode, &s)?;
        if !was_archived && s.archived {
            r.care_marks.clear();
        }
        if live == Some(true) {
            r.last_msg_ms = (self.now_ms)();
        }
        r.settings = s.clone();
        let _ = r.tx.send(ServerFrame::Settings {
            settings: s.clone(),
        });
        Ok(s)
    }

    /// Broadcast a control frame (e.g. operator `/stop`) to a room.
    pub fn control(&self, room: &str, cmd: &str) {
        let rooms = self.rooms.lock_or_recover();
        if let Some(r) = rooms.get(room) {
            let _ = r.tx.send(ServerFrame::Control {
                cmd: cmd.to_string(),
            });
        }
    }

    /// Rebroadcast an ephemeral typing signal (not persisted).
    pub fn typing(&self, room: &str, name: &str, on: bool) {
        let rooms = self.rooms.lock_or_recover();
        if let Some(r) = rooms.get(room) {
            let _ = r.tx.send(ServerFrame::Typing {
                name: name.to_string(),
                on,
            });
        }
    }

    // ---- per-participant moderation (admin-only, enforced at post/join) ----

    /// Whether `name` is banned from `room`.
    pub fn is_banned(&self, room: &str, name: &str) -> bool {
        let rooms = self.rooms.lock_or_recover();
        rooms
            .get(room)
            .map(|r| r.banned.contains(name))
            .unwrap_or(false)
    }

    pub fn mod_state(&self, room: &str) -> protocol::ModState {
        let rooms = self.rooms.lock_or_recover();
        match rooms.get(room) {
            Some(r) => {
                let mut muted: Vec<String> = r.muted.iter().cloned().collect();
                let mut banned: Vec<String> = r.banned.iter().cloned().collect();
                muted.sort();
                banned.sort();
                protocol::ModState { muted, banned }
            }
            None => protocol::ModState::default(),
        }
    }

    /// Apply a moderation action. Returns the updated mod state. For kick/ban a
    /// `Kicked` frame is broadcast so the targeted client closes its socket.
    pub fn moderate(
        &self,
        room: &str,
        action: protocol::ModAction,
        name: &str,
    ) -> rusqlite::Result<protocol::ModState> {
        use protocol::ModAction::*;
        // A sealed loca is not reopened by a moderation action.
        if self.is_deleted(room) {
            return Ok(self.mod_state(room));
        }
        match action {
            Mute => self.store.set_ban(room, name, "mute", (self.now_ms)())?,
            Unmute => self.store.clear_ban(room, name, "mute")?,
            Ban => {
                self.store.set_ban(room, name, "ban", (self.now_ms)())?;
                self.revoke_invites_for(room, name)?;
            }
            Unban => self.store.clear_ban(room, name, "ban")?,
            Kick | Release => self.revoke_invites_for(room, name)?,
        }
        let mut rooms = self.rooms.lock_or_recover();
        let r = Self::room_mut(&mut rooms, room, &self.default_settings);
        match action {
            Mute => {
                r.muted.insert(name.to_string());
            }
            Unmute => {
                r.muted.remove(name);
            }
            Kick => {
                // PRINCIPLES: "çıkar — bağlantı kapanır, DAVETİ DURUR." Kick is
                // not just a disconnect: it ends the davet, otherwise the same
                // token walks right back in. Take the seat AND the davet.
                let _ = r.tx.send(ServerFrame::Kicked {
                    name: name.to_string(),
                    banned: false,
                });
                // Moderation targets a NAME (that is what the operator sees);
                // the seat map is identity-keyed, so drop whichever seat wears
                // that label.
                r.members.retain(|_id, (n, _, _)| n != name);
                let members = r.member_list();
                let _ = r.tx.send(ServerFrame::Members { members });
                drop(rooms);
                return Ok(self.mod_state(room));
            }
            Ban => {
                r.banned.insert(name.to_string());
                r.members.retain(|_id, (n, _, _)| n != name); // ghost or live, they are out
                let _ = r.tx.send(ServerFrame::Kicked {
                    name: name.to_string(),
                    banned: true,
                });
                // Broadcast the new roster, exactly as Kick does. Without this
                // the banned name lingered in every client's list until the
                // next unrelated join/leave — the asymmetry the operator hit.
                let members = r.member_list();
                let _ = r.tx.send(ServerFrame::Members { members });
                // The door shuts, reading included — persist so a restart does
                // not quietly reopen it (PRINCIPLES: "restart odayı öldürmez").
                drop(rooms);
                return Ok(self.mod_state(room));
            }
            Unban => {
                r.banned.remove(name);
            }
            Release => {
                // Take back the seat, not the belonging. Their davets for THIS
                // loca end and the connection closes; membership of the
                // building is untouched, so the next call-in is one click.
                // Drop the roster entry directly (Kick/Ban do) so a dead socket
                // leaves no ghost — the whole point of "işi bitti."
                let _ = r.tx.send(ServerFrame::Kicked {
                    name: name.to_string(),
                    banned: false,
                });
                r.members.retain(|_id, (n, _, _)| n != name);
                let members = r.member_list();
                let _ = r.tx.send(ServerFrame::Members { members });
                drop(rooms);
                return Ok(self.mod_state(room));
            }
        }
        let mut muted: Vec<String> = r.muted.iter().cloned().collect();
        let mut banned: Vec<String> = r.banned.iter().cloned().collect();
        muted.sort();
        banned.sort();
        let state = protocol::ModState { muted, banned };
        let _ = r.tx.send(ServerFrame::Mod {
            state: state.clone(),
        });
        Ok(state)
    }

    /// Seal a room for good: tell everyone and drop it from live memory while
    /// preserving its record. Connected clients get a `room-closed` control,
    /// then their
    /// broadcast channel closes (which ends their sessions). Returns whether
    /// the room existed. The configured home loca may be closed too — it
    /// simply respawns empty on the next join.
    pub fn delete_room(&self, room: &str) -> Result<bool, DeleteReject> {
        {
            let rooms = self.rooms.lock_or_recover();
            match rooms.get(room) {
                Some(r) if !r.settings.archived => return Err(DeleteReject::NotArchived),
                None => return Ok(false),
                _ => {}
            }
        }
        self.store
            .seal_room(room, (self.now_ms)())
            .map_err(|_| DeleteReject::Storage)?;
        let removed = self.rooms.lock_or_recover().remove(room);
        if let Some(r) = removed {
            let _ = r.tx.send(ServerFrame::Control {
                cmd: "room-closed".into(),
            });
        }
        self.deleted.lock_or_recover().insert(room.to_string());
        Ok(true)
    }

    /// Is this loca deleted (tombstoned)? A subscribe/join to it must not
    /// re-create it. The configured home loca is never tombstoned.
    pub fn is_deleted(&self, room: &str) -> bool {
        room != self.home_room.as_str() && self.deleted.lock_or_recover().contains(room)
    }

    /// Is this loca archived (closed, read-only)? Archived is a deliberate
    /// state: the room is kept but nothing new may be written to it.
    pub fn is_archived(&self, room: &str) -> bool {
        self.rooms
            .lock_or_recover()
            .get(room)
            .map(|r| r.settings.archived)
            .unwrap_or(false)
    }

    /// May a domain mutation land in this loca? A sealed (deleted) or archived
    /// loca is read-only — no message, note, task, journal or moderation write.
    /// PRINCIPLES: "archived = read-only, hiçbir domain mutation." One predicate
    /// so every write path asks the same question instead of each guessing.
    pub fn is_writable(&self, room: &str) -> bool {
        !self.is_deleted(room) && !self.is_archived(room)
    }

    /// The master opening a loca again clears its tombstone, so the next join
    /// creates it fresh.
    pub fn revive(&self, room: &str) {
        self.deleted.lock_or_recover().remove(room);
    }

    /// Turn live mode off in any room that has been silent past its timeout.
    /// Called periodically by a background task so a forgotten live room can't
    /// keep waking every agent.
    pub fn expire_live(&self) {
        let now = (self.now_ms)();
        let mut rooms = self.rooms.lock_or_recover();
        for (name, r) in rooms.iter_mut() {
            let t = r.settings.live_timeout_secs as u64;
            if r.settings.live
                && t > 0
                && r.last_msg_ms > 0
                && now.saturating_sub(r.last_msg_ms) > t * 1000
            {
                r.settings.live = false;
                let s = r.settings.clone();
                let _ = r.tx.send(ServerFrame::Settings {
                    settings: s.clone(),
                });
                let _ = r.tx.send(ServerFrame::Control {
                    cmd: format!("live-expired:{}", t),
                });
                if let Err(error) = self.store.save_room(name, &r.mode, &s) {
                    r.settings.live = true;
                    tracing::error!(room = %name, %error, "could not persist live expiry");
                } else {
                    tracing::info!(room = %name, "live mode auto-disabled after {}s idle", t);
                }
            }
        }
    }

    pub fn loca_operator(&self, room: &str) -> Option<LocaOperatorAssignment> {
        self.store.loca_operator(room)
    }

    pub fn loca_operator_history(&self, room: &str) -> Vec<LocaOperatorAssignment> {
        self.store.loca_operator_history(room)
    }

    pub fn master_principal(&self) -> Option<PrincipalIdentity> {
        self.store.active_master_principal()
    }

    pub fn profiles(&self) -> Vec<PrincipalIdentity> {
        self.store.active_principals()
    }

    pub fn appoint_loca_operator(
        &self,
        room: &str,
        target_principal_id: &str,
        actor: &RequestAuthority,
    ) -> Result<LocaOperatorAssignment, OperatorAssignmentError> {
        if !actor.is_building_admin() {
            return Err(OperatorAssignmentError::AuthorityRequired);
        }
        let actor_id = actor
            .principal_id
            .as_deref()
            .ok_or(OperatorAssignmentError::AuthorityRequired)?;
        self.store
            .appoint_loca_operator(room, target_principal_id, actor_id, self.now())
            .map_err(map_loca_operator_error)
    }

    pub fn revoke_loca_operator(
        &self,
        room: &str,
        actor: &RequestAuthority,
    ) -> Result<LocaOperatorAssignment, OperatorAssignmentError> {
        if !actor.is_building_admin() {
            return Err(OperatorAssignmentError::AuthorityRequired);
        }
        let actor_id = actor
            .principal_id
            .as_deref()
            .ok_or(OperatorAssignmentError::AuthorityRequired)?;
        let current = self
            .store
            .loca_operator(room)
            .ok_or(OperatorAssignmentError::NotFound)?;
        if actor.building_role == Some(BuildingRole::Smaster)
            && (current.appointed_by_role != BuildingRole::Smaster
                || current.appointed_by_principal_id != actor_id)
        {
            return Err(OperatorAssignmentError::MasterProtected);
        }
        self.store
            .revoke_loca_operator(room, &current.principal_id, self.now())
            .map_err(map_loca_operator_error)
    }

    /// Operator authority in one loca: Building authority everywhere, or the
    /// principal explicitly appointed to this loca. In-memory/dev stores keep
    /// the old name list for compatibility; persistent stores migrate it at
    /// boot and never grant authority from a display label.
    pub fn is_loca_operator(
        &self,
        room: &str,
        admin_token: Option<&str>,
        session_token: Option<&str>,
        name: &str,
    ) -> bool {
        let authority = self.resolve_authority(admin_token, session_token);
        if authority.is_building_admin() {
            return true;
        }
        if self.admin_token.is_empty() {
            return true; // dev-open, consistent with the other admin gates
        }
        if let (Some(principal_id), Some(assignment)) = (
            authority.principal_id.as_deref(),
            self.store.loca_operator(room),
        ) {
            return assignment.principal_id == principal_id;
        }
        if self.store.is_persistent() {
            return false;
        }
        let rooms = self.rooms.lock_or_recover();
        rooms
            .get(room)
            .map(|r| r.settings.operators.iter().any(|o| o == name))
            .unwrap_or(false)
    }

    // ---- tasks: declared work (the guest object) ----

    /// Everything recorded in this loca's journal, oldest first.
    pub fn journal(&self, room: &str) -> Vec<protocol::JournalEntry> {
        let rooms = self.rooms.lock_or_recover();
        rooms
            .get(room)
            .map(|r| r.journal.clone())
            .unwrap_or_default()
    }

    /// Record a piece of finished work. Nobody assigns this and nothing closes
    /// it — the writer is simply saying what they did, and it stands.
    pub fn append_journal(
        &self,
        room: &str,
        by: String,
        by_type: SenderType,
        text: String,
    ) -> rusqlite::Result<protocol::JournalEntry> {
        let now = (self.now_ms)();
        let mut rooms = self.rooms.lock_or_recover();
        let id = rooms.get(room).map(|r| r.next_journal_id).unwrap_or(1);
        let entry = protocol::JournalEntry {
            id,
            room: room.to_string(),
            by,
            by_type,
            text,
            at: now,
        };
        self.store.append_journal(&entry)?;
        let r = Self::room_mut(&mut rooms, room, &self.default_settings);
        r.next_journal_id = id + 1;
        r.journal.push(entry.clone());
        let _ = r.tx.send(ServerFrame::Journal {
            entry: entry.clone(),
        });
        Ok(entry)
    }

    // ---- notes (living, keyed project state) ----

    /// All notes in a room (sorted by key for stable display).
    pub fn notes(&self, room: &str) -> Vec<Note> {
        let rooms = self.rooms.lock_or_recover();
        let mut list: Vec<Note> = rooms
            .get(room)
            .map(|r| r.notes.values().cloned().collect())
            .unwrap_or_default();
        list.sort_by(|a, b| a.key.cmp(&b.key));
        list
    }

    /// One note by key.
    pub fn note(&self, room: &str, key: &str) -> Option<Note> {
        let rooms = self.rooms.lock_or_recover();
        rooms.get(room).and_then(|r| r.notes.get(key).cloned())
    }

    /// Create a note. Errors if the key already exists (use `update_note`).
    pub fn create_note(&self, room: &str, req: CreateNote) -> Result<Note, NoteError> {
        let mut rooms = self.rooms.lock_or_recover();
        if rooms
            .get(room)
            .is_some_and(|r| r.notes.contains_key(&req.key))
        {
            return Err(NoteError::Exists);
        }
        let rev = rooms.get(room).map(|r| r.next_rev).unwrap_or(1);
        let note = Note {
            key: req.key.clone(),
            title: req.title,
            body: req.body,
            can_write: req.can_write,
            updated_by: req.by,
            updated_at: (self.now_ms)(),
            rev,
        };
        self.store
            .upsert_note(room, &note)
            .map_err(|_| NoteError::Storage)?;
        let r = Self::room_mut(&mut rooms, room, &self.default_settings);
        r.next_rev = rev + 1;
        r.notes.insert(req.key, note.clone());
        let _ = r.tx.send(ServerFrame::Note { note: note.clone() });
        Ok(note)
    }

    /// Update an existing note in place. Errors if it doesn't exist (use
    /// `create_note`). Soft permissions: an unassigned writer still succeeds,
    /// but a `NoteWarn` frame is broadcast. `can_write` is only reassigned when
    /// `is_operator` — derived by the caller from the admin token, never from
    /// the request body.
    pub fn update_note(
        &self,
        room: &str,
        key: &str,
        req: UpdateNote,
        is_operator: bool,
    ) -> Result<Note, NoteError> {
        let mut rooms = self.rooms.lock_or_recover();
        let r = rooms.get_mut(room).ok_or(NoteError::NotFound)?;
        // (note: room must already exist; we don't create on update)
        let previous = r.notes.get(key).cloned().ok_or(NoteError::NotFound)?;
        let mut note = previous.clone();
        // Direction 3 (team memory): archive the version being replaced, so
        // "when and by whom did this change" stays answerable forever.

        // Soft-permission check (advisory).
        let allowed = note.can_write.is_empty() || note.can_write.contains(&req.by);
        let warn = if allowed {
            None
        } else {
            Some(ServerFrame::NoteWarn {
                key: key.to_string(),
                by: req.by.clone(),
                can_write: note.can_write.clone(),
            })
        };

        if let Some(t) = req.title {
            note.title = t;
        }
        if let Some(b) = req.body {
            note.body = b;
        }
        // Only the operator may reassign who can write.
        if is_operator {
            if let Some(cw) = req.can_write {
                note.can_write = cw;
            }
        }
        note.updated_by = req.by;
        note.updated_at = (self.now_ms)();
        note.rev = r.next_rev;
        let updated = note;

        self.store
            .add_note_revision(room, &previous)
            .map_err(|_| NoteError::Storage)?;
        self.store
            .upsert_note(room, &updated)
            .map_err(|_| NoteError::Storage)?;
        r.next_rev += 1;
        r.notes.insert(key.to_string(), updated.clone());
        let _ = r.tx.send(ServerFrame::Note {
            note: updated.clone(),
        });
        if let Some(w) = warn {
            let _ = r.tx.send(w);
        }
        Ok(updated)
    }

    /// A note's archived past versions (newest first).
    pub fn note_history(&self, room: &str, key: &str) -> Vec<Note> {
        self.store.note_history(room, key)
    }

    /// Room memory search: full message archive (DB) + current notes.
    pub fn search(&self, room: &str, q: &str, limit: usize) -> (Vec<Message>, Vec<Note>) {
        let msgs = self.store.search_messages(room, q, limit);
        let ql = q.to_lowercase();
        let notes: Vec<Note> = {
            let rooms = self.rooms.lock_or_recover();
            rooms
                .get(room)
                .map(|r| {
                    r.notes
                        .values()
                        .filter(|n| {
                            n.title.to_lowercase().contains(&ql)
                                || n.body.to_lowercase().contains(&ql)
                                || n.key.to_lowercase().contains(&ql)
                        })
                        .cloned()
                        .collect()
                })
                .unwrap_or_default()
        };
        (msgs, notes)
    }

    /// The bounded hot tail used by a normal browser room-open.
    pub fn recent_messages(&self, room: &str) -> Vec<Message> {
        let rooms = self.rooms.lock_or_recover();
        rooms
            .get(room)
            .map(|r| r.history.clone())
            .unwrap_or_default()
    }

    /// One ordered page after `since` (exclusive) for durable REST backfill.
    pub fn messages_after(
        &self,
        room: &str,
        since: u64,
        limit: usize,
    ) -> rusqlite::Result<Vec<Message>> {
        if let Some(messages) = self.store.messages_after(room, since, limit)? {
            return Ok(messages);
        }
        let rooms = self.rooms.lock_or_recover();
        Ok(rooms
            .get(room)
            .map(|r| {
                r.history
                    .iter()
                    .filter(|m| m.id > since)
                    .take(limit)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default())
    }

    pub fn members(&self, room: &str) -> Vec<Member> {
        let rooms = self.rooms.lock_or_recover();
        rooms.get(room).map(Room::member_list).unwrap_or_default()
    }

    /// Loca list for a caller. A davet opens ONE loca — so the list a davet
    /// holder sees is that one loca, not the whole building's floor plan (which
    /// leaked every loca's name, member count, mode and last line to anyone
    /// holding any davet). `reach` returns true for a loca the caller may
    /// enter; the master/building-key reaches all, a davet reaches its own.
    pub fn room_summaries_for(&self, reach: impl Fn(&str) -> bool) -> Vec<RoomSummary> {
        // Decide reachability BEFORE taking the rooms lock: `reach` calls
        // `enter_decision`, which itself locks `rooms` (via is_banned). Calling
        // it while holding the lock here would deadlock the same mutex.
        let names: Vec<String> = {
            let rooms = self.rooms.lock_or_recover();
            rooms.keys().cloned().collect()
        };
        let visible: std::collections::HashSet<String> =
            names.into_iter().filter(|n| reach(n)).collect();

        let rooms = self.rooms.lock_or_recover();
        let mut list: Vec<RoomSummary> = rooms
            .iter()
            .filter(|(name, _)| visible.contains(name.as_str()))
            .map(|(name, r)| {
                let humans = r
                    .members
                    .values()
                    .filter(|(_, k, n)| *n > 0 && *k == SenderType::User)
                    .count();
                let agents = r
                    .members
                    .values()
                    .filter(|(_, k, n)| *n > 0 && *k == SenderType::Agent)
                    .count();
                RoomSummary {
                    room: name.clone(),
                    members: humans + agents,
                    humans,
                    agents,
                    mode: r.mode.clone(),
                    archived: r.settings.archived,
                    last: r.history.last().map(|m| {
                        let t: String = m.text.chars().take(48).collect();
                        format!("{}: {}", m.sender, t)
                    }),
                    last_id: r.history.last().map(|m| m.id).unwrap_or(0),
                    special: self.is_reserved_room(name),
                }
            })
            .collect();
        list.sort_by(|a, b| a.room.cmp(&b.room));
        list
    }

    /// Delete a note by key. Returns whether it existed. Broadcasts a
    /// tombstone note (empty body, key prefixed) is overkill for v0 — clients
    /// simply refetch on the next `note` frame; here we just drop it and push a
    /// control so watchers can refresh.
    pub fn delete_note(&self, room: &str, key: &str) -> rusqlite::Result<bool> {
        let mut rooms = self.rooms.lock_or_recover();
        let Some(r) = rooms.get_mut(room) else {
            return Ok(false);
        };
        if !r.notes.contains_key(key) {
            return Ok(false);
        }
        self.store.delete_note(room, key)?;
        r.notes.remove(key);
        let _ = r.tx.send(ServerFrame::Control {
            cmd: format!("note-deleted:{key}"),
        });
        Ok(true)
    }
}

/// Why a chat post was rejected by the current mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostReject {
    /// Chat is paused; only the admin may speak.
    Paused,
    /// Restricted mode and the sender is not on the allowlist.
    NotAllowed,
    /// Round-robin and it is someone else's turn.
    NotYourTurn { whose: Option<String> },
    /// Sliding-window rate limit hit; retry after this many seconds.
    RateLimited { retry_after_secs: u64 },
    /// Frozen by the admin (muted).
    Muted,
    /// Banned from the room.
    Banned,
    /// The room is archived (closed) and read-only.
    Archived,
    /// The room was sealed (deleted); it does not come back on a post.
    Deleted,
    /// The message could not be persisted — refused rather than broadcast a
    /// line a restart would forget.
    Storage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReactionReject {
    InvalidEmoji,
    NotFound,
    OwnMessage,
    ReadOnly,
    Storage,
}

/// Why a room delete was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteReject {
    /// Active rooms can't be deleted — close (archive) them first.
    NotArchived,
    Storage,
}

impl PostReject {
    pub fn message(&self) -> String {
        match self {
            PostReject::Paused => "chat is paused by the admin".into(),
            PostReject::NotAllowed => "restricted mode: you are not on the allowlist".into(),
            PostReject::NotYourTurn { whose } => match whose {
                Some(w) => format!("round-robin: it is {w}'s turn"),
                None => "round-robin: no turn available".into(),
            },
            PostReject::RateLimited { retry_after_secs } => {
                format!("rate limit exceeded — slow down, retry in ~{retry_after_secs}s")
            }
            PostReject::Muted => "you are muted by the admin".into(),
            PostReject::Banned => "you are banned from this room".into(),
            PostReject::Archived => "this room is closed (archived) — read-only".into(),
            PostReject::Deleted => "this loca no longer exists".into(),
            PostReject::Storage => "could not save the message — try again".into(),
        }
    }

    /// Whether this rejection is a rate-limit (maps to HTTP 429 vs 403).
    pub fn is_rate_limit(&self) -> bool {
        matches!(self, PostReject::RateLimited { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttentionError {
    NotFound,
    NoRecipient,
    Forbidden,
    Conflict,
    Storage,
}

/// Why a note create/update was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteError {
    /// POST to a key that already exists — caller should PUT instead.
    Exists,
    /// PUT to a key that does not exist — caller should POST instead.
    NotFound,
    Storage,
}

fn default_now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Outcome of approving a join request.
#[derive(Debug, PartialEq, Eq)]
pub enum Approve {
    /// A fresh approval: stock consumed, Lobby membership issued.
    Approved,
    /// The request was already approved/denied/being-decided — a no-op that
    /// consumes no additional stock (idempotent re-approve).
    AlreadyDecided,
    /// The requested name now belongs to an existing member; approving would
    /// leak that member's credential, so it is refused (no stock consumed).
    NameTaken,
    /// No admission stock is available; the claim was released so the Master can
    /// retry after replenishing.
    NoStock,
    /// The membership could not be issued after the stock was consumed (rare);
    /// the consumed right was refunded and the claim released for retry.
    Failed,
}

/// Outcome of creating a join request.
#[derive(Debug, PartialEq, Eq)]
pub enum JoinRequestCreate {
    Created {
        request_id: String,
        request_secret: String,
    },
    /// The chosen name is already a member or already has a pending request.
    NameTaken,
    /// Too many undecided requests are queued.
    BacklogFull,
}

#[cfg(test)]
mod tests;
