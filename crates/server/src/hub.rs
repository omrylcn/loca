//! In-memory room state: message history, live broadcast, and member tracking.
//!
//! One [`Hub`] holds every room. Each [`Room`] keeps its backlog and a
//! `tokio::sync::broadcast` channel that every connected client subscribes to.
//! Human membership is reference-counted so the same person may read from two
//! web clients without vanishing from the roster when one closes. Agent
//! runtimes remain single-writer and replace a stale predecessor on reconnect.

mod admission;
mod attention;
mod authority;
mod build;
mod building_settings;
mod claims;
mod dm;
mod door;
mod extensions;
mod groups;
mod history;
mod identity;
mod invites;
mod journal;
mod keys;
pub mod kit_version;
mod lobby;
pub mod loca_name;
mod loca_removal;
mod locas;
mod master_agent;
mod master_agent_admissions;
mod members;
mod moderation;
mod notes;
mod notifications;
mod post;
mod profile_card;
mod ranks;
mod room;
mod search;
mod sessions;
mod vault;
mod wiki;
mod work;

pub use building_settings::BuildingNameError;
pub use claims::ClaimRefusal;
pub use dm::{DmCandidate, DmError, DmView};
pub use extensions::{normalize_extensions, ExtensionEdit, ExtensionError};
pub use groups::{GroupError, GroupView};
pub use identity::{IdentityEdit, IdentityError};
pub use keys::KeyRevokeError;
pub use loca_name::LocaNameError;
pub use loca_removal::LocaRemovalError;
pub use master_agent::MasterAgentError;
pub use master_agent_admissions::AdmissionError;
pub use notifications::{NotificationAudience, NotificationError, NOTIFICATION_SEND_LIMIT};
pub use vault::VaultError;
pub use work::{BoardOff, GoalError, OpenTasks, TaskError, Unfinished, WaitError};

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;

use protocol::{
    Attention, AttentionAudience, AttentionStatus, CareReason, CareSignal, ChatMode, CreateGoal,
    CreateNote, CreateTask, Goal, GoalCompletion, GoalStatus, Member, Message, Note, PostMessage,
    ReminderRecipient, RoomSettings, RoomSummary, SenderType, ServerFrame, SetWait, Task,
    TaskStatus, UpdateGoal, UpdateNote, UpdateTask, WaitState,
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
    profile: Option<protocol::RuntimeProfile>,
}

use crate::store::{
    secret_hash, BuildingRole, CredentialError, CredentialSummary, InviteRecord,
    LocaOperatorAssignment, LocaOperatorError, MemberRecord, PrincipalIdentity, Store,
};
use crate::sync::RecoverMutex;

/// Recent messages kept per room and replayed to new joiners.
const HISTORY_LIMIT: usize = 200;
/// Broadcast backlog; slow clients that lag past this get a `Lagged` and
/// resync from history on their next reconnect.
const BROADCAST_CAP: usize = 256;
type ModerationRanks = HashMap<(String, String, String), u8>;
/// Most attachments one message may cite (RFC limits & security).
const MAX_ATTACHMENTS_PER_MESSAGE: usize = 4;
// Attachment limits: compile-time defaults, each overridable by an env var so
// an operator (and the test suite) can tune them without a rebuild — the RFC's
// attachments.*_max_bytes knobs. Read on use: uploads are infrequent, so the
// env lookup cost is irrelevant and there is no config to thread through the
// Hub. A non-positive or unparseable override falls back to the default.
const ATTACHMENT_MAX_BYTES_DEFAULT: u64 = 10 * 1024 * 1024;
const ATTACHMENT_ROOM_MAX_BYTES_DEFAULT: u64 = 200 * 1024 * 1024;
const ATTACHMENT_BUILDING_MAX_BYTES_DEFAULT: u64 = 2 * 1024 * 1024 * 1024;
const ATTACHMENT_PENDING_TTL_MS_DEFAULT: u64 = 60 * 60 * 1000;

fn attachment_env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(default)
}
/// Per-file upload cap (bytes). `ATTACHMENT_MAX_BYTES` overrides. The upload
/// route also caps the streamed body at this so an oversize file is rejected
/// before it is ever fully buffered.
pub(crate) fn attachment_max_bytes() -> u64 {
    attachment_env_u64("ATTACHMENT_MAX_BYTES", ATTACHMENT_MAX_BYTES_DEFAULT)
}

#[derive(Clone, Copy)]
struct CareMark {
    last_signal_at: u64,
    signal_count: u32,
}

struct CareDraft {
    attention_key: String,
    owner: Option<String>,
    /// Set only for an Everyone per-member signal — the canonical principal that
    /// must receive it. Its presence flips the signal to principal-only delivery
    /// and a `Person` audience (no group broadcast).
    owner_principal_id: Option<String>,
    /// Generation id shared by every per-member signal of one Everyone reminder.
    group: Option<String>,
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
    /// seat identity -> (display name, kind, live connections by session id).
    /// One key = one seat: the map is keyed by IDENTITY (derived from the
    /// credential at the door — "@master", "mb:…", "sm:…", "st:…", "name:…"),
    /// never by the free-text display name. The same key reconnecting under a
    /// new name takes over its old seat; it cannot sit twice. Two human web
    /// clients under the same identity+name share that one seat, each listed
    /// by its session. The name is just the label the seat currently wears.
    members: HashMap<String, (String, SenderType, std::collections::HashSet<u64>)>,
    /// seat identity -> the principal the door verified for it (the same id
    /// its messages are attributed to). Only such a seat wears that person's
    /// full name and icon (`Hub::prove_seat`); a seat taken by typing a name
    /// has no entry and shows its plain name.
    seat_principals: HashMap<String, String>,
    /// seat identity -> (the connection that sent it, the kit version it
    /// sent). Shown only while that connection holds the seat
    /// (`kit_version.rs`).
    seat_kits: HashMap<String, (u64, kit_version::KitVersion)>,
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
            seat_principals: HashMap::new(),
            seat_kits: HashMap::new(),
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

    /// The roster. `cards` (`Hub::profile_cards`, by principal) adds a
    /// person's full name and icon to a seat whose principal the door
    /// verified; every other seat is listed exactly as before.
    fn member_list(&self, cards: &HashMap<String, crate::store::ProfileCard>) -> Vec<Member> {
        let mut list: Vec<Member> = self
            .members
            .iter()
            .map(|(identity, (name, kind, _))| {
                let principal = self.seat_principals.get(identity);
                let card = principal.and_then(|principal| cards.get(principal));
                Member {
                    name: name.clone(),
                    kind: *kind,
                    role: seat_rank(identity).map(str::to_string),
                    principal_id: principal.cloned(),
                    full_name: card.and_then(|c| c.full_name.clone()),
                    icon: card.and_then(|c| c.icon.clone()),
                    avatar_color: card.and_then(|c| c.avatar_color.clone()),
                    avatar_shape: card.and_then(|c| c.avatar_shape.clone()),
                    kit_version: self.seat_kit_version(identity).map(|v| v.to_string()),
                }
            })
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }
}

/// The Building rank a seat key proves. Seat keys are minted only by
/// `Hub::seat_identity` from server-side credentials, so the prefix is
/// authoritative: `@master` is the Master, every Smaster seat starts `sm:`.
pub(crate) fn seat_rank(identity: &str) -> Option<&'static str> {
    if identity == "@master" {
        Some("master")
    } else if identity.starts_with("sm:") {
        Some("smaster")
    } else {
        None
    }
}

/// A test-only pause point (see `Hub::set_reissue_swapped_hook`).
#[cfg(test)]
pub type TestHook = Arc<dyn Fn() + Send + Sync>;

/// The whole server's room state. Cheap to clone (`Arc` inside).
///
/// # Lock order
///
/// Every field below that is a `Mutex` is a plain `std::sync::Mutex`: not
/// re-entrant, so a function must not call another that takes a lock it
/// already holds. The order, outermost first, as the code takes them (each
/// line restates a comment kept beside the code that relies on it):
///
/// 0. `master_agent_admitting` — only around a Master Agent's admission;
///    `name_claims` is taken under it.
/// 1. `name_claims` — only while a name comes into being or changes, and
///    taken first, with no other hub lock held (or under 0).
/// 2. `member_lifecycle` — membership cutoffs; under `name_claims` only for
///    a rename.
/// 3. `moderation_ranks`.
/// 4. `sessions` → `invites`, `members` (`hub/sessions.rs`).
/// 5. `rooms` → `groups` (`create_loca`, `post`; `hub/groups.rs`). `groups`
///    is a leaf: nothing else is taken under it but the store's connection.
/// 6. The store's connection, last, under any of the above.
///
/// A door decision (`enter_decision`, via `is_banned`) takes `rooms` itself,
/// so callers decide reachability before they take `rooms`
/// (`hub/history.rs`, `hub/search.rs`, `hub/identity.rs`). A new path that
/// needs two of these locks takes them in this order.
#[derive(Clone)]
pub struct Hub {
    rooms: Arc<Mutex<HashMap<String, Room>>>,
    /// Locas the master has deleted. A create-on-access hub would otherwise let
    /// the next touch — a `watch=1` listener's subscribe, a stray join — quietly
    /// re-create a room the operator just removed. A tombstone here means "this
    /// name is deleted, do not resurrect it"; it is cleared when the master
    /// explicitly opens the loca again.
    deleted: Arc<Mutex<std::collections::HashSet<String>>>,
    /// Direct messages by room id (PRINCIPLES "Birebir"): private two-person locas.
    dms: Arc<Mutex<HashMap<String, crate::store::Dm>>>,
    /// Each loca's active Loca Operator principal, mirrored from
    /// `room_operator_assignments`. The door asks on every request (and
    /// `GET /rooms` once per loca), so it must not cost a SQLite query.
    /// A leaf lock: held across the store write that changes an assignment,
    /// never while taking another hub lock.
    operators: Arc<Mutex<HashMap<String, String>>>,
    /// Groups, their members and their locas (PRINCIPLES "Groups"). A leaf
    /// lock; see `hub/groups.rs` for the order rules.
    groups: Arc<Mutex<groups::GroupBook>>,
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
    /// Live memberships, keyed by their management id (`mbid_`), never by
    /// their key: who belongs to the building. Separate from invites: an
    /// invite seats a member in a loca, membership is the identity itself and
    /// survives every room they leave. A record holds no key; a presented
    /// key is found through its credential (persistent) or `member_keys`.
    members: Arc<Mutex<HashMap<String, MemberRecord>>>,
    /// A memory-only Building's membership keys: each key's `secret_hash` ->
    /// the membership's id. A persistent Building finds a presented key
    /// through `credentials` instead and keeps nothing here.
    member_keys: Arc<Mutex<HashMap<String, String>>>,
    /// Membership keys claimed with an invite while the Building has no
    /// database (memory-only mode); a persistent Building keeps them in
    /// `credentials` and `membership_claims`. See `hub/claims.rs`.
    memory_claims: Arc<Mutex<claims::MemoryClaims>>,
    /// Live legacy Smaster codes, keyed by the code's `secret_hash` (never by
    /// the code), -> name, principal and management id. A Smaster is a second
    /// master: same powers, but the master has the last word (see `is_master`).
    smasters: Arc<Mutex<HashMap<String, crate::store::SmasterRecord>>>,
    /// Issued sessions, keyed by the token's `secret_hash` (never by the
    /// token), -> bound identity. A presented token is hashed and looked up;
    /// the hash itself is not a token. Invite sessions are ephemeral; browser
    /// admin sessions survive a deploy until their normal expiry, and after
    /// a restart the server has only their hashes.
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
    /// longer be spoofed via the request body (PRODUCTION.md stage 0).
    require_sessions: bool,
    /// Seats per loca (`LOCA_CAPACITY`, default [`Hub::DEFAULT_LOCA_CAPACITY`]).
    /// Shared across clones so one configured value governs every handler.
    loca_capacity: Arc<std::sync::atomic::AtomicUsize>,
    /// The most seats a loca's own number may reach (`LOCA_CAPACITY_MAX`,
    /// default [`Hub::LOCA_CAPACITY_MAX`]); read through
    /// `loca_capacity_max()`, which never answers below `loca_capacity`.
    loca_capacity_max: Arc<std::sync::atomic::AtomicUsize>,
    /// Notifications one sender may send per window
    /// (`NOTIFICATION_RATE_LIMIT`, default [`NOTIFICATION_SEND_LIMIT`]).
    notification_send_limit: Arc<std::sync::atomic::AtomicU32>,
    /// When true, an empty room_token means 'invite-only', not 'open house'.
    /// This is how the building key is retired: no key, but the door still
    /// demands an invite for every loca.
    require_invite: bool,
    /// Live invites, keyed by the invite's `secret_hash` (never by the
    /// invite): a presented invite is hashed and looked up, and the hash itself
    /// is not an invite. Unlike sessions these are persisted: a restart must
    /// not un-invite anyone.
    invites: Arc<Mutex<HashMap<String, InviteRecord>>>,
    /// Live invites whose delivery copy is still held (`invites.delivery_token`
    /// in a persistent store), by their hash -> the invite, the one thing a
    /// delivery needs in clear text: minted for delivery and neither used nor
    /// past `INVITE_DELIVERY_WINDOW_MS`. Every Lobby connect of the member is
    /// sent these again. Never held while taking the `invites` lock.
    deliveries: Arc<Mutex<HashMap<String, String>>>,
    /// Building-lobby signal bus. Lobby sockets never join a loca; they wait
    /// here for an invite addressed to their permanent membership.
    lobby_tx: broadcast::Sender<LobbyEvent>,
    /// Serialize membership access cutoffs with invite persistence/cache writes.
    member_lifecycle: Arc<Mutex<()>>,
    /// Makes "nobody wears this name in any letter case" and "now someone
    /// does" one step. Held from the clash check until the name is written,
    /// in the store and in memory, by every path that brings a name into
    /// being: `admit_member` (direct admission and the master desk),
    /// `approve_join_request`, `create_join_request` (a pending request
    /// holds its name), `add_smaster` (both Smaster doors) and
    /// `rename_profile` (a rank's own rename and the Master's rename of a
    /// Smaster). Lock order: `name_claims`, then `member_lifecycle` (only the
    /// rename takes it), then `moderation_ranks`, then the data locks
    /// (`members`, `smasters`, `sessions`, `rooms`), then the store's
    /// connection. It is taken only by those six functions, first, while no
    /// other hub lock is held, and none of them calls another while it holds
    /// it (it is not re-entrant).
    name_claims: Arc<Mutex<()>>,
    /// Makes a Master Agent's admission one step: its limits are counted,
    /// the member admitted and the claim recorded under it, so concurrent
    /// admissions cannot all pass the same count. Taken first, before
    /// `name_claims` (which `admit_member_once` takes inside it).
    master_agent_admitting: Arc<Mutex<()>>,
    /// Serialize restriction checks and writes; keep provenance in memory-only
    /// mode too. Persistent decisions restore this mirror before serving.
    moderation_ranks: Arc<Mutex<ModerationRanks>>,
    /// Live lobby sockets per membership, by its management id (`mbid_`). A
    /// count (rather than a set) keeps presence correct while one identity is
    /// reconnecting.
    lobby_online: Arc<Mutex<HashMap<String, usize>>>,
    /// The kit version each live Lobby connection sent (`kit_version.rs`).
    lobby_kits: Arc<Mutex<kit_version::LobbyKits>>,
    /// Adapter health is ephemeral and deliberately separate from transport
    /// presence. Heartbeats never become membership or room history.
    runtime_health: Arc<Mutex<HashMap<String, RuntimeRecord>>>,
    /// Hot delivery-attempt → stable attention identity map. SQLite is the
    /// restart authority; this mirror keeps memory-only/dev behavior exact.
    care_deliveries: Arc<Mutex<HashMap<String, String>>>,
    /// The building's private governance/caretaker loca (production: `iye`).
    home_room: Arc<String>,
    /// When set, only configured caretakers may receive an invite for this loca;
    /// master and smaster enter by rank and need no invite.
    reserved_room: Arc<String>,
    caretakers: Arc<std::collections::HashSet<String>>,
    /// Test-only: runs inside `reissue_invite` right after the swap, before
    /// the old seat's clean-up and the delivery, so a test can hold one
    /// re-issue there while another runs.
    #[cfg(test)]
    reissue_swapped_hook: Arc<Mutex<Option<TestHook>>>,
}

#[derive(Clone)]
struct AdminPairing {
    code: String,
    session_ttl_ms: u64,
    expires_at_ms: u64,
    /// The ledger row of a code that was handed out; `None` for the internal
    /// placeholder rotated in after a use or an expiry (never handed out).
    log_id: Option<String>,
}

/// A private signal delivered only to the lobby socket of the membership
/// `member` (its id, `mbid_`). The invite remains persisted in `invites`; this
/// bus is merely the immediate wake-up path, and `Called.token` is the invite
/// being delivered, held only for that. A reconnect receives the seat list
/// and the invites whose delivery is still owed (`Hub::pending_deliveries`).
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
    MembershipSuspended {
        member: String,
    },
    /// One credential was revoked at the profile door (door B): the Lobby
    /// sockets that authenticated with it close. The membership and the
    /// member's other credentials are untouched.
    CredentialRevoked {
        credential_id: String,
    },
    /// A Building notification (docs/concepts.md "Notifications"), addressed
    /// by principal id: one event reaches every recipient's Lobby socket.
    /// `frame` is the ready `{"t":"notification",…}` text; it carries no
    /// credential.
    Notification {
        recipients: Arc<std::collections::HashSet<String>>,
        frame: Arc<String>,
    },
}

impl LobbyEvent {
    /// The membership (its id) a per-member event is for; `None` for an
    /// event addressed by principal (a notification) or by credential.
    pub fn member(&self) -> Option<&str> {
        match self {
            Self::Called { member, .. }
            | Self::MembershipRevoked { member }
            | Self::MembershipSuspended { member } => Some(member),
            Self::CredentialRevoked { .. } | Self::Notification { .. } => None,
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

/// What an id names at the door of a private loca (see `Hub::private_door`).
enum PrivateDoor {
    /// A direct message or a group loca, and the door's answer for it.
    Private(EnterDecision),
    /// The direct message id space, naming no direct message: nobody enters there.
    Reserved,
    /// Anything else: the ordinary door decides.
    Ordinary,
}

/// Why an invite could not be issued (see [`Hub::invite_member_to_room`]).
/// An invite never creates identity — it seats an existing member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InviteError {
    /// Closed Buildings require an explicitly created loca before an invitation.
    MissingRoom,
    /// The name is not a building member — admit them first.
    MemberNotFound,
    /// This member already holds a live invite for this loca.
    AlreadyInvited,
    /// The loca is full — every configured seat is already handed out.
    Full,
    /// The invite could not be persisted.
    Storage,
    /// This is the building's private governance/caretaker loca.
    Reserved,
    /// Sealed rooms retain their records and cannot be reopened by an invite.
    Sealed,
    /// A group loca seats only its group's members.
    NotInGroup,
    /// Direct message admission is internal and bound to its two principals.
    PrivatePair,
    /// Every Building member is already in the Lobby room; it takes no
    /// invitations.
    Lobby,
}

/// Why an invite could not be re-issued (see [`Hub::reissue_invite`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReissueError {
    /// Refused for a reason a call is refused for too (sealed or missing
    /// loca, not a live member, reserved loca, group gate, direct message, storage).
    Invite(InviteError),
    /// The member holds no live invite for this loca: there is nothing to
    /// re-issue, and a re-issue never turns into a first call.
    NotHeld,
    /// The caller may not end the invite it would replace.
    Forbidden,
}

/// Why `Hub::create_loca_in` refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateLocaError {
    Operator(OperatorAssignmentError),
    Group(GroupError),
    /// An identity field (description, seats, icon, colour, labels) broke
    /// its rule.
    Identity(IdentityError),
    /// The name broke the rule for a new loca (`loca_name`).
    Name(LocaNameError),
    /// A Master Agent asked for a floor it was not given.
    MasterAgent(MasterAgentError),
}

impl From<OperatorAssignmentError> for CreateLocaError {
    fn from(error: OperatorAssignmentError) -> Self {
        Self::Operator(error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeadError {
    ActiveGoal,
    /// The Loca Board is on: it needs a Lead.
    BoardOn,
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
    /// The loca this session is confined to, when it was taken with an invite.
    /// `None` = taken with a Building-level credential: the building key, the
    /// admin key or pairing, or a member's own key or profile credential.
    /// Without this an invite holder could take a session and walk the whole
    /// building — the invite would mean nothing.
    pub loca: Option<String>,
    /// The building membership this session proves, by its management id
    /// (`mbid_`, never the key), when it was taken with an invite — or with the
    /// member's own key in a memory-only store, where there is no principal
    /// to bind instead. Empty for an unbound legacy invite, which has no
    /// membership. The seat key derives from this: one member, one seat,
    /// whatever display name the connection claims.
    pub member: Option<String>,
    /// The `secret_hash` of the invite this session was taken with. An invite
    /// session lives while that invite is live and the member's principal is
    /// active; it is not bound to any of the member's credentials, so
    /// revoking one of them (the original key included) leaves it alone.
    pub invite: Option<String>,
    /// Was this session taken with a credential that reaches the whole
    /// Building through the legacy building key (`ROOM_TOKEN` itself)? Only
    /// such a session inherits that reach; one taken with a member's own key
    /// or profile credential reaches what that member is entitled to.
    pub building_key: bool,
    /// Does this bounded session authenticate the Master principal? It lets
    /// the browser prove Master authority without retaining or repeatedly
    /// transmitting the root/bootstrap/recovery credential. It expires so a
    /// leaked session dies on its own.
    pub admin: bool,
    /// Unix-ms from which this session is no longer valid. Every session has
    /// one: [`Hub::ADMIN_SESSION_TTL_MS`] after it was taken, or the lifetime
    /// chosen for a Master pairing. A leaked or forgotten session dies on its
    /// own, and the background sweep removes it. `0` is never issued and
    /// reads as already expired.
    pub expires_at: u64,
}

/// The key an `op_id` is remembered under (`messages.principal`), built by
/// `Hub::operation_scope`.
#[derive(Clone, Debug)]
pub struct OperationScope {
    /// What a new operation is stored under: `pr:<principal id>`,
    /// `mb:<membership>` or `label:<kind>:<name>`.
    key: String,
    /// The verified principal behind `key`, when there is one.
    principal: Option<String>,
    /// The key the REST route stored this request's operations under before
    /// this version, when it differs from `key`. Read only.
    legacy: Option<String>,
}

/// Whose a record found under a legacy operation key is, as far as the
/// server can prove (see `Hub::legacy_operation_owner`).
enum LegacyOwner {
    /// The sender now re-sending: the record answers the re-send.
    Sender,
    /// Somebody else, by the writer the record names: it is not this
    /// sender's operation, and the sender's own is posted.
    Other,
    /// A record that names no writer under a key several senders could have
    /// built. Neither answering with it nor posting again is safe.
    Unknown,
}

/// Words that name a rank, a title or an addressing keyword. A person's name
/// is only a label, so it must never read as a title: "smaster" on the roster
/// says nothing about who is sitting there, and `@all` is not a person. The
/// address words themselves are `ADDRESS_WORDS`, refused here too.
/// The id of the Lobby room (`Hub::is_lobby_room`).
pub const LOBBY_ROOM: &str = "lobby";

const RESERVED_NAMES: &[&str] = &[
    "master", "smaster", "operator", "lead", "admin", "root", "everyone", "here", "goal",
];

/// Words that address a message instead of naming a person: a `target` of
/// `all`, or `@all` in the text, calls everyone at the table (`msg_addresses`
/// in ws.rs). Somebody named so could never be addressed alone, so no door
/// admits a new name that is one of these, in any letter case. The admission
/// doors' refusal names the word (`ADDRESS_WORD_RULE`).
const ADDRESS_WORDS: &[&str] = &["all"];

/// Is `name` an address word (`ADDRESS_WORDS`), compared without case?
pub(crate) fn is_address_word(name: &str) -> bool {
    ADDRESS_WORDS
        .iter()
        .any(|word| name.eq_ignore_ascii_case(word))
}

/// Why `valid_agent_name` refused a name, said the same way at every door.
pub(crate) const AGENT_NAME_RULE: &str =
    "name must be 1-64 ASCII letters, digits, dot, dash, or underscore, starting with a letter or digit";

/// A new identity name: 1-64 ASCII letters, digits, `.`, `_` or `-`, and the
/// first a letter or digit, the rule the agent kit and the web client hold
/// too (`routes::valid_identity_name`). Names already on record are not
/// re-checked.
pub(crate) fn valid_agent_name(name: &str) -> bool {
    name.as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphanumeric)
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Why an address word (`is_address_word`) is refused as a new name.
pub(crate) const ADDRESS_WORD_RULE: &str = "\"all\" is an address that calls everyone, not a name";

/// Why a name being admitted now is refused, said the same way at every
/// admission door: the character rule first, then the address words. The
/// join-request door applies it to the name it files; `Hub::admit_member`
/// and `Hub::approve_join_request` apply it under `name_claims`, where the
/// name is written.
pub(crate) fn admission_name_refusal(name: &str) -> Option<&'static str> {
    if !valid_agent_name(name) {
        Some(AGENT_NAME_RULE)
    } else if is_address_word(name) {
        Some(ADDRESS_WORD_RULE)
    } else {
        None
    }
}

/// Why a display name is refused (see `validate_display_name`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameError {
    /// Empty, too long, or containing characters a `@mention` cannot carry.
    Invalid,
    /// A rank, title or addressing keyword (`RESERVED_NAMES`).
    Reserved,
}

/// A person's display name: 1–64 ASCII letters, digits, `-`, `_` or `.`,
/// starting with a letter or digit (the join-request rule, so it survives
/// `@mention`), and never a rank or title word.
pub fn validate_display_name(name: &str) -> Result<&str, NameError> {
    let name = name.trim();
    if !name
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphanumeric)
        || name.len() > 64
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(NameError::Invalid);
    }
    let folded = crate::store::search_fold(name);
    if RESERVED_NAMES.contains(&folded.as_str()) || is_address_word(name) {
        return Err(NameError::Reserved);
    }
    Ok(name)
}

/// Why `Hub::rename_profile` refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileRenameError {
    /// Only a verified Master or Smaster principal renames itself here.
    AuthorityRequired,
    Name(NameError),
    /// Another active person already wears this name.
    NameTaken,
    /// Labels still key legacy moderation; renaming may not evade a restriction.
    RestrictionActive,
    Failed,
}

/// One Building rank and where it currently sits (see `Hub::building_ranks`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RankPresence {
    pub name: String,
    pub role: &'static str,
    pub locas: Vec<String>,
    /// A Smaster code that was issued but never used: a code, not a person.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub unused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestAuthority {
    pub principal_id: Option<String>,
    pub display_name: Option<String>,
    pub kind: Option<SenderType>,
    pub building_role: Option<BuildingRole>,
}

impl RequestAuthority {
    fn moderation_rank(&self) -> u8 {
        match self.building_role {
            Some(BuildingRole::Master) => 3,
            Some(BuildingRole::Smaster) => 2,
            _ => 1,
        }
    }

    pub fn invitation_issuer(&self) -> String {
        if self.is_master() {
            "master".into()
        } else if self.building_role == Some(BuildingRole::Smaster) {
            format!(
                "smaster:{}",
                self.display_name.as_deref().unwrap_or("unknown")
            )
        } else if self.building_role == Some(BuildingRole::MasterAgent) {
            Hub::master_agent_label(self)
        } else {
            format!(
                "operator:{}",
                self.principal_id.as_deref().unwrap_or("unknown")
            )
        }
    }
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

#[derive(Debug)]
pub enum ModerationError {
    Forbidden,
    Storage,
}

impl From<rusqlite::Error> for ModerationError {
    fn from(error: rusqlite::Error) -> Self {
        tracing::error!(%error, "moderation persistence failed");
        Self::Storage
    }
}

/// The rank an invite's issuer holds, on the scale of
/// `RequestAuthority::moderation_rank`. Master, internal (`group-move`,
/// `direct message`) and unknown legacy issuers fail closed at the Master's rank.
fn invite_issuer_rank(issued_by: &str) -> u8 {
    if issued_by.starts_with("smaster:") {
        2
    } else if issued_by == "operator"
        || issued_by.starts_with("operator:")
        // A Master Agent ranks below every Smaster: either may end its invites.
        || issued_by.starts_with("master-agent:")
    {
        1
    } else {
        3
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
    const RUNTIME_SEEN_TTL_MS: u64 = 20_000;
    const RUNTIME_WAKE_GRACE_MS: u64 = 10_000;
    const RUNTIME_ACK_GRACE_MS: u64 = 360_000;

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

    /// Lifetime of every session except a Master pairing's, which carries the
    /// duration chosen for it: 12 hours, whether taken with an invite, a
    /// membership, a Smaster or profile key, or the sandbox building key.
    /// Clients renew from the key they hold when a session is refused.
    pub const ADMIN_SESSION_TTL_MS: u64 = 12 * 60 * 60 * 1000;
    pub const MIN_ADMIN_SESSION_TTL_MS: u64 = 60 * 60 * 1000;
    pub const MAX_ADMIN_SESSION_TTL_MS: u64 = 365 * 24 * 60 * 60 * 1000;
    /// A pairing code is only the bridge into an admin session. Keeping an
    /// unused bearer alive longer than a few minutes widens the log/screen
    /// shoulder-surfing window without helping the operator.
    pub const ADMIN_PAIRING_TTL_MS: u64 = 5 * 60 * 1000;

    fn secure_token(prefix: &str, bytes: usize) -> String {
        use std::fmt::Write as _;

        use rand::{rngs::OsRng, RngCore, TryRngCore};

        let mut random = vec![0_u8; bytes];
        // The operating system's source, as before; a failure to read it
        // panics as `fill_bytes` did.
        OsRng.unwrap_err().fill_bytes(&mut random);
        let mut token = String::with_capacity(prefix.len() + random.len() * 2);
        token.push_str(prefix);
        for byte in random {
            write!(&mut token, "{byte:02x}").expect("writing to a String cannot fail");
        }
        token
    }

    /// Max undecided join requests at once — a DoS guard on the authless create
    /// endpoint (a full per-source rate-limiter is a documented hardening
    /// follow-up). A request grants no authority, so the only abuse is table
    /// growth, which this bounds.
    const MAX_PENDING_JOIN_REQUESTS: usize = 200;
    const JOIN_CREATE_WINDOW_MS: u64 = 60_000;
    const JOIN_CREATE_MAX_IN_WINDOW: usize = 30;

    /// Register a member on connect and broadcast the updated roster.
    ///
    /// Agent runtimes are last-writer-wins: a fresh listener evicts a stale
    /// predecessor so a ghost cannot shadow delivery. Human web readers are
    /// different: the same identity+name may be open on multiple devices, all
    /// sharing one roster entry and one seat, each listed by its session.
    /// A loca seats at most this many. The number is not arbitrary: a loca is
    /// by definition a small, private room — more than this and it is a salon,
    /// a different kind of place (DESIGN §5g). Watchers do not take a seat, so
    /// they are not counted here (they never call `join`).
    pub const DEFAULT_LOCA_CAPACITY: usize = 7;
    /// The bounds a configured capacity is held to: a loca stays a small,
    /// private table — at least a pair, never a hall.
    pub const LOCA_CAPACITY_MIN: usize = 2;
    pub const LOCA_CAPACITY_MAX: usize = 32;
}

/// Attachment operations the Hub exposes to the upload/fetch routes. Thin
/// pass-throughs to the Store that stamp the current time and the configured
/// quotas, so the routes stay policy-free.
impl Hub {
    /// Attachments need a durable blob dir + DB (memory-only mode → off).
    pub fn attachments_enabled(&self) -> bool {
        self.store.attachments_enabled()
    }

    /// Store an uploaded blob as pending in `room` (see Store). `mime` is the
    /// server-sniffed type; `name` is already sanitized display metadata.
    pub fn put_pending_attachment(
        &self,
        room: &str,
        uploader: &str,
        name: &str,
        mime: &str,
        bytes: &[u8],
    ) -> Result<protocol::Attachment, crate::store::AttachError> {
        let now = (self.now_ms)();
        self.store.put_pending_attachment(
            room,
            uploader,
            name,
            mime,
            bytes,
            now,
            attachment_env_u64(
                "ATTACHMENT_ROOM_MAX_BYTES",
                ATTACHMENT_ROOM_MAX_BYTES_DEFAULT,
            ),
            attachment_env_u64(
                "ATTACHMENT_BUILDING_MAX_BYTES",
                ATTACHMENT_BUILDING_MAX_BYTES_DEFAULT,
            ),
        )
    }

    /// Read a blob for serving IF it is referenced by a message in `room`.
    pub fn read_room_attachment(&self, room: &str, id: &str) -> Option<crate::store::BlobServe> {
        self.store.read_room_attachment(room, id)
    }

    /// Collect expired pending uploads and unreferenced blobs. Returns the
    /// number of physical files deleted.
    pub fn sweep_attachments(&self) -> usize {
        let now = (self.now_ms)();
        self.store.sweep_attachments(
            now,
            attachment_env_u64(
                "ATTACHMENT_PENDING_TTL_MS",
                ATTACHMENT_PENDING_TTL_MS_DEFAULT,
            ),
        )
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
    /// More attachments than a single message may carry.
    TooManyAttachments { max: usize },
    /// A cited attachment id is not an available upload in this room (never
    /// uploaded here, already swept, or belongs to another room).
    UnknownAttachment,
    /// The `op_id` matches a record an earlier version stored under a name
    /// alone, with no writer: the server cannot tell a re-send from another
    /// sender's operation under the same name. Nothing is posted and nobody
    /// else's message is returned.
    OperationUnclear,
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
            PostReject::OperationUnclear => {
                "this operation id was used before the upgrade under your name, and the \
                 server cannot tell whether by you; nothing was posted — send it again \
                 as a new operation"
                    .into()
            }
            PostReject::TooManyAttachments { max } => {
                format!("too many attachments — at most {max} per message")
            }
            PostReject::UnknownAttachment => {
                "an attachment id is not an available upload in this loca".into()
            }
        }
    }

    /// Whether this rejection is a rate-limit (maps to HTTP 429 vs 403).
    pub fn is_rate_limit(&self) -> bool {
        matches!(self, PostReject::RateLimited { .. })
    }

    /// A caller-input rejection (maps to HTTP 400, not the 403 mode gates use).
    pub fn is_bad_request(&self) -> bool {
        matches!(
            self,
            PostReject::TooManyAttachments { .. } | PostReject::UnknownAttachment
        )
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
    /// Someone wears the requested name in another letter case, or it is a
    /// rank's name (`new_member_name_clashes`). Nothing is written: the
    /// request stays pending and no stock is consumed.
    NameWorn,
    /// An older server filed the request under a name the doors' rule
    /// refuses (`admission_name_refusal`, whose sentence this carries).
    /// Nothing is written, as for `NameWorn`.
    NameRule(&'static str),
    /// No admission stock is available; the transaction rolled back, so the
    /// request stays pending and no right is consumed — the Master can retry
    /// after replenishing.
    NoStock,
    /// A DB error rolled the whole approval back (rare); nothing was persisted —
    /// no member, no consumed right — and the request stays pending for retry.
    Failed,
}

/// Why `add_smaster` minted no code.
#[derive(Debug)]
pub enum NameClaimError {
    /// Someone already wears it in some letter case.
    Worn,
    /// The store refused the write; nothing was changed.
    Storage,
}

impl From<rusqlite::Error> for NameClaimError {
    fn from(_: rusqlite::Error) -> Self {
        Self::Storage
    }
}

/// Why `admit_member` admitted nobody.
#[derive(Debug)]
pub enum AdmitError {
    /// The new name breaks the doors' rule (`admission_name_refusal`, whose
    /// sentence this carries).
    Rule(&'static str),
    /// Someone already wears it in some letter case.
    Worn,
    /// The store refused the write; nothing was changed.
    Storage,
}

impl From<rusqlite::Error> for AdmitError {
    fn from(_: rusqlite::Error) -> Self {
        Self::Storage
    }
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
    /// Someone in the Building wears the chosen name in another letter case,
    /// or it is a rank's name (`Hub::new_member_name_clashes`).
    NameWorn,
    /// Too many undecided requests are queued.
    BacklogFull,
}

/// The names a text calls with `@name`, lowercased, in the order written.
/// This is the one mention grammar of the server (the agent kit mirrors it,
/// and `docs/mention-cases.json` pins it for both): a name is the
/// identity-name characters (letters, digits, `-`, `_`, `.`) after an `@`
/// that does not sit inside a word (an e-mail address is not a mention);
/// a trailing `.` or `-` is punctuation, so `@ayse.` calls `ayse` and
/// `@ops.bot` calls `ops.bot`. `all` is returned like any other name.
pub(crate) fn mentioned_names(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut prev: Option<char> = None;
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '@' && prev.is_none_or(|p| !p.is_alphanumeric() && p != '_' && p != '.') {
            let start = i + 1;
            let mut end = start;
            while end < chars.len()
                && (chars[end].is_alphanumeric() || matches!(chars[end], '-' | '_' | '.'))
            {
                end += 1;
            }
            let name: String = chars[start..end].iter().collect();
            let name = name.trim_end_matches(['.', '-']);
            if !name.is_empty() {
                names.push(name.to_lowercase());
            }
            i = end;
            prev = chars.get(end.saturating_sub(1)).copied();
            continue;
        }
        prev = Some(c);
        i += 1;
    }
    names
}

/// Does this text call everyone with `@all`, as a whole word in any letter
/// case? The same grammar the mentions filter reads (`mentioned_names`).
pub(crate) fn text_calls_all(text: &str) -> bool {
    mentioned_names(text).iter().any(|name| name == "all")
}

/// Does this text address specific people with `@name`, and not everyone with
/// `@all`? (`mentioned_names`.)
pub(crate) fn names_people_but_not_all(text: &str) -> bool {
    let names = mentioned_names(text);
    !names.is_empty() && !names.iter().any(|name| name == "all")
}

#[cfg(test)]
mod tests;
