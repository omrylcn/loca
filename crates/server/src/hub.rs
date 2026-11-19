//! In-memory room state: message history, live broadcast, and member tracking.
//!
//! One [`Hub`] holds every room. Each [`Room`] keeps its backlog and a
//! `tokio::sync::broadcast` channel that every connected client subscribes to.
//! Human membership is reference-counted so the same person may read from two
//! web clients without vanishing from the roster when one closes. Agent
//! runtimes remain single-writer and replace a stale predecessor on reconnect.

mod attention;
mod building_settings;
mod claims;
mod dm;
mod extensions;
mod groups;
mod identity;
mod keys;
pub mod kit_version;
pub mod loca_name;
mod loca_removal;
mod notifications;
mod profile_card;
mod search;
mod vault;
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
/// in main.rs). Somebody named so could never be addressed alone, so no door
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
    "name must be 1-64 ASCII letters, digits, dot, dash, or underscore";

pub(crate) fn valid_agent_name(name: &str) -> bool {
    !name.is_empty()
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

/// A person's display name: 1–64 ASCII letters, digits, `-`, `_` or `.`
/// (the join-request rule, so it survives `@mention`), and never a rank or
/// title word.
pub fn validate_display_name(name: &str) -> Result<&str, NameError> {
    let name = name.trim();
    if name.is_empty()
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
    } else if issued_by == "operator" || issued_by.starts_with("operator:") {
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
            dms: Arc::new(Mutex::new(HashMap::new())),
            operators: Arc::new(Mutex::new(HashMap::new())),
            groups: Arc::new(Mutex::new(groups::GroupBook::default())),
            next_id: Arc::new(AtomicU64::new(1)),
            condition_generation: Arc::new(AtomicU64::new(0)),
            now_ms: default_now_ms,
            admin_token: Arc::new(admin_token),
            room_token: Arc::new(room_token),
            store,
            default_settings,
            epoch,
            members: Arc::new(Mutex::new(HashMap::new())),
            member_keys: Arc::new(Mutex::new(HashMap::new())),
            memory_claims: Arc::new(Mutex::new(claims::MemoryClaims::default())),
            smasters: Arc::new(Mutex::new(HashMap::new())),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            admin_pairing: Arc::new(Mutex::new(AdminPairing {
                code: Self::secure_token("pair_", 16),
                session_ttl_ms: Self::ADMIN_SESSION_TTL_MS,
                expires_at_ms: pairing_now.saturating_add(Self::ADMIN_PAIRING_TTL_MS),
                log_id: None,
            })),
            join_create_times: Arc::new(Mutex::new(std::collections::HashMap::new())),
            require_sessions,
            loca_capacity: Arc::new(std::sync::atomic::AtomicUsize::new(
                Self::DEFAULT_LOCA_CAPACITY,
            )),
            loca_capacity_max: Arc::new(std::sync::atomic::AtomicUsize::new(
                Self::LOCA_CAPACITY_MAX,
            )),
            notification_send_limit: Arc::new(std::sync::atomic::AtomicU32::new(
                NOTIFICATION_SEND_LIMIT,
            )),
            require_invite,
            invites: Arc::new(Mutex::new(HashMap::new())),
            deliveries: Arc::new(Mutex::new(HashMap::new())),
            lobby_tx,
            member_lifecycle: Arc::new(Mutex::new(())),
            name_claims: Arc::new(Mutex::new(())),
            moderation_ranks: Arc::new(Mutex::new(HashMap::new())),
            lobby_online: Arc::new(Mutex::new(HashMap::new())),
            lobby_kits: Arc::new(Mutex::new(HashMap::new())),
            runtime_health: Arc::new(Mutex::new(HashMap::new())),
            care_deliveries: Arc::new(Mutex::new(HashMap::new())),
            home_room: Arc::new(home_room),
            reserved_room: Arc::new(reserved_room),
            caretakers: Arc::new(caretakers),
            #[cfg(test)]
            reissue_swapped_hook: Arc::new(Mutex::new(None)),
        };

        // Invites survive restarts — load them before anyone knocks. An
        // unreadable table stops the start: it must not read as "no invites".
        {
            let mut inv = hub.invites.lock_or_recover();
            for i in hub
                .store
                .load_invites()
                .expect("persistent invites are unreadable; refusing to start")
            {
                inv.insert(i.secret_hash.clone(), i);
            }
            // The deliveries still owed: `Store::open` has already cleared
            // every copy whose window closed while the server was stopped.
            let mut held = hub.deliveries.lock_or_recover();
            for invite_key in hub
                .store
                .load_invite_deliveries()
                .expect("persistent invite deliveries are unreadable; refusing to start")
            {
                let hash = secret_hash(&invite_key);
                if inv.contains_key(&hash) {
                    held.insert(hash, invite_key);
                }
            }
        }

        // Restore persisted state.
        let mut max_id = hub
            .store
            .max_message_id()
            .expect("persistent message high-water mark is unreadable; refusing to start");
        let mut max_generation = 0u64;
        {
            let mut rooms = hub.rooms.lock_or_recover();
            for snap in hub
                .store
                .load()
                .expect("persistent room state is unreadable; refusing to start")
            {
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
                room.journal = hub
                    .store
                    .load_journal(&snap.room)
                    .expect("persistent journal is unreadable; refusing to start");
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
            for m in hub
                .store
                .load_members()
                .expect("persistent memberships are unreadable; refusing to start")
            {
                mem.insert(m.id.clone(), m);
            }
        }
        {
            // Only after `Store::open` committed any registry migration. An
            // unreadable registry stops the start: it must not read as "no
            // Smasters".
            let mut sm = hub.smasters.lock_or_recover();
            for record in hub
                .store
                .load_smasters()
                .expect("persistent Smaster registry is unreadable; refusing to start")
            {
                sm.insert(record.secret_hash.clone(), record);
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
            for record in hub
                .store
                .load_admin_sessions(hub.now())
                .expect("persisted admin sessions are unreadable; refusing to start")
            {
                let crate::store::AdminSessionRecord {
                    secret_hash: session_hash,
                    expires_at,
                } = record;
                let Some(principal) = hub
                    .store
                    .principal_for_session_by_hash(&session_hash, hub.now())
                else {
                    // Unbound legacy sessions cannot prove their original role.
                    // Fail closed rather than promoting them to the current Master.
                    hub.store
                        .delete_admin_session_by_hash(&session_hash)
                        .expect("invalid persisted session cleanup failed; refusing to start");
                    continue;
                };
                if !matches!(principal.role, BuildingRole::Master | BuildingRole::Smaster) {
                    hub.store
                        .delete_admin_session_by_hash(&session_hash)
                        .expect("non-admin persisted session cleanup failed; refusing to start");
                    continue;
                }
                sessions.insert(
                    session_hash,
                    SessionIdentity {
                        principal_id: Some(principal.id),
                        building_role: principal.role,
                        name: principal.display_name,
                        kind: principal.kind,
                        loca: None,
                        member: None,
                        invite: None,
                        building_key: false,
                        admin: true,
                        expires_at,
                    },
                );
            }
        }

        // Migration: an invite now seats a MEMBER, but legacy invites predate the
        // link (empty `member_id`). Bind each to the membership carrying its
        // name; when none exists, mint one marked `migration:legacy-invite` —
        // the invite-only identities of the old model become real members, so
        // no one loses a seat they already held. Such a membership's key is
        // never handed to anyone: its holder enters with the invite it holds.
        // After this pass every live invite points at a membership and the new
        // invariant holds building-wide. A write that fails stops the start.
        {
            let mut inv = hub.invites.lock_or_recover();
            let mut mem = hub.members.lock_or_recover();
            let orphans: Vec<String> = inv
                .values()
                .filter(|i| i.member_id.is_empty())
                .map(|i| i.secret_hash.clone())
                .collect();
            for hash in orphans {
                let (name, kind) = {
                    let i = &inv[&hash];
                    (i.name.clone(), i.kind.clone())
                };
                let member_id = match mem.values().find(|m| m.name == name) {
                    Some(m) => m.id.clone(),
                    None => {
                        let key = hub.new_invite_token().replacen("dv_", "mb_", 1);
                        let m = hub
                            .store
                            .add_member(&key, &name, &kind, hub.now(), "migration:legacy-invite")
                            .expect(
                                "binding a legacy invite to a new membership failed; refusing to start",
                            );
                        let id = m.id.clone();
                        mem.insert(id.clone(), m);
                        id
                    }
                };
                hub.store
                    .bind_invite_member_by_hash(&hash, &member_id)
                    .expect("binding a legacy invite to its membership failed; refusing to start");
                if let Some(i) = inv.get_mut(&hash) {
                    i.member_id = member_id;
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
                .map(|invite| invite.secret_hash.clone())
                .collect();
            for hash in stale {
                hub.store
                    .revoke_invite_by_hash(&hash, hub.now())
                    .expect("persist reserved-loca seat cleanup");
                hub.invites.lock_or_recover().remove(&hash);
            }
        }

        // The configured home room always exists.
        // It may be created here rather than restored from a snapshot (a loca
        // with a journal but no messages leaves no snapshot), so its journal
        // has to be loaded either way — otherwise a restart forgets the record.
        {
            let mut rooms = hub.rooms.lock_or_recover();
            loca_name::require_home_rule(&rooms, &hub.home_room);
            let room = rooms
                .entry((*hub.home_room).clone())
                .or_insert_with(|| Room::with(ChatMode::Free, hub.default_settings.clone(), 1));
            if room.journal.is_empty() {
                room.journal = hub
                    .store
                    .load_journal(&hub.home_room)
                    .expect("persistent home journal is unreadable; refusing to start");
                room.next_journal_id = room.journal.last().map(|e| e.id + 1).unwrap_or(1);
            }
        }

        // The Lobby room always exists: everyone in the Building lands there.
        hub.rooms
            .lock_or_recover()
            .entry(LOBBY_ROOM.to_string())
            .or_insert_with(|| Room::with(ChatMode::Free, hub.default_settings.clone(), 1));

        // Bans and mutes survive the restart (PRINCIPLES: "a restart does not
        // kill the room"). Load them into their rooms so the door is exactly as the
        // operator left it — otherwise a banned name's invite reloads and walks
        // back in.
        {
            let mut rooms = hub.rooms.lock_or_recover();
            for record in hub
                .store
                .load_bans()
                .expect("moderation state is unreadable; refusing to start")
            {
                let crate::store::ModerationRecord {
                    room,
                    name,
                    kind,
                    authority_rank,
                } = record;
                hub.moderation_ranks
                    .lock_or_recover()
                    .insert((room.clone(), name.clone(), kind.clone()), authority_rank);
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
            for room in hub
                .store
                .sealed_rooms()
                .expect("persistent room seals are unreadable; refusing to start")
            {
                deleted.insert(room);
            }
        }
        {
            let mut dms = hub.dms.lock_or_recover();
            for b in hub
                .store
                .load_dms()
                .expect("persistent direct messages are unreadable; refusing to start")
            {
                dms.insert(b.room.clone(), b);
            }
        }
        // After the legacy operator migration above, so it sees its result.
        *hub.operators.lock_or_recover() = hub
            .store
            .active_loca_operators()
            .expect("persistent operator assignments are unreadable; refusing to start")
            .into_iter()
            .collect();
        hub.load_groups();
        hub
    }

    /// True if `token` authorizes admin actions. When no token is configured,
    /// any request is treated as admin (dev mode).
    /// May this token act — issue invites, run a loca, change settings?
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
            // An invite proves identity only inside one room. Never turn that
            // proof into Building/profile authority (including key issuance).
            if session.loca.is_some() {
                return RequestAuthority {
                    principal_id: None,
                    display_name: Some(session.name),
                    kind: Some(session.kind),
                    building_role: Some(BuildingRole::Member),
                };
            }
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

    /// The principal's own appearance preferences (defaults when unsaved).
    pub fn preferences_for(&self, principal_id: &str) -> crate::store::Preferences {
        self.store.principal_preferences(principal_id)
    }

    /// Validate and save a partial preference update for this principal.
    pub fn update_preferences(
        &self,
        principal_id: &str,
        patch: &serde_json::Value,
    ) -> Result<crate::store::Preferences, crate::store::PreferenceError> {
        self.store
            .merge_principal_preferences(principal_id, patch, self.now())
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

    /// The id (`cr_`) of the live credential this secret is, if any.
    pub fn credential_id_for_secret(&self, credential: &str) -> Option<String> {
        self.store.credential_id_for_secret(credential)
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
        self.sessions.lock_or_recover().retain(|session_hash, _| {
            !self
                .store
                .session_uses_credential_by_hash(session_hash, credential_id)
        });
        // Door B: the Lobby sockets opened with this credential close. Its
        // reconnect is refused at the handshake (the credential is revoked).
        let _ = self.lobby_tx.send(LobbyEvent::CredentialRevoked {
            credential_id: credential_id.to_string(),
        });
        // v2 authority is DB-backed, but a legacy Smaster key also lives in
        // the in-memory compatibility map. Retire that fallback only after the
        // credential transaction commits, so "revoked" cannot still mean
        // "Smaster through the old path".
        if let Some((role, hash)) = legacy {
            if role == "smaster" {
                self.smasters.lock_or_recover().remove(&hash);
            }
        }
        Ok(())
    }

    /// The live Smaster a presented code belongs to. The code is hashed and
    /// looked up by its hash; nothing else (the hash itself, a `pr_` or a
    /// `smid_`) is a code.
    pub fn smaster_for_code(&self, code: Option<&str>) -> Option<crate::store::SmasterRecord> {
        let code = code.filter(|code| !code.is_empty())?;
        self.smasters
            .lock_or_recover()
            .get(&crate::store::secret_hash(code))
            .cloned()
    }

    /// The smaster behind this token, if it is a live one.
    pub fn smaster_name(&self, token: Option<&str>) -> Option<String> {
        self.smaster_for_code(token).map(|record| record.name)
    }

    /// Mint a second master. Only the master may call this — the caller checks.
    /// The code is returned once, to the response that hands it over. A name
    /// someone already wears in any letter case (`rank_name_clashes`) is
    /// refused; the check and the write happen under `name_claims`.
    pub fn add_smaster(&self, name: &str) -> Result<String, NameClaimError> {
        let _claim = self.name_claims.lock_or_recover();
        if self.rank_name_clashes(name) {
            return Err(NameClaimError::Worn);
        }
        let code = self.new_invite_token().replacen("dv_", "sm_", 1);
        let now = (self.now_ms)();
        let record = self.store.add_smaster(&code, name, now)?;
        self.smasters
            .lock_or_recover()
            .insert(record.secret_hash.clone(), record);
        Ok(code)
    }

    /// Take one back, by its code's hash. The row survives, so "who could
    /// act, and when" stays answerable.
    fn revoke_smaster_hash(&self, secret_hash: &str) -> rusqlite::Result<bool> {
        let Some(record) = self.smasters.lock_or_recover().get(secret_hash).cloned() else {
            return Ok(false);
        };
        self.store.revoke_smaster(secret_hash, (self.now_ms)())?;
        self.smasters.lock_or_recover().remove(secret_hash);
        // A group whose founder this was now belongs to the Master.
        self.pass_groups_to_master(&record.principal_id);
        Ok(true)
    }

    /// The live Smaster a management reference names: its `smid_`, or the
    /// Smaster's own code presented by its holder (hashed, then compared).
    fn smaster_by_reference(&self, reference: &str) -> Option<crate::store::SmasterRecord> {
        if reference.is_empty() {
            return None;
        }
        let hash = crate::store::secret_hash(reference);
        let smasters = self.smasters.lock_or_recover();
        smasters.get(&hash).cloned().or_else(|| {
            smasters
                .values()
                .find(|record| record.management_id == reference)
                .cloned()
        })
    }

    /// Revoke the Smaster a management reference names (`smid_…`, or the
    /// code itself).
    pub fn revoke_smaster_ref(&self, reference: &str) -> rusqlite::Result<bool> {
        match self.smaster_by_reference(reference) {
            Some(record) => self.revoke_smaster_hash(&record.secret_hash),
            None => Ok(false),
        }
    }

    /// Live Smasters. Never a code: a record carries its hash and ids.
    pub fn smasters(&self) -> Vec<crate::store::SmasterRecord> {
        self.smasters.lock_or_recover().values().cloned().collect()
    }

    /// True if `token` may connect / post. Open when no room token is set; the
    /// admin token also counts (admins are members).
    pub fn is_member(&self, token: Option<&str>) -> bool {
        if self.room_token.is_empty() {
            // No building key. In invite-only mode this is not an open house —
            // the door is closed and only an invite (checked per-loca elsewhere)
            // opens it. Being a "member of the building" now means holding the
            // admin token; everyone else needs an invite for the specific loca.
            return !self.require_invite
                || (!self.admin_token.is_empty() && token == Some(self.admin_token.as_str()));
        }
        token == Some(self.room_token.as_str())
            || (!self.admin_token.is_empty() && token == Some(self.admin_token.as_str()))
    }

    /// `is_member` for the key whose hash is `key_hash`: the configured
    /// building and admin keys are hashed and compared with it.
    fn is_member_key_hash(&self, key_hash: &str) -> bool {
        let is = |configured: &str| !configured.is_empty() && secret_hash(configured) == key_hash;
        if self.room_token.is_empty() {
            return !self.require_invite || is(&self.admin_token);
        }
        is(&self.room_token) || is(&self.admin_token)
    }

    /// True if `token` is an invite for THIS loca. An invite opens one loca and no
    /// other — that is the whole point of it (DESIGN §5g).
    /// A presented invite, whichever loca it belongs to: hashed and looked up
    /// by its hash. Nothing else (the hash itself, an `ivid_`) is an invite.
    pub fn invite_by_token(&self, token: &str) -> Option<InviteRecord> {
        if token.is_empty() {
            return None;
        }
        let invite = self
            .invites
            .lock_or_recover()
            .get(&secret_hash(token))
            .cloned()?;
        if !invite.member_id.is_empty() && self.member_suspended(&invite.member_id) {
            return None;
        }
        Some(invite)
    }

    /// Is the invite whose hash is `invite_hash` still live, its member not
    /// suspended? `invite_by_token` for a caller that holds the hash.
    fn invite_live_by_hash(&self, invite_hash: &str) -> bool {
        let Some(invite) = self.invites.lock_or_recover().get(invite_hash).cloned() else {
            return false;
        };
        invite.member_id.is_empty() || !self.member_suspended(&invite.member_id)
    }

    pub fn invite_for(&self, room: &str, token: Option<&str>) -> Option<InviteRecord> {
        let token = token?;
        let inv = self.invite_by_token(token)?;
        (inv.room == room).then_some(inv)
    }

    /// Resolve the canonical principal carried by an active invite for this
    /// loca. Native listeners commonly authenticate their WebSocket directly
    /// with the invite instead of first minting a session; principal-scoped
    /// Everyone Care must work for both credential forms.
    pub fn principal_id_for_invite(&self, room: &str, token: Option<&str>) -> Option<String> {
        let invite = self.invite_for(room, token)?;
        self.member_principal(&invite.member_id)
    }

    /// May this room credential enter `room`? An active per-device credential
    /// inherits the transitional building-key access of its durable Member
    /// record; loca-specific invites remain separate, scoped bearers.
    /// That indirection is important: revoking the original legacy bearer must
    /// close the door, while another credential of the same principal keeps
    /// the principal's inherited building-key access.
    pub fn may_enter(&self, room: &str, token: Option<&str>) -> bool {
        self.invite_for(room, token).is_some() || self.reaches_building(token)
    }

    /// Does this room credential reach every ordinary loca through the
    /// building key (`may_enter` without an invite)?
    pub fn reaches_building(&self, token: Option<&str>) -> bool {
        let Some(token) = token else {
            return self.is_member(None);
        };
        if self.store.credential_is_revoked(token) {
            return false;
        }
        if let Some(member) = self.member_for_credential(Some(token)) {
            // What the building key's rule answers for the membership's own
            // key, asked by that key's hash: an installation upgraded with
            // ROOM_TOKEN set to a member's key still reaches the building
            // with every credential of that member.
            return self.is_member_key_hash(&member.secret_hash);
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
    ///   - `invite_tok`: an invite or the building key (`x-room-token`)
    ///   - `session`:   an identity taken earlier (`x-session-token`)
    ///
    /// A `name_for_ban` hint lets the WS door pass the query `?name=` when there
    /// is no session yet; REST passes `None` and the ban is judged from whatever
    /// identity the credentials resolve to.
    pub fn enter_decision(
        &self,
        room: &str,
        admin_tok: Option<&str>,
        invite_tok: Option<&str>,
        session: Option<&SessionIdentity>,
        name_hint: Option<&str>,
    ) -> EnterDecision {
        // Rank first, and rank ignores bans: the master is never banned from
        // their own building. master + smaster, resolved in ONE place — by the
        // raw admin key OR a live admin session (the browser holds the session,
        // not the key). Without the session branch the door turned the master
        // away in invite mode: the admin session's loca is None, so it matched no
        // loca and every room read/WS handshake 401'd.
        let credential_authority = self.resolve_authority(admin_tok, None);
        if session.is_some_and(|s| s.loca.as_deref().is_some_and(|scope| scope != room)) {
            return EnterDecision::Denied;
        }
        if self.is_lobby_room(room) {
            return self.lobby_door(
                &credential_authority,
                admin_tok,
                invite_tok,
                session,
                name_hint,
            );
        }
        // A direct message and a group loca come BEFORE the rank bypass below: no
        // Smaster reaches either by rank (see `private_door`).
        match self.private_door(
            room,
            &credential_authority,
            admin_tok,
            invite_tok,
            session,
            name_hint,
        ) {
            PrivateDoor::Private(decision) => return decision,
            PrivateDoor::Reserved => return EnterDecision::Denied,
            PrivateDoor::Ordinary => {}
        }
        if credential_authority.is_building_admin() && admin_tok.is_some() {
            return EnterDecision::Allowed;
        }
        if session.map(|s| s.admin).unwrap_or(false) {
            return EnterDecision::Allowed;
        }

        let ban_name = self.door_name(invite_tok, session, &credential_authority, name_hint);
        if let Some(n) = &ban_name {
            if self.is_banned(room, n) {
                return EnterDecision::Banned;
            }
        }

        let principal_at_door = session
            .and_then(|identity| identity.principal_id.as_deref())
            .or(credential_authority.principal_id.as_deref());
        if principal_at_door
            .is_some_and(|principal_id| self.is_operator_principal(room, principal_id))
        {
            return EnterDecision::Allowed;
        }

        // An invite for THIS loca, or the building key (is_member) — may_enter
        // already folds both. An invite for another loca opens nothing.
        if self.may_enter(room, invite_tok) {
            return EnterDecision::Allowed;
        }

        // A person signed in with their own key holds a live invite for this
        // loca: the invite is their seat, whichever credential they came with.
        if let Some(principal) = session.and_then(|s| s.principal_id.as_deref()) {
            if self.principal_holds_invite(room, principal) {
                return EnterDecision::Allowed;
            }
        }

        // A session opens the loca it was minted for.
        if let Some(s) = session {
            match &s.loca {
                Some(l) if l == room => return EnterDecision::Allowed,
                Some(_) => {}
                // A session with no loca was taken with a Building-level
                // credential. Only one taken with the building key itself
                // (`building_key`) reaches the whole building, exactly as the
                // key does: the browser takes a session once and stops sending
                // the key. A session taken with a member's own key or profile
                // credential is that member signed in — it got everything it
                // is entitled to from the rules above and nothing from the key.
                // In invite-only mode (no ROOM_TOKEN) there is no building key,
                // so a loca-less session reaches nothing through it.
                None if s.building_key && !self.room_token.is_empty() => {
                    return EnterDecision::Allowed
                }
                None => {}
            }
        }

        EnterDecision::Denied
    }

    /// Whose name is at this door? A session names itself; otherwise an invite
    /// carries a name; otherwise the caller's ?name= hint. This is the name a
    /// ban is checked against.
    fn door_name(
        &self,
        invite_tok: Option<&str>,
        session: Option<&SessionIdentity>,
        credential_authority: &RequestAuthority,
        name_hint: Option<&str>,
    ) -> Option<String> {
        session
            .map(|s| s.name.clone())
            .or_else(|| {
                invite_tok
                    .and_then(|t| self.invite_by_token(t))
                    .map(|i| i.name)
            })
            .or_else(|| credential_authority.display_name.clone())
            .or_else(|| name_hint.map(str::to_string))
    }

    /// The door of a loca whose very existence is private: a direct message, the
    /// direct message id space, a group loca. Everything here that costs anything
    /// (the invite's principal, which may read SQLite; the direct message and group
    /// lookups) is read before the answer depends on what the id names, so
    /// an id that names nothing costs this door what a hidden loca costs and
    /// timing does not tell them apart (PRINCIPLES "Birebir", "Groups").
    fn private_door(
        &self,
        room: &str,
        credential_authority: &RequestAuthority,
        admin_tok: Option<&str>,
        invite_tok: Option<&str>,
        session: Option<&SessionIdentity>,
        name_hint: Option<&str>,
    ) -> PrivateDoor {
        let pair = self.dm(room);
        // An agent at the door may hold an invite for this very loca: the invite
        // the opener's call minted for a direct message, or one for a group loca.
        let invited = self.principal_id_for_invite(room, invite_tok);
        let mut principals: Vec<&str> = Vec::new();
        if let Some(s) = session {
            // An invite session stays confined to the loca its invite opened.
            if s.loca.as_deref().is_none_or(|l| l == room) {
                principals.extend(s.principal_id.as_deref());
            }
        }
        principals.extend(credential_authority.principal_id.as_deref());
        principals.extend(invited.as_deref());
        let group_member = self.group_gate(room, &principals);

        // A direct message belongs to its two people: no Master or Smaster reaches
        // one by rank, and nobody else learns it exists (PRINCIPLES
        // "Birebir"). The other side may be an agent holding the invite the
        // opener's call minted for this room, or a session taken with it.
        if let Some(b) = pair {
            let principal = session
                .and_then(|s| s.principal_id.as_deref())
                .or(credential_authority.principal_id.as_deref());
            let party =
                principal.is_some_and(|p| b.has(p)) || invited.as_deref().is_some_and(|p| b.has(p));
            return PrivateDoor::Private(if party {
                EnterDecision::Allowed
            } else {
                EnterDecision::Denied
            });
        }
        // The direct message id space is reserved: nobody reaches (and so nobody
        // creates by joining or posting) an ordinary loca there, so a pair's
        // room can never be squatted with someone else's invites in it.
        if room.starts_with(Self::DM_PREFIX) {
            return PrivateDoor::Reserved;
        }
        // A group loca belongs to its group: a Smaster outside the group is
        // an outsider here, and only the Master reaches it by rank
        // (PRINCIPLES "Groups").
        match group_member {
            Some(member) => PrivateDoor::Private(self.group_door(
                room,
                credential_authority,
                admin_tok,
                invite_tok,
                session,
                name_hint,
                member,
            )),
            None => PrivateDoor::Ordinary,
        }
    }

    /// The door's answer for a direct message or a group loca, `None` for any other
    /// id; the same answer `enter_decision` gives. A door asks this where it
    /// decides whether the loca exists, so a caller who may not enter one is
    /// refused there exactly as for an id that names nothing, after the same
    /// work (see `private_door`), and no later fence is first to refuse.
    pub fn private_loca_decision(
        &self,
        room: &str,
        admin_tok: Option<&str>,
        invite_tok: Option<&str>,
        session: Option<&SessionIdentity>,
    ) -> Option<EnterDecision> {
        let credential_authority = self.resolve_authority(admin_tok, None);
        let scoped_elsewhere =
            session.is_some_and(|s| s.loca.as_deref().is_some_and(|scope| scope != room));
        match self.private_door(
            room,
            &credential_authority,
            admin_tok,
            invite_tok,
            session,
            None,
        ) {
            PrivateDoor::Private(_) if scoped_elsewhere => Some(EnterDecision::Denied),
            PrivateDoor::Private(decision) => Some(decision),
            PrivateDoor::Reserved | PrivateDoor::Ordinary => None,
        }
    }

    /// The door of a group loca. The Master enters by rank; anyone else only
    /// as a member of the loca's group (`member`, from `private_door`: by the
    /// principal of their session or credential, or, for an agent, of the
    /// invite it holds for this loca). A member still answers to the loca's
    /// bans.
    #[allow(clippy::too_many_arguments)]
    fn group_door(
        &self,
        room: &str,
        credential_authority: &RequestAuthority,
        admin_tok: Option<&str>,
        invite_tok: Option<&str>,
        session: Option<&SessionIdentity>,
        name_hint: Option<&str>,
        member: bool,
    ) -> EnterDecision {
        let master = (credential_authority.is_master()
            && (admin_tok.is_some() || self.admin_open()))
            || (!self.admin_token.is_empty() && invite_tok == Some(self.admin_token.as_str()))
            || session.is_some_and(|s| s.admin && s.building_role == BuildingRole::Master);
        if master {
            return EnterDecision::Allowed;
        }
        if !member {
            return EnterDecision::Denied;
        }
        if let Some(n) = self.door_name(invite_tok, session, credential_authority, name_hint) {
            if self.is_banned(room, &n) {
                return EnterDecision::Banned;
            }
        }
        EnterDecision::Allowed
    }

    /// The seat key for a connection — one key = one seat. Derived from the
    /// strongest credential at the door, so the same key entering under two
    /// display names lands on ONE seat (the name is just the label).
    ///
    /// Precedence mirrors `enter_decision`: rank first (the master is the
    /// master even if they also hold an invite), then the invite's member, then
    /// the session, then the bare name.
    ///
    /// In open dev mode (no ADMIN_TOKEN) rank identifies nobody, so identity
    /// falls through to the name — two differently-named anons stay distinct,
    /// exactly as before.
    ///
    /// The seat key travels in `Evicted` frames, so it names a membership by
    /// its management id (`mb:<mbid_>`) and an unbound legacy invite by its id
    /// (`dv:<ivid_>`), never by the key or the invite.
    pub fn seat_identity(
        &self,
        admin_tok: Option<&str>,
        invite_tok: Option<&str>,
        session: Option<&SessionIdentity>,
        session_tok: Option<&str>,
        name: &str,
    ) -> String {
        if let Some(t) = admin_tok {
            if let Some(smaster) = self.smaster_for_code(Some(t)) {
                return format!("sm:pr:{}", smaster.principal_id);
            }
            if !self.admin_token.is_empty() && t == self.admin_token.as_str() {
                // ONE fixed key for the master: whatever name they type, the
                // seat is the same. Per-room, because each roster is per-room.
                return "@master".into();
            }
        }
        if let Some(inv) = invite_tok.and_then(|t| self.invite_by_token(t)) {
            if !inv.member_id.is_empty() {
                // Two invites of the same member (or the same invite twice) are
                // one person — the membership is the identity.
                return format!("mb:{}", inv.member_id);
            }
            return format!("dv:{}", inv.id);
        }
        if let Some(member) = self.member_for_credential(invite_tok) {
            return format!("mb:{}", member.id);
        }
        if let Some(s) = session {
            if s.admin {
                // `admin` is true for every Building administrator, Smaster
                // included. A Smaster is a different person: give it the seat
                // of its own principal, or it would share (and keep evicting)
                // the Master's seat in every loca. Every Smaster seat starts
                // `sm:` so the roster can show the rank (`seat_rank`).
                if s.building_role == BuildingRole::Smaster {
                    if let Some(principal) = &s.principal_id {
                        return format!("sm:pr:{principal}");
                    }
                    if let Some(t) = session_tok {
                        // The seat key travels in `Evicted` frames: name the
                        // session by its hash, never by its token.
                        return format!("sm:st:{}", secret_hash(t));
                    }
                }
                // Every session minted for the one Master principal occupies the
                // same per-loca seat, regardless of the display name used
                // while taking the session.
                return "@master".into();
            }
            if let Some(m) = &s.member {
                return format!("mb:{m}");
            }
            if let Some(t) = session_tok {
                return format!("st:{}", secret_hash(t));
            }
        }
        format!("name:{name}")
    }

    /// Who an `op_id` belongs to (PRINCIPLES: a message that arrives twice
    /// does not produce its effect twice). The same sender gets the same key
    /// over REST and over the WebSocket, under any of its sessions and after
    /// a restart: the principal the door verified, read in the order the
    /// socket names its writer (the session's; else the invite's or the
    /// membership credential's; else the one behind an admin credential,
    /// which is the Master's for the root key and the Smaster's for its
    /// code). Without a principal (a memory-only store) the membership is
    /// the key, by its management id (`mb:<mbid_>`). With neither (the open
    /// sandbox, the shared building key) the display name and kind are all
    /// that names the sender; that key has a prefix no earlier version wrote,
    /// so it cannot meet an older row.
    ///
    /// `legacy` is the key the REST route stored before this version, built
    /// from the same request. It is only ever read, so an operation sent
    /// before the upgrade is still answered with its original message. That
    /// route keyed an invite session by `mb:<membership key>`; `Store::open`
    /// rewrites every such stored key to `mb:<mbid_>` (`rewrite_key_scopes`),
    /// which is what an invite session names its membership by now, so the
    /// same lookup finds it.
    pub fn operation_scope(
        &self,
        room: &str,
        admin_tok: Option<&str>,
        invite_tok: Option<&str>,
        session: Option<&SessionIdentity>,
        kind: SenderType,
        name: &str,
    ) -> OperationScope {
        let kind = match kind {
            SenderType::Agent => "agent",
            SenderType::User => "user",
        };
        let (principal, member) = match session {
            Some(session) => (session.principal_id.clone(), session.member.clone()),
            None => (
                self.principal_id_for_invite(room, invite_tok)
                    .or_else(|| {
                        self.member_for_credential(invite_tok)
                            .and_then(|member| member.principal_id)
                    })
                    .or_else(|| {
                        admin_tok
                            .filter(|token| !token.is_empty())
                            .and_then(|token| {
                                self.resolve_authority(Some(token), None).principal_id
                            })
                    })
                    .or_else(|| self.smaster_for_code(admin_tok).map(|s| s.principal_id)),
                self.invite_for(room, invite_tok)
                    .map(|invite| invite.member_id)
                    .filter(|member| !member.is_empty())
                    .or_else(|| self.member_for_credential(invite_tok).map(|m| m.id)),
            ),
        };
        let key = match (&principal, &member) {
            (Some(principal), _) => format!("pr:{principal}"),
            (None, Some(member)) => format!("mb:{member}"),
            (None, None) => format!("label:{kind}:{name}"),
        };
        let legacy = match session.and_then(|session| session.member.as_deref()) {
            Some(member) => format!("mb:{member}"),
            None => format!("{kind}:{name}"),
        };
        OperationScope {
            legacy: (legacy != key).then_some(legacy),
            key,
            principal,
        }
    }

    /// Is a record an earlier version stored under `legacy` this sender's?
    /// Proved, never assumed. A record that names its writer belongs to that
    /// writer. One that names none (every REST row before writers were
    /// recorded) is this sender's only when its key can have had one sender:
    /// a membership key (`mb:`). An unverified sender (the open sandbox, the
    /// shared Building key) is its label before and after, so the label is
    /// its proof. A display name proves nothing for a verified sender: two
    /// principals can bear one name, a revoked member's name can be admitted
    /// again, and a Master or Smaster rename leaves no history, so the one
    /// principal that bears a name today need not be the one that wrote
    /// under it. Such a record is `Unknown`; otherwise two senders could
    /// each be answered with the other's message and have their own dropped.
    fn legacy_operation_owner(
        scope: &OperationScope,
        legacy: &str,
        record: &Message,
    ) -> LegacyOwner {
        if let Some(writer) = record.sender_principal.as_deref() {
            return if scope.principal.as_deref() == Some(writer) {
                LegacyOwner::Sender
            } else {
                LegacyOwner::Other
            };
        }
        if scope.principal.is_none() || legacy.starts_with("mb:") {
            LegacyOwner::Sender
        } else {
            LegacyOwner::Unknown
        }
    }

    /// Admit somebody to the building. The founding act: it creates the
    /// identity every invite later refers to. Legacy admission remains idempotent by
    /// name, so admitting an existing member returns the existing principal
    /// rather than minting a duplicate membership. Authority still resolves
    /// from principal identity, never from display name or credential text.
    /// The two-step "admit & invite" flow can therefore be replayed safely.
    /// A new name must pass the doors' rule (`admission_name_refusal`) and
    /// must not be one someone already wears in any letter case
    /// (`new_member_name_clashes`). The lookup of an existing member, the
    /// rule and the write all happen under `name_claims`: two racing requests
    /// cannot both get in, two racing requests for the same exact name get
    /// the same member, and a member admitted before the rule is found again
    /// while it lasts, but its name is not admitted anew once it is revoked.
    pub fn admit_member(
        &self,
        name: &str,
        kind: &str,
        by: &str,
    ) -> Result<MemberRecord, AdmitError> {
        self.admit_member_once(name, kind, by)
            .map(|(member, _)| member)
    }

    /// `admit_member`, with the key when this call admitted the member
    /// (`Some`, the only time the key exists outside its holder) or `None`
    /// when it found the member already admitted: an existing member's key is
    /// not stored and cannot be read back out.
    pub fn admit_member_once(
        &self,
        name: &str,
        kind: &str,
        by: &str,
    ) -> Result<(MemberRecord, Option<String>), AdmitError> {
        let _claim = self.name_claims.lock_or_recover();
        if let Some(existing) = self.member_by_name(name) {
            return Ok((existing, None));
        }
        if let Some(rule) = admission_name_refusal(name) {
            return Err(AdmitError::Rule(rule));
        }
        if self.new_member_name_clashes(name) {
            return Err(AdmitError::Worn);
        }
        let key = self.new_invite_token().replacen("dv_", "mb_", 1);
        let m = self
            .store
            .add_member(&key, name, kind, (self.now_ms)(), by)?;
        if !self.store.is_persistent() {
            self.member_keys
                .lock_or_recover()
                .insert(secret_hash(&key), m.id.clone());
        }
        self.members
            .lock_or_recover()
            .insert(m.id.clone(), m.clone());
        Ok((m, Some(key)))
    }

    /// End a membership. Their invites are not touched here — losing the
    /// building is a decision of its own, and the record of where they sat
    /// stays readable.
    /// End a membership — and everything it authorized. PRINCIPLES: when a
    /// parent credential is revoked, every child dies with it. Losing the
    /// building means losing every seat (invite), every proof (session), and
    /// every open connection at once — otherwise "you no longer belong here"
    /// would leave the ejected person still reading and speaking. By the
    /// membership's id (`mbid_`).
    pub fn revoke_member(&self, member_id: &str) -> rusqlite::Result<bool> {
        let _lifecycle = self.member_lifecycle.lock_or_recover();
        if !self.members.lock_or_recover().contains_key(member_id) {
            return Ok(false);
        }
        let now = (self.now_ms)();
        self.store.revoke_member_cascade(member_id, now)?;
        let member_invites: Vec<InviteRecord> = self
            .invites
            .lock_or_recover()
            .values()
            .filter(|i| i.member_id == member_id)
            .cloned()
            .collect();
        {
            let mut invites = self.invites.lock_or_recover();
            for inv in &member_invites {
                invites.remove(&inv.secret_hash);
            }
        }
        for inv in member_invites {
            self.revoke_sessions_for_invite(&inv.room, &inv.member_id);
            self.close_connections(&inv.room, &inv.name);
        }
        // Every key its invites claimed stops working (the store revoked them
        // above; a memory-only Building holds them here), and so does its own.
        self.memory_claims
            .lock_or_recover()
            .forget_member(member_id);
        self.member_keys
            .lock_or_recover()
            .retain(|_, member| member != member_id);
        // Any session still bound to this member (its own key in a
        // memory-only store) dies too — the identity behind it is gone. The
        // membership leaves `members` before the sweep, so a session being
        // recorded concurrently sees it gone (`create_session_scoped`).
        self.members.lock_or_recover().remove(member_id);
        self.revoke_sessions_for_member(member_id);
        let _ = self.lobby_tx.send(LobbyEvent::MembershipRevoked {
            member: member_id.to_string(),
        });
        Ok(true)
    }

    pub fn set_member_suspended(
        &self,
        reference: &str,
        suspended: bool,
        reason: &str,
    ) -> rusqlite::Result<bool> {
        let _lifecycle = self.member_lifecycle.lock_or_recover();
        if !self.members.lock_or_recover().contains_key(reference) {
            return Ok(false);
        }
        let member_id = reference;
        if !suspended {
            return self.store.resume_member(member_id);
        }
        if !self.store.suspend_member(member_id, reason, self.now())? {
            return Ok(false);
        }
        let member_invites: Vec<InviteRecord> = self
            .invites
            .lock_or_recover()
            .values()
            .filter(|invite| invite.member_id == member_id)
            .cloned()
            .collect();
        {
            let mut invites = self.invites.lock_or_recover();
            for invite in &member_invites {
                invites.remove(&invite.secret_hash);
            }
        }
        for invite in member_invites {
            self.close_connections(&invite.room, &invite.name);
        }
        self.revoke_sessions_for_member(member_id);
        let _ = self.lobby_tx.send(LobbyEvent::MembershipSuspended {
            member: member_id.to_string(),
        });
        Ok(true)
    }

    /// Is the membership `member_id` suspended? A suspension that cannot be
    /// read counts as one (fail-closed).
    pub fn member_suspended(&self, member_id: &str) -> bool {
        self.store.member_suspended(member_id).unwrap_or(true)
    }

    pub fn member_suspension_supported(&self) -> bool {
        self.store.is_persistent()
    }

    /// End the membership whose management id (`mbid_`) is `reference`. A
    /// raw key is not a reference: it finds nothing, like any unknown one,
    /// so a key never has to travel in a URL.
    pub fn revoke_member_ref(&self, reference: &str) -> rusqlite::Result<bool> {
        self.revoke_member(reference)
    }

    /// The live, unsuspended membership whose id (`mbid_`) is `member_id`.
    pub fn member_by_id(&self, member_id: &str) -> Option<MemberRecord> {
        if member_id.is_empty() || self.member_suspended(member_id) {
            return None;
        }
        self.members.lock_or_recover().get(member_id).cloned()
    }

    /// Resolve an active per-device/member credential to its durable Building
    /// membership. The credential is hashed and looked up, never compared
    /// with anything stored: in a persistent Building through `credentials`
    /// (its principal, then the membership that principal is), in a
    /// memory-only one through `member_keys` and the keys invites claimed.
    /// Revoking the original bearer makes that bearer stop working, while
    /// another credential for the same principal still finds the same
    /// membership record.
    pub fn member_for_credential(&self, token: Option<&str>) -> Option<MemberRecord> {
        let token = token.filter(|token| !token.is_empty())?;
        if !self.store.is_persistent() {
            let hash = secret_hash(token);
            let member = self.member_keys.lock_or_recover().get(&hash).cloned();
            // A key an invite claimed in memory-only mode.
            let member =
                member.or_else(|| self.memory_claims.lock_or_recover().member_for_key(token))?;
            return self.member_by_id(&member);
        }
        let principal = self.store.principal_for_credential(token)?;
        if principal.role != BuildingRole::Member {
            return None;
        }
        self.members
            .lock_or_recover()
            .values()
            .find(|member| member.principal_id.as_deref() == Some(principal.id.as_str()))
            .cloned()
    }

    /// Is this a correct, unrevoked credential of a membership the Master has
    /// suspended (and not revoked)? Doors use it to answer "suspended" instead
    /// of "invalid", so an agent can wait for Resume. Only the holder of that
    /// member's own live key gets `true`: an unknown key, a revoked key and
    /// any key of a revoked membership (whose record has left `members`) all
    /// get `false`, so they stay indistinguishable from each other.
    pub fn credential_of_suspended_member(&self, token: Option<&str>) -> bool {
        let Some(token) = token else { return false };
        if !self.store.is_persistent() {
            return false;
        }
        let Some(principal) = self.store.disabled_member_principal_for_credential(token) else {
            return false;
        };
        // Revocation commits and leaves `members` under this lock: holding it,
        // a membership still listed is not one being revoked right now. The
        // record names the principal its key was bound to; the suspension is
        // read by the membership's id.
        let _lifecycle = self.member_lifecycle.lock_or_recover();
        let record = self
            .members
            .lock_or_recover()
            .values()
            .find(|member| member.principal_id.as_deref() == Some(principal.as_str()))
            .map(|member| member.id.clone());
        record.is_some_and(|id| self.store.member_suspended(&id).unwrap_or(false))
    }

    pub fn memberships(&self) -> Vec<MemberRecord> {
        self.members.lock_or_recover().values().cloned().collect()
    }

    /// The membership carrying this name, if the building knows them.
    pub fn member_by_name(&self, name: &str) -> Option<MemberRecord> {
        self.members
            .lock_or_recover()
            .values()
            .find(|m| m.name == name)
            .cloned()
    }

    /// Would a new member named `name` share it, without regard to letter
    /// case, with someone already in the Building: a live member, the Master,
    /// a Smaster, or a configured caretaker under another spelling? `@Bob`
    /// in a message wakes `bob` too, and a target of `LOCA-CARE` summons the
    /// caretaker `loca-care` (`addressed_caretakers`), so such a name is
    /// refused at every admission door. A caretaker's own exact name is how
    /// that caretaker is admitted, so it is no clash. Compared with
    /// `store::search_fold`, the fold `validate_display_name` uses.
    pub fn new_member_name_clashes(&self, name: &str) -> bool {
        let folded = crate::store::search_fold(name);
        self.caretakers
            .iter()
            .any(|caretaker| caretaker != name && crate::store::search_fold(caretaker) == folded)
            || self.name_worn_by_member_or_rank(&folded)
    }

    /// Would a new Smaster named `name` share it, without regard to letter
    /// case, with a member, the Master, another Smaster or a configured
    /// caretaker (its exact spelling included: a rank is not a caretaker)?
    pub fn rank_name_clashes(&self, name: &str) -> bool {
        let folded = crate::store::search_fold(name);
        self.caretaker_name_folds_to(&folded) || self.name_worn_by_member_or_rank(&folded)
    }

    /// Is a configured caretaker's name `folded` once folded?
    fn caretaker_name_folds_to(&self, folded: &str) -> bool {
        self.caretakers
            .iter()
            .any(|caretaker| crate::store::search_fold(caretaker) == folded)
    }

    /// Does a live member, the Master or a Smaster wear a name that folds to
    /// `folded`? The Master's name is known only from the stored profiles,
    /// so a memory-only Building checks members and Smasters alone.
    fn name_worn_by_member_or_rank(&self, folded: &str) -> bool {
        let worn = |other: &str| crate::store::search_fold(other) == folded;
        // One lock at a time: each guard ends with its own statement.
        let by_member = self
            .members
            .lock_or_recover()
            .values()
            .any(|member| worn(&member.name));
        let by_smaster = self
            .smasters
            .lock_or_recover()
            .values()
            .any(|record| worn(&record.name));
        by_member
            || by_smaster
            || self.profiles().iter().any(|profile| {
                profile.role != crate::store::BuildingRole::Member && worn(&profile.display_name)
            })
    }

    /// Does this member already hold a live invite for this loca?
    /// Does this principal hold a live invite for `room`?
    pub fn principal_holds_invite(&self, room: &str, principal: &str) -> bool {
        let members: Vec<String> = self
            .invites
            .lock_or_recover()
            .values()
            .filter(|i| i.room == room)
            .map(|i| i.member_id.clone())
            .collect();
        members
            .iter()
            .any(|m| self.member_principal(m).as_deref() == Some(principal))
    }

    /// Does the membership `member_id` hold a live invite for `room`?
    pub fn has_active_invite(&self, room: &str, member_id: &str) -> bool {
        self.invites
            .lock_or_recover()
            .values()
            .any(|i| i.room == room && i.member_id == member_id)
    }

    const RUNTIME_SEEN_TTL_MS: u64 = 20_000;
    const RUNTIME_WAKE_GRACE_MS: u64 = 10_000;
    const RUNTIME_ACK_GRACE_MS: u64 = 360_000;

    /// Which loca does this invite open, if it is one? The presented invite
    /// is hashed and looked up.
    pub fn invite_room(&self, token: &str) -> Option<String> {
        self.invites
            .lock_or_recover()
            .get(&secret_hash(token))
            .map(|i| i.room.clone())
    }

    /// Is this token a live invite at all — for any loca? Used by the blanket
    /// gate, which cannot know the target loca; the per-loca decision is
    /// `may_enter`.
    pub fn is_invite_token(&self, token: Option<&str>) -> bool {
        token.is_some_and(|t| self.invite_by_token(t).is_some())
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

    /// Mint an invite token from the operating system CSPRNG. Invites, memberships
    /// and smaster credentials are long-lived bearer authority, so a seeded
    /// hash/clock/counter construction is not an acceptable source of entropy.
    pub fn new_invite_token(&self) -> String {
        Self::secure_token("dv_", 32)
    }

    /// THE one way an invite comes to exist. Both doors — the terminal
    /// (`POST /invites` via invite.sh) and the UI (`+ call` via
    /// `POST /rooms/:id/call`) — land here, so the rules cannot drift apart
    /// again: an invite seats an EXISTING member (it never creates identity),
    /// and one member holds at most one live invite per loca.
    ///
    /// Capacity stays a door rule (`join`), not an invite rule: "a full loca
    /// refuses the eighth even with an invite" is the tested behaviour — more
    /// invites than seats may exist, the seats decide.
    ///
    /// `member_id` is the membership's id (`mbid_`). The answer is the
    /// invite's record and the invite itself, handed over once here.
    pub fn invite_member_to_room(
        &self,
        member_id: &str,
        room: &str,
        issued_by: &str,
    ) -> Result<(InviteRecord, String), InviteError> {
        self.invite_member_to_room_inner(member_id, room, issued_by, false)
    }

    // The only private-pair exception is the internal human-to-agent call.
    // Public HTTP, Lobby call and the SSH console all use the non-exception path.
    fn invite_member_to_room_inner(
        &self,
        member_id: &str,
        room: &str,
        issued_by: &str,
        private_agent_call: bool,
    ) -> Result<(InviteRecord, String), InviteError> {
        if let Some(pair) = self.dm(room) {
            let allowed = private_agent_call
                && self
                    .member_by_id(member_id)
                    .is_some_and(|m| m.kind == "agent")
                && self
                    .member_principal(member_id)
                    .is_some_and(|p| pair.has(&p));
            if !allowed {
                return Err(InviteError::PrivatePair);
            }
        }
        if self.is_deleted(room) {
            return Err(InviteError::Sealed);
        }
        if !self.room_exists(room)
            && (!self.admin_open() || self.refuse_implicit_loca(room).is_some())
        {
            return Err(InviteError::MissingRoom);
        }
        let Some(member) = self.member_by_id(member_id) else {
            return Err(InviteError::MemberNotFound);
        };
        if room == self.reserved_room.as_str() && !self.caretakers.contains(&member.name) {
            return Err(InviteError::Reserved);
        }
        if self.is_lobby_room(room) {
            return Err(InviteError::Lobby);
        }
        // The group check, one invite per person, no more invites than seats,
        // and the write below share the invite lock: two concurrent calls (or
        // a call racing a group move) cannot each pass a check and then both
        // mint.
        let lifecycle = self.member_lifecycle.lock_or_recover();
        // Nobody outside a group is invited or called into its loca: add them
        // to the group first (PRINCIPLES "Groups").
        if self.is_group_loca(room) {
            let principal = member.principal_id.clone();
            let candidates: Vec<&str> = principal.as_deref().into_iter().collect();
            if self.group_gate(room, &candidates) != Some(true) {
                return Err(InviteError::NotInGroup);
            }
        }
        if self.has_active_invite(room, member_id) {
            return Err(InviteError::AlreadyInvited);
        }
        // Model A: the seat IS the invite (PRINCIPLES: "Invited participant — has
        // a seat in one loca"). A loca seats at most `room_capacity(room)` (its own
        // number, or the Building's), so no more than that many live invites
        // may exist for it — the cap is checked here at mint, not only when
        // someone connects. (The join-time guard stays as defence in depth.)
        let active = self.invite_count(room);
        if active >= self.room_capacity(room) {
            return Err(InviteError::Full);
        }
        let invite = self.new_invite_token();
        let inv = InviteRecord::issued(
            &invite,
            room,
            &member.id,
            // Snapshot for the audit trail — identity lives in the membership.
            &member.name,
            &member.kind,
            self.now(),
            issued_by,
        );
        self.add_invite_locked(inv.clone(), &invite)
            .map_err(|_| InviteError::Storage)?;
        // Wake an already-running agent in the building lobby. Failure only
        // means nobody is listening at this instant; the invite is sent again
        // on their next Lobby connect while its delivery copy is held (until
        // its first use, or 24 hours after issue). Sent under the invite
        // lock, so a re-issue or revoke of this seat cannot slip in between
        // the mint and its delivery: the Lobby bus carries one seat's invites
        // in the order they became live (see `reissue_invite`).
        let _ = self.lobby_tx.send(LobbyEvent::Called {
            member: member.id,
            room: inv.room.clone(),
            token: invite.clone(),
        });
        drop(lifecycle);
        Ok((inv, invite))
    }

    /// Give a member a fresh invite for a seat it already holds. The old invite
    /// ends and the new one is minted in one store transaction, with the same
    /// member, loca, name, kind and issuer; the new one is then delivered
    /// over the member's Lobby line exactly like a call. The seat itself does
    /// not move: the swap happens under the invite lock, so the loca's invite
    /// count never drops and a concurrent call cannot take the place, and a
    /// full loca re-issues as readily as an empty one.
    ///
    /// Re-issuing ends an invite, so it needs the authority that ending it needs
    /// (`may_end_invite`): a Smaster cannot re-issue what the Master issued.
    /// When the store write fails nothing changes, in the store or here.
    pub fn reissue_invite(
        &self,
        member_id: &str,
        room: &str,
        actor: &RequestAuthority,
    ) -> Result<(InviteRecord, String), ReissueError> {
        // The refusals a call gives, in the same order (see
        // `invite_member_to_room_inner`); public routes never re-issue a
        // direct message's invite.
        if self.dm(room).is_some() {
            return Err(ReissueError::Invite(InviteError::PrivatePair));
        }
        if self.is_deleted(room) {
            return Err(ReissueError::Invite(InviteError::Sealed));
        }
        if !self.room_exists(room)
            && (!self.admin_open() || self.refuse_implicit_loca(room).is_some())
        {
            return Err(ReissueError::Invite(InviteError::MissingRoom));
        }
        let Some(member) = self.member_by_id(member_id) else {
            return Err(ReissueError::Invite(InviteError::MemberNotFound));
        };
        if room == self.reserved_room.as_str() && !self.caretakers.contains(&member.name) {
            return Err(ReissueError::Invite(InviteError::Reserved));
        }
        let lifecycle = self.member_lifecycle.lock_or_recover();
        if self.is_group_loca(room) {
            let principal = member.principal_id.clone();
            let candidates: Vec<&str> = principal.as_deref().into_iter().collect();
            if self.group_gate(room, &candidates) != Some(true) {
                return Err(ReissueError::Invite(InviteError::NotInGroup));
            }
        }
        // One member holds at most one invite per loca since the mint rule;
        // rows from before it may hold more, and all of them end here.
        let held: Vec<InviteRecord> = self
            .invites
            .lock_or_recover()
            .values()
            .filter(|i| i.room == room && i.member_id == member.id)
            .cloned()
            .collect();
        // The invite the new one stands in for: the highest issuer, so a
        // re-issue never makes an invite easier to end than the one it replaces.
        let Some(replaced) = held
            .iter()
            .max_by_key(|i| {
                (
                    invite_issuer_rank(&i.issued_by),
                    std::cmp::Reverse(i.issued_at),
                )
            })
            .cloned()
        else {
            return Err(ReissueError::NotHeld);
        };
        if held
            .iter()
            .any(|invite| !self.may_end_invite(actor, invite))
        {
            return Err(ReissueError::Forbidden);
        }
        let invite_key = self.new_invite_token();
        let fresh = InviteRecord::issued(
            &invite_key,
            room,
            &member.id,
            &replaced.name,
            &replaced.kind,
            self.now(),
            &replaced.issued_by,
        );
        let ended: Vec<String> = held.iter().map(|i| i.secret_hash.clone()).collect();
        // As `add_invite_locked` does for a mint: the store refuses a sealed
        // loca too, but a memory-only Building has only this check.
        if self.is_deleted(room) {
            return Err(ReissueError::Invite(InviteError::Sealed));
        }
        if let Err(error) = self
            .store
            .reissue_invite(&ended, &fresh, &invite_key, (self.now_ms)())
        {
            tracing::error!(%error, room, "could not re-issue an invite");
            return Err(ReissueError::Invite(InviteError::Storage));
        }
        // Committed: swap the mirror in one step under the invite lock.
        {
            let mut invites = self.invites.lock_or_recover();
            for hash in &ended {
                invites.remove(hash);
            }
            invites.insert(fresh.secret_hash.clone(), fresh.clone());
        }
        self.hold_delivery(&invite_key);
        #[cfg(test)]
        self.run_reissue_swapped_hook();
        // What the old invite opened ends now: its sessions, then its socket;
        // then the fresh invite goes out. All of it stays under the invite lock,
        // which every other way an invite of this seat begins or ends (a call,
        // another re-issue, release, kick, revoke, suspension, membership
        // revoke, group move) also takes. So no later invite of this seat
        // exists while this clean-up runs (it works by member and loca, not
        // by token, and would end a session taken with one), and the Lobby
        // bus receives this seat's invites in the order they became live: the
        // last `called` a client gets is the invite the server holds. Only
        // in-memory work remains here: two short leaf locks (`sessions`,
        // `rooms`) and two `broadcast::Sender::send`s, which never wait for a
        // receiver.
        for invite in &held {
            self.revoke_sessions_for_invite(&invite.room, &invite.member_id);
        }
        self.recheck_connections(room);
        let _ = self.lobby_tx.send(LobbyEvent::Called {
            member: member.id,
            room: fresh.room.clone(),
            token: invite_key.clone(),
        });
        drop(lifecycle);
        Ok((fresh, invite_key))
    }

    #[cfg(test)]
    fn run_reissue_swapped_hook(&self) {
        let hook = self.reissue_swapped_hook.lock_or_recover().clone();
        if let Some(hook) = hook {
            hook();
        }
    }

    /// Test-only: set (or clear) the hook `reissue_invite` runs after its swap.
    #[cfg(test)]
    pub fn set_reissue_swapped_hook(&self, hook: Option<TestHook>) {
        *self.reissue_swapped_hook.lock_or_recover() = hook;
    }

    /// Is `token` still the live invite for `room` of the membership
    /// `member_id`? The Lobby forwards a `called` only while it is, so an
    /// invite that ended between its delivery on the bus and the moment a
    /// Lobby socket forwards it (a call, then a revoke or re-issue of the
    /// same seat) never reaches the client after the invite that replaced it.
    pub fn call_is_current(&self, member_id: &str, room: &str, token: &str) -> bool {
        self.invites
            .lock_or_recover()
            .get(&secret_hash(token))
            .is_some_and(|invite| invite.member_id == member_id && invite.room == room)
    }

    /// Subscribe before taking the seat list and the pending deliveries, so a
    /// call racing the handshake is observed either there or on the bus
    /// (possibly both; clients de-duplicate by loca/token).
    pub fn subscribe_lobby(&self) -> broadcast::Receiver<LobbyEvent> {
        self.lobby_tx.subscribe()
    }

    pub fn home_room(&self) -> &str {
        &self.home_room
    }

    pub fn is_reserved_room(&self, room: &str) -> bool {
        !self.reserved_room.is_empty() && room == self.reserved_room.as_str()
    }

    /// The Lobby room: where everyone in the Building lands first and may
    /// talk. Every Building member reaches it by membership alone; it takes
    /// no invitations, has no seat limit, carries no extensions, and is never
    /// deleted, closed or moved into a group (PRINCIPLES "Lobby").
    pub fn is_lobby_room(&self, room: &str) -> bool {
        room == LOBBY_ROOM
    }

    /// Who may enter the Lobby room: the ranks, and any Building member
    /// signed in with a Building-level credential (their own key, profile
    /// credential or the building key). A session taken with a loca invite
    /// stays in its loca; a banned name stays out.
    fn lobby_door(
        &self,
        credential_authority: &RequestAuthority,
        admin_tok: Option<&str>,
        invite_tok: Option<&str>,
        session: Option<&SessionIdentity>,
        name_hint: Option<&str>,
    ) -> EnterDecision {
        if credential_authority.is_building_admin() && admin_tok.is_some() {
            return EnterDecision::Allowed;
        }
        if session.is_some_and(|s| s.admin) {
            return EnterDecision::Allowed;
        }
        if let Some(name) = self.door_name(invite_tok, session, credential_authority, name_hint) {
            if self.is_banned(LOBBY_ROOM, &name) {
                return EnterDecision::Banned;
            }
        }
        let building_session = session.is_some_and(|s| s.loca.is_none());
        if building_session {
            return EnterDecision::Allowed;
        }
        // A member's own key presented as the room credential (an agent's
        // listener and REST reads do this): a key, not an invite.
        if session.is_none()
            && invite_tok.is_some_and(|token| {
                !self.store.credential_is_revoked(token)
                    && self.member_for_credential(Some(token)).is_some()
            })
        {
            return EnterDecision::Allowed;
        }
        EnterDecision::Denied
    }

    /// Who calls everyone in the Lobby room. The Lobby holds the whole
    /// Building, so a member's `@all` there would wake every agent of the
    /// Building: only a rank (`rank`: the post carries Building-admin
    /// authority) calls everyone there. A rank's call is stored as `target:
    /// all`, so every client and a reconnecting agent's REST backfill see it;
    /// anyone else's `target: all` is dropped and their `@all` stays words.
    /// The mentions filter counts only `target: all` in the Lobby
    /// (`msg_addresses`). Every other room is left as it is.
    pub fn settle_lobby_call(&self, room: &str, body: &mut PostMessage, rank: bool) {
        if !self.is_lobby_room(room) {
            return;
        }
        let calls_all = body.target.as_deref() == Some("all") || text_calls_all(&body.text);
        if !calls_all {
            return;
        }
        if rank {
            if body.target.is_none() {
                body.target = Some("all".to_string());
            }
        } else if body.target.as_deref() == Some("all") {
            body.target = None;
        }
    }

    pub fn is_caretaker(&self, name: &str) -> bool {
        self.caretakers.contains(name)
    }

    pub fn caretaker_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.caretakers.iter().cloned().collect();
        names.sort();
        names
    }

    /// Count a Lobby socket of the membership `member_id` (its `mbid_`).
    pub fn lobby_join(&self, member_id: &str) {
        let mut online = self.lobby_online.lock_or_recover();
        *online.entry(member_id.to_string()).or_insert(0) += 1;
    }

    pub fn lobby_leave(&self, member_id: &str) {
        let mut online = self.lobby_online.lock_or_recover();
        if let Some(count) = online.get_mut(member_id) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                online.remove(member_id);
            }
        }
    }

    fn lobby_is_online(&self, member_id: &str) -> bool {
        self.lobby_online
            .lock_or_recover()
            .get(member_id)
            .copied()
            .unwrap_or(0)
            > 0
    }

    /// Record an invite. The master issues; we only persist and index it.
    #[cfg(test)]
    pub fn add_invite(&self, inv: InviteRecord, invite: &str) -> rusqlite::Result<()> {
        let _lifecycle = self.member_lifecycle.lock_or_recover();
        self.add_invite_locked(inv, invite)
    }

    /// `add_invite` for a caller that already holds the invite lock
    /// (`member_lifecycle`), which is not re-entrant. `invite` is the invite
    /// `inv` was derived from, in hand only at its mint.
    fn add_invite_locked(&self, inv: InviteRecord, invite: &str) -> rusqlite::Result<()> {
        if !inv.member_id.is_empty() && self.member_by_id(&inv.member_id).is_none() {
            return Err(rusqlite::Error::InvalidQuery);
        }
        if self.is_deleted(&inv.room) {
            return Err(rusqlite::Error::InvalidParameterName(
                "room is permanently sealed".into(),
            ));
        }
        // Every invite minted here is handed over on the member's Lobby, so
        // it keeps its delivery copy until its first use or deadline.
        self.store.issue_invite(&inv, invite)?;
        self.invites
            .lock_or_recover()
            .insert(inv.secret_hash.clone(), inv);
        self.hold_delivery(invite);
        Ok(())
    }

    /// An invite was minted for delivery: send it again on every Lobby connect
    /// of its member until it is first used or its window closes. This is the
    /// in-memory side of its bounded delivery copy.
    pub(crate) fn hold_delivery(&self, invite: &str) {
        self.deliveries
            .lock_or_recover()
            .insert(secret_hash(invite), invite.to_string());
    }

    /// Is an invite issued at `issued_at` still inside its delivery window at
    /// `now`? The counterpart of the store's `PAST_INVITE_DELIVERY_DEADLINE`.
    fn delivery_open(issued_at: u64, now: u64) -> bool {
        issued_at > 0 && now < issued_at.saturating_add(crate::store::INVITE_DELIVERY_WINDOW_MS)
    }

    /// The first session or loca socket taken with the invite whose hash is
    /// `invite_hash`: the holder has proved it has it, so its delivery ends
    /// (the copy is cleared and `first_used_at` stamped). Nothing is revoked
    /// or rotated, and an invite with no copy costs no write. A failed write is
    /// logged and the copy stays held, so the next use (or the deadline)
    /// clears it.
    pub fn note_invite_used_by_hash(&self, invite_hash: &str) {
        if !self.deliveries.lock_or_recover().contains_key(invite_hash) {
            return;
        }
        match self
            .store
            .end_invite_delivery_by_hash(invite_hash, (self.now_ms)())
        {
            Ok(_) => {
                self.deliveries.lock_or_recover().remove(invite_hash);
            }
            Err(error) => tracing::error!(
                %error,
                "could not end an invite's delivery copy at its first use; it stays until the next use or its deadline"
            ),
        }
    }

    /// The next invite this Lobby connection is owed, decided now: live, its
    /// copy still held, its window open, and not in `sent` (the hashes of
    /// what this pass already sent). A Lobby pass asks again before every
    /// frame instead of walking a list taken earlier, because sending a frame
    /// waits on the socket: an invite revoked, re-issued or used in the meantime
    /// is then no longer owed and is never sent, and an invite that replaced one
    /// is. The answer is the invite's record and the invite to deliver.
    pub fn next_owed_delivery(
        &self,
        member_id: &str,
        sent: &std::collections::HashSet<String>,
    ) -> Option<(InviteRecord, String)> {
        self.pending_deliveries(member_id)
            .into_iter()
            .find(|(invite, _)| !sent.contains(&invite.secret_hash))
    }

    /// The live invites of the membership `member_id` whose delivery is still
    /// owed: their copy is held and their window open at this moment, each
    /// with the invite. A Lobby connect sends each as an ordinary `called`,
    /// the same invite every time. A copy found past its deadline here is
    /// cleared now (a late delivery attempt), not sent.
    pub fn pending_deliveries(&self, member_id: &str) -> Vec<(InviteRecord, String)> {
        let now = (self.now_ms)();
        let held: Vec<(InviteRecord, String)> = {
            let candidates = self.invites_for_member(member_id);
            let deliveries = self.deliveries.lock_or_recover();
            candidates
                .into_iter()
                .filter_map(|invite| {
                    let invite_key = deliveries.get(&invite.secret_hash)?.clone();
                    Some((invite, invite_key))
                })
                .collect()
        };
        let (due, late): (Vec<_>, Vec<_>) = held
            .into_iter()
            .partition(|(invite, _)| Self::delivery_open(invite.issued_at, now));
        for (invite, _) in late {
            if let Err(error) = self
                .store
                .expire_invite_deliveries(now, Some(&invite.secret_hash))
            {
                tracing::error!(%error, "could not clear an invite delivery copy past its deadline");
            }
            self.deliveries
                .lock_or_recover()
                .remove(&invite.secret_hash);
        }
        due
    }

    /// Clear invite delivery copies whose window has closed or whose invite
    /// has ended, in the store and here. Called by the background sweep, so
    /// a copy nobody asks for again still ends on time. A storage error is
    /// logged as one; the copy is not sent past its deadline either way.
    pub fn expire_invite_deliveries(&self) {
        let now = (self.now_ms)();
        match self.store.expire_invite_deliveries(now, None) {
            Ok(0) => {}
            Ok(cleared) => tracing::info!(
                cleared,
                "invite delivery copies past their window or of ended invites were cleared"
            ),
            Err(error) => tracing::error!(
                %error,
                "could not clear invite delivery copies past their window; the sweep tries again"
            ),
        }
        let held: Vec<String> = self.deliveries.lock_or_recover().keys().cloned().collect();
        let ended: Vec<String> = {
            let invites = self.invites.lock_or_recover();
            held.into_iter()
                .filter(|hash| {
                    !invites
                        .get(hash)
                        .is_some_and(|invite| Self::delivery_open(invite.issued_at, now))
                })
                .collect()
        };
        let mut deliveries = self.deliveries.lock_or_recover();
        for hash in ended {
            deliveries.remove(&hash);
        }
    }

    /// The member's seats: one `(loca, ivid_)` per live invite. Ids only; no
    /// invite leaves here.
    pub fn seats_of(&self, member_id: &str) -> Vec<(String, String)> {
        let mut seats: Vec<(String, String)> = self
            .invites_for_member(member_id)
            .into_iter()
            .map(|invite| (invite.room, invite.id))
            .collect();
        seats.sort();
        seats
    }

    /// End an invite, by its hash. The row survives with `revoked_at` so "who
    /// was let in" stays answerable — revoking is not forgetting.
    ///
    /// Revoking cascades: an invite that dies takes with it any session minted
    /// from it and closes the connection it opened. Otherwise the door would be
    /// shut on paper while a live socket (and a still-valid session token) kept
    /// working — the audit's revoke-does-nothing hole.
    pub fn revoke_invite_by_hash(&self, invite_hash: &str) -> rusqlite::Result<bool> {
        // Under the invite lock, like every other way a seat's invite begins or
        // ends: a revoke that read this invite before a re-issue replaced it
        // would otherwise run its clean-up (by member and loca) after the
        // re-issue, ending the session and socket of the fresh invite.
        let _lifecycle = self.member_lifecycle.lock_or_recover();
        let Some(inv) = self.invites.lock_or_recover().get(invite_hash).cloned() else {
            return Ok(false);
        };
        self.store
            .revoke_invite_by_hash(invite_hash, (self.now_ms)())?;
        self.invites.lock_or_recover().remove(invite_hash);
        // Kill sessions bound to this invite's (loca, member) and close the
        // live connection in that loca for that name.
        self.revoke_sessions_for_invite(&inv.room, &inv.member_id);
        self.close_connections(&inv.room, &inv.name);
        Ok(true)
    }

    /// The management id (`ivid_`) of an invite in hand (a call's answer, a
    /// delivery), with v0.13.0's derivation. A stored invite's id is
    /// `InviteRecord::id`.
    pub fn invite_management_id(&self, token: &str) -> String {
        crate::store::hashed_id("ivid_", &format!("invite:{token}"))
    }

    /// The live invite whose management id (`ivid_`) is `reference`. A raw
    /// invite is not a reference and finds nothing.
    pub fn invite_by_ref(&self, reference: &str) -> Option<InviteRecord> {
        self.invites
            .lock_or_recover()
            .values()
            .find(|invite| invite.id == reference)
            .cloned()
    }

    pub fn revoke_invite_ref(&self, reference: &str) -> rusqlite::Result<bool> {
        match self.invite_by_ref(reference) {
            Some(invite) => self.revoke_invite_by_hash(&invite.secret_hash),
            None => Ok(false),
        }
    }

    /// Drop every session bound to a member (any loca). Used when a MEMBERSHIP
    /// is revoked — the identity itself is gone, so no session proving it may
    /// survive.
    fn revoke_sessions_for_member(&self, member_id: &str) {
        self.sessions
            .lock_or_recover()
            .retain(|_st, idy| idy.member.as_deref() != Some(member_id));
    }

    /// Drop every session bound to one invite's (loca, member). Used when a
    /// single INVITE is revoked — sessions for that member in OTHER locas (other
    /// invites) survive, only this seat's proof dies.
    fn revoke_sessions_for_invite(&self, room: &str, member_id: &str) {
        if member_id.is_empty() {
            return;
        }
        self.sessions.lock_or_recover().retain(|_st, idy| {
            !(idy.member.as_deref() == Some(member_id) && idy.loca.as_deref() == Some(room))
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

    /// Make every live socket in `room` check its credentials now, so the
    /// ones whose invite or session has just ended close. A re-issue is not a
    /// kick: the seat goes on under the fresh invite, and a `kicked` frame
    /// tells a listener to forget the loca's invite, which would race the
    /// Lobby `called` carrying the fresh one and could erase it. The roster
    /// frame is one every client already takes on each join and leave; each
    /// socket re-checks its credentials before it forwards any frame.
    fn recheck_connections(&self, room: &str) {
        let rooms = self.rooms.lock_or_recover();
        if let Some(r) = rooms.get(room) {
            let _ = r.tx.send(ServerFrame::Members {
                members: r.member_list(&self.profile_cards()),
            });
        }
    }

    /// End every live invite a name holds for one loca. This is how "kick"
    /// and "release" stop the invite — without it, the ejected
    /// name's token would open the door right back (PRINCIPLES: kick stops the
    /// invite; release takes back the seat). The membership is untouched.
    pub fn revoke_invites_for(&self, room: &str, name: &str) -> rusqlite::Result<()> {
        self.store.revoke_invites_for(room, name, (self.now_ms)())?;
        self.revoke_invites_cache_for(room, name);
        Ok(())
    }

    fn revoke_invites_cache_for(&self, room: &str, name: &str) {
        for inv in self.end_invites_cache_for(room, name) {
            self.close_connections(&inv.room, &inv.name);
        }
    }

    /// End the cached invites (and their sessions) a name holds for one loca,
    /// without closing the connection: for a caller that sends its own, more
    /// specific `Kicked` right after. Ban does — the socket closes on the
    /// first `Kicked` it reads, so a generic one sent here would tell a banned
    /// person they were not banned.
    fn end_invites_cache_for(&self, room: &str, name: &str) -> Vec<InviteRecord> {
        let matches: Vec<InviteRecord> = self
            .invites
            .lock_or_recover()
            .values()
            .filter(|i| i.room == room && i.name == name)
            .cloned()
            .collect();
        {
            let mut invites = self.invites.lock_or_recover();
            for inv in &matches {
                invites.remove(&inv.secret_hash);
            }
        }
        for inv in &matches {
            self.revoke_sessions_for_invite(&inv.room, &inv.member_id);
        }
        matches
    }

    /// Every live invite held by one member (its id, as a session names it
    /// too), across all locas — for "who am I" and member views.
    pub fn invites_for_member(&self, member_id: &str) -> Vec<InviteRecord> {
        if member_id.is_empty() {
            return Vec::new();
        }
        self.invites
            .lock_or_recover()
            .values()
            .filter(|i| i.member_id == member_id)
            .cloned()
            .collect()
    }

    /// Live invites for one loca (master view).
    pub fn invites_of(&self, room: &str) -> Vec<InviteRecord> {
        let mut v: Vec<InviteRecord> = self
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
        let now = (self.now_ms)();
        let expires_at_ms = now.saturating_add(Self::ADMIN_PAIRING_TTL_MS);
        let mut current = self.admin_pairing.lock_or_recover();
        // Only one Master code works at a time: an unused earlier one is
        // superseded, and the ledger says so.
        if let Some(previous) = current.log_id.take() {
            let _ = self.store.mark_pairing_superseded(&previous, now);
        }
        let log_id = Self::secure_token("pl_", 12);
        *current = AdminPairing {
            code: Self::secure_token("pair_", 16),
            session_ttl_ms: session_ttl_ms.clamp(
                Self::MIN_ADMIN_SESSION_TTL_MS,
                Self::MAX_ADMIN_SESSION_TTL_MS,
            ),
            expires_at_ms,
            log_id: Some(log_id.clone()),
        };
        if let Err(error) = self.store.log_pairing(
            &log_id,
            &current.code,
            now,
            expires_at_ms,
            current.session_ttl_ms,
        ) {
            tracing::error!(%error, "could not record the Master code in the key ledger");
        }
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
                log_id: None,
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
        if let Some(id) = current.log_id.as_deref() {
            let _ = self.store.mark_pairing_used(id, now);
        }
        *current = AdminPairing {
            code: Self::secure_token("pair_", 16),
            session_ttl_ms: Self::ADMIN_SESSION_TTL_MS,
            expires_at_ms: now.saturating_add(Self::ADMIN_PAIRING_TTL_MS),
            log_id: None,
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
        // Checked and filed under `name_claims`: a pending request holds its
        // name until it is decided. Released before the announcement.
        let claim = self.name_claims.lock_or_recover();
        if self.member_by_name(name).is_some() || self.store.has_pending_join_request_named(name) {
            return JoinRequestCreate::NameTaken;
        }
        if self.new_member_name_clashes(name) {
            return JoinRequestCreate::NameWorn;
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
        drop(claim);
        // Make the request VISIBLE where a human can act on it: a persisted
        // announcement in the home loca so the Master/Smaster/loca-care see
        // "<name> wants to join" in chat (addressed to the lead so a filtered
        // runtime also wakes). Without this the request only reached the hidden
        // Master Desk and never surfaced to the operator.
        self.announce_join_request(name, kind);
        JoinRequestCreate::Created {
            request_id,
            request_secret,
        }
    }

    /// Post a visible "<name> wants to join" announcement into the home loca so
    /// the request surfaces in the main app (not only the admin desk). Persisted
    /// and broadcast, addressed to the lead (falling back to `all`) so a filtered
    /// runtime wakes. Never carries any secret — just the requested name/kind.
    fn announce_join_request(&self, name: &str, kind: &str) {
        let room = self.home_room.as_ref().clone();
        let mut rooms = self.rooms.lock_or_recover();
        // Never silently drop the announce (review blocker): if the home loca is
        // not in memory yet, use defaults and let `room_mut` materialise it — the
        // same shape `set_lead` uses. And persist BEST-EFFORT: even if the DB
        // write fails, still push to history + broadcast so the request always
        // surfaces live. The request row already exists; the only job here is to
        // make it visible, and that must not be conditional.
        let (mode, settings) = rooms
            .get(&room)
            .map(|r| (r.mode.clone(), r.settings.clone()))
            .unwrap_or_else(|| (ChatMode::Free, self.default_settings.clone()));
        let target = settings.lead.clone().or_else(|| Some("all".into()));
        let msg = Message {
            attachments: Vec::new(),
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            room: room.clone(),
            sender: "loca".into(),
            sender_type: SenderType::User,
            target,
            text: format!("{name} ({kind}) wants to join — approve or deny it in Join requests."),
            reply_to: None,
            reply_to_sender: None,
            sender_principal: None,
            kind: protocol::MessageKind::Announce,
            ts: (self.now_ms)(),
        };
        let _ = self
            .store
            .insert_message_with_room(&msg, &mode, &settings, None);
        let r = Self::room_mut(&mut rooms, &room, &self.default_settings);
        r.history.push(msg.clone());
        let _ = r.tx.send(ServerFrame::Msg { message: msg });
    }

    /// Poll a join request with its secret. `Ok(None)`: unknown request or
    /// wrong secret; a storage error is an error.
    pub fn join_request_view(
        &self,
        id: &str,
        secret: &str,
    ) -> rusqlite::Result<Option<crate::store::JoinRequestView>> {
        self.store
            .join_request_view(id, secret, (self.now_ms)())
            .inspect_err(|error| tracing::error!(%error, "could not read a join request"))
    }

    /// The Master's pending-request review list.
    pub fn list_pending_join_requests(&self) -> Vec<(String, String, String, u64)> {
        self.store.list_pending_join_requests()
    }

    /// The name a still-pending join request asks for (it never changes).
    pub fn pending_join_request_name(&self, id: &str) -> Option<String> {
        self.store
            .list_pending_join_requests()
            .into_iter()
            .find(|(request, ..)| request == id)
            .map(|(_, name, ..)| name)
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
        // Approving admits a new name, so it meets the doors' rule again: a
        // rank or a caretaker may have come to wear it since the request was
        // filed, or an older server filed it unchecked. Refused here, before
        // the transaction, nothing is written: the request stays pending (the
        // Master can still deny it) and no admission right is consumed. Held
        // under `name_claims` until the new member is in the cache too, so a
        // member, a Smaster or a rename racing this approval sees it or is
        // seen by it.
        let _claim = self.name_claims.lock_or_recover();
        if let Some(name) = self.pending_join_request_name(id) {
            if let Some(rule) = admission_name_refusal(&name) {
                return Approve::NameRule(rule);
            }
            if self.member_by_name(&name).is_none() && self.new_member_name_clashes(&name) {
                return Approve::NameWorn;
            }
        }
        match self
            .store
            .approve_join_request_atomic(id, &mb_token, by, (self.now_ms)())
        {
            crate::store::ApproveTxn::Committed(member) => {
                // Mirror the committed DB row into the in-memory member cache.
                self.members
                    .lock_or_recover()
                    .insert(member.id.clone(), member);
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

    /// Deliver an approved request's `mb_`, re-fetchable until the client ACKs
    /// (crash-safe: the credential cannot be lost between receive and persist)
    /// or until `store::JOIN_BOOTSTRAP_WINDOW_MS` after the approval.
    pub fn claim_join_request_bootstrap(
        &self,
        id: &str,
        secret: &str,
    ) -> rusqlite::Result<crate::store::BootstrapClaim> {
        self.store
            .claim_join_request_bootstrap(id, secret, (self.now_ms)())
            .inspect_err(|error| tracing::error!(%error, "could not read a join request's key"))
    }

    /// Finalize an approved request's bootstrap after the client verified its
    /// `mb_` via `/whoami`. Closes the re-fetch window. `Some(true)` = just
    /// acked, `Some(false)` = already acked by this secret, `None` = unknown /
    /// unowned / not approved. Also answers after the delivery deadline: a
    /// requester whose ACK was lost has done nothing wrong.
    pub fn ack_join_request_bootstrap(
        &self,
        id: &str,
        secret: &str,
    ) -> rusqlite::Result<Option<bool>> {
        self.store
            .ack_join_request_bootstrap(id, secret, (self.now_ms)())
            .inspect_err(|error| tracing::error!(%error, "could not acknowledge a join request"))
    }

    /// Clear key delivery copies whose window has closed (join requests and
    /// membership claims), and retry an owed WAL clean-up. Called by the
    /// background sweep, so a copy nobody asks for again still ends on time.
    /// The membership and the claimed credentials are not touched. A storage
    /// error is logged as one, never as "nothing to clear".
    pub fn expire_join_request_bootstraps(&self) {
        let memory = self.expire_memory_claim_copies();
        match self.store.expire_join_request_bootstraps((self.now_ms)()) {
            Ok(0) if memory == 0 => {}
            Ok(cleared) => tracing::info!(
                cleared = cleared + memory,
                "key delivery copies (join requests, membership claims) past their window were cleared"
            ),
            Err(error) => tracing::error!(
                %error,
                "could not clear key delivery copies past their window; none cleared"
            ),
        }
    }

    /// Same, but taken with an invite: the session is confined to the invite's
    /// loca AND its identity comes from the invite's member — the session is
    /// proof of who the invite seats, not of whatever name the request body
    /// claims. (Before this, alice's invite could mint a "bob" session.)
    ///
    /// `admin_grant` is resolved server-side from a principal credential (or
    /// the one-use root pairing). It binds both role and credential ownership;
    /// request-body names never manufacture authority.
    ///
    /// `building_key` says the session was taken with a credential that
    /// reaches the whole Building through the legacy building key; only then
    /// does a loca-less session inherit that reach. `key_member` is the
    /// membership whose own key took the session when no principal grant can
    /// bind it (a memory-only store): the session speaks as that member and
    /// dies with the membership.
    ///
    /// `Ok(None)`: the invite or membership the caller resolved ended before
    /// the session could be recorded, so no session exists.
    pub fn create_session_scoped(
        &self,
        req: protocol::CreateSession,
        invite_record: Option<&InviteRecord>,
        admin_grant: Option<AdminSessionGrant>,
        building_key: bool,
        key_member: Option<MemberRecord>,
    ) -> rusqlite::Result<Option<protocol::SessionInfo>> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let now = (self.now_ms)();
        // A session is a bearer credential. Generate it directly from the
        // operating system CSPRNG; timestamps, counters and SipHash output are
        // not suitable secrets.
        let t = Self::secure_token("st_", 32);
        // The session names the membership by its id; whether it still
        // stands is checked below.
        let key_member_id = key_member.as_ref().map(|member| member.id.clone());
        let identity = match invite_record {
            Some(inv) => {
                // The membership is the identity; the invite's snapshot is the
                // fallback if the member record has since been revoked.
                let (name, kind) = match self.member_by_id(&inv.member_id) {
                    Some(m) => (m.name, m.kind),
                    None => (inv.name.clone(), inv.kind.clone()),
                };
                let kind = match kind.as_str() {
                    "user" => SenderType::User,
                    _ => SenderType::Agent,
                };
                // The principal comes from the membership record, not from
                // the member's original key: an invite session must not depend
                // on that one credential still being live (see `invite`).
                SessionIdentity {
                    principal_id: self.member_principal(&inv.member_id),
                    building_role: BuildingRole::Member,
                    name,
                    kind,
                    loca: Some(inv.room.clone()),
                    member: Some(inv.member_id.clone()),
                    invite: Some(inv.secret_hash.clone()),
                    building_key: false,
                    admin: false,
                    expires_at: now.saturating_add(Self::ADMIN_SESSION_TTL_MS),
                }
            }
            // No invite: a Building-level session. A verified credential names
            // it through `admin_grant` (its principal) or, in a memory-only
            // store, through `key_member` (its membership). Only a session with
            // neither — the shared building key, the sandbox — takes its name
            // from the body; that name is a label, and the key it was taken
            // with is the only authority it carries.
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
                    .or_else(|| key_member.as_ref().map(|m| m.name.clone()))
                    .unwrap_or_else(|| req.name.clone()),
                kind: admin_grant
                    .as_ref()
                    .and_then(|grant| grant.authority.kind)
                    .or_else(|| {
                        key_member.as_ref().map(|m| match m.kind.as_str() {
                            "user" => SenderType::User,
                            _ => SenderType::Agent,
                        })
                    })
                    .unwrap_or_else(|| req.kind.unwrap_or(SenderType::Agent)),
                loca: None,
                member: key_member.map(|m| m.id),
                invite: None,
                building_key,
                admin: admin_grant
                    .as_ref()
                    .map(|grant| grant.authority.is_building_admin())
                    .unwrap_or(false),
                expires_at: now.saturating_add(
                    admin_grant
                        .as_ref()
                        .map_or(Self::ADMIN_SESSION_TTL_MS, |grant| grant.ttl_ms),
                ),
            },
        };
        let name = identity.name.clone();
        let session_admin = identity.admin;
        let expires_at = Some(identity.expires_at);
        // Record the session under the session map's lock, and only while the
        // invite or membership it was taken with still stands. Every revocation
        // and suspension removes the invite from `invites` (and a revoked
        // membership from `members`) BEFORE it sweeps this map, so checking
        // under the same lock as the insert leaves no window: a revocation
        // either finished before this check (the session is refused) or
        // sweeps after the insert (the session dies with the rest). Lock
        // order: sessions -> invites, members, the store's connection, each
        // taken and released in turn under it (session_identity already
        // reaches the store under this lock). This is the record-time half of
        // the one rule for an invite session (`invite_session_live` is the
        // use-time half): it stands while its invite stands. The credential's
        // session cap is enforced inside this same critical section: the
        // store transaction below counts, ends the oldest and inserts, and
        // the ended sessions leave this map before the new one enters it, so
        // "source still stands, count, evict, insert" is one step.
        let mut sessions = self.sessions.lock_or_recover();
        let source_live = match invite_record {
            Some(inv) => self.invite_live_by_hash(&inv.secret_hash),
            None => key_member_id
                .as_deref()
                .is_none_or(|member| self.member_by_id(member).is_some()),
        };
        if !source_live {
            return Ok(None);
        }
        if identity.admin {
            self.store.save_admin_session(
                &t,
                &identity.name,
                identity.kind,
                identity.expires_at,
            )?;
        }
        // An invite session is taken with the invite, not with a credential, so
        // it gets no `principal_sessions` row: revoking a credential ends the
        // sessions taken with that credential and nothing else.
        let credential_id = match invite_record {
            Some(_) => None,
            None => admin_grant.and_then(|grant| grant.credential_id),
        };
        // A session filed under a credential counts against that credential's
        // cap (`store::SESSIONS_PER_CREDENTIAL`); the store ends the oldest in
        // the same transaction and names them so memory forgets them too.
        let mut ended = Vec::new();
        if let (Some(principal_id), Some(credential_id)) =
            (identity.principal_id.as_deref(), credential_id.as_deref())
        {
            ended = self.store.save_principal_session(
                &t,
                principal_id,
                credential_id,
                now,
                identity.expires_at,
            )?;
        }
        if !ended.is_empty() {
            tracing::info!(
                ended = ended.len(),
                "a credential took more than its live sessions allow; its oldest were ended"
            );
        }
        for session_hash in &ended {
            sessions.remove(session_hash);
        }
        // An invite session is filed under its invite, not under a credential
        // (above), so the credential's cap never counts it. Its own cap is
        // counted here, by the invite, in this same critical section: one
        // invite holds at most `SESSIONS_PER_CREDENTIAL` sessions, and taking
        // one more ends its oldest. These sessions live in memory only, so
        // there is no row to end.
        if let Some(invite_hash) = identity.invite.as_deref() {
            let mut held: Vec<(u64, String)> = sessions
                .iter()
                .filter(|(_, held)| held.invite.as_deref() == Some(invite_hash))
                .map(|(hash, held)| (held.expires_at, hash.clone()))
                .collect();
            if held.len() >= crate::store::SESSIONS_PER_CREDENTIAL {
                // Every session has the same lifetime, so the earliest to
                // expire is the oldest.
                held.sort();
                let over = held.len() + 1 - crate::store::SESSIONS_PER_CREDENTIAL;
                tracing::info!(
                    ended = over,
                    "an invite took more than its live sessions allow; its oldest were ended"
                );
                for (_, session_hash) in held.into_iter().take(over) {
                    sessions.remove(&session_hash);
                }
            }
        }
        sessions.insert(secret_hash(&t), identity);
        drop(sessions);
        if let Some(invite) = invite_record {
            self.note_invite_used_by_hash(&invite.secret_hash);
        }
        Ok(Some(protocol::SessionInfo {
            participant_id: format!("p_{id}"),
            session_token: t,
            name,
            admin: session_admin,
            expires_at,
        }))
    }

    /// Resolve a session token to its bound identity. An expired session (admin
    /// TTL passed) resolves to nothing — and is dropped — so a stale token is
    /// as good as no token.
    pub fn session_identity(&self, token: Option<&str>) -> Option<SessionIdentity> {
        let session_hash = secret_hash(token?);
        let idy = self.sessions.lock_or_recover().get(&session_hash)?.clone();
        let live = match idy.invite.as_deref() {
            Some(invite_hash) => self.invite_session_live(&idy, invite_hash),
            None => {
                idy.principal_id.is_none()
                    || !self.store.is_persistent()
                    || self
                        .store
                        .principal_session_active_by_hash(&session_hash, (self.now_ms)())
            }
        };
        let mut sessions = self.sessions.lock_or_recover();
        if !live {
            sessions.remove(&session_hash);
            return None;
        }
        if idy.expires_at <= (self.now_ms)() {
            sessions.remove(&session_hash);
            if idy.admin {
                let _ = self.store.delete_admin_session_by_hash(&session_hash);
            }
            return None;
        }
        Some(idy)
    }

    /// An invite session is live while the invite it was taken with is live and
    /// the member's principal is active, whatever happened to the member's
    /// credentials. Revoking, releasing or kicking the invite, suspending the
    /// member and revoking the membership all end it. The use-time half of
    /// the rule `create_session_scoped` checks when it records the session.
    /// Takes no lock while holding another: `invites`, then the store.
    fn invite_session_live(&self, idy: &SessionIdentity, invite_hash: &str) -> bool {
        let (Some(room), Some(member)) = (idy.loca.as_deref(), idy.member.as_deref()) else {
            return false;
        };
        let invite_live = self
            .invites
            .lock_or_recover()
            .get(invite_hash)
            .is_some_and(|i| i.room == room && i.member_id == member);
        invite_live
            && match idy.principal_id.as_deref() {
                Some(principal) if self.store.is_persistent() => {
                    self.store.active_principal(principal).is_some()
                }
                _ => true,
            }
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
        let session_hash = secret_hash(token);
        let admin = self
            .sessions
            .lock_or_recover()
            .get(&session_hash)
            .is_some_and(|identity| identity.admin);
        if admin {
            self.store.delete_admin_session_by_hash(&session_hash)?;
        }
        self.store
            .revoke_principal_session_by_hash(&session_hash, self.now())?;
        Ok(self
            .sessions
            .lock_or_recover()
            .remove(&session_hash)
            .is_some())
    }

    /// End what has expired without waiting for someone to present it: drop
    /// expired sessions from memory, then delete expired and ended sessions
    /// from storage. Called by the background sweep. Memory is swept even when
    /// storage fails; a storage error is logged as one and returned, never
    /// read as "nothing to sweep". Returns how many stored sessions went.
    pub fn sweep_sessions(&self) -> rusqlite::Result<usize> {
        let now = (self.now_ms)();
        let forgotten = {
            let mut sessions = self.sessions.lock_or_recover();
            let before = sessions.len();
            sessions.retain(|_, identity| identity.expires_at > now);
            before - sessions.len()
        };
        if forgotten > 0 {
            tracing::info!(forgotten, "expired sessions were dropped from memory");
        }
        match self.store.sweep_sessions(now) {
            Ok(0) => Ok(0),
            Ok(deleted) => {
                tracing::info!(
                    deleted,
                    "expired and ended sessions were deleted from storage"
                );
                Ok(deleted)
            }
            Err(error) => {
                tracing::error!(
                    %error,
                    "could not delete expired and ended sessions from storage; none deleted"
                );
                Err(error)
            }
        }
    }

    /// Check room existence without implicitly provisioning it.
    pub fn room_exists(&self, room: &str) -> bool {
        self.rooms.lock_or_recover().contains_key(room)
    }

    /// Internal sandbox/state helper; external closed-Building entry is gated.
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

    /// Seats per loca. Every person — human or agent — takes one; watchers
    /// take none.
    pub fn loca_capacity(&self) -> usize {
        self.loca_capacity
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Configure seats per loca, clamped to the bounds above. Seats already
    /// taken are never revoked by lowering it; only new seats are refused.
    pub fn set_loca_capacity(&self, seats: usize) -> usize {
        let seats = seats.clamp(Self::LOCA_CAPACITY_MIN, Self::LOCA_CAPACITY_MAX);
        self.loca_capacity
            .store(seats, std::sync::atomic::Ordering::Relaxed);
        seats
    }

    /// Notifications one sender may send in
    /// [`notifications::NOTIFICATION_SEND_WINDOW_MS`].
    pub fn notification_send_limit(&self) -> u32 {
        self.notification_send_limit
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Configure the sending limit's count (at least 1; the window is
    /// fixed). Returns the value in force.
    pub fn set_notification_send_limit(&self, max: u32) -> u32 {
        let max = max.max(1);
        self.notification_send_limit
            .store(max, std::sync::atomic::Ordering::Relaxed);
        max
    }

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
        // The Lobby room holds the whole Building: it has no seat limit.
        let seats = if room == LOBBY_ROOM {
            usize::MAX
        } else {
            identity::seats_of(&r.settings, self.seat_numbers())
        };
        if !r.members.contains_key(identity) && r.members.len() >= seats {
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
                entry.2.insert(session);
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
            // The seat's connections are now exactly this one: the evicted
            // session is gone whether or not its reader ever runs leave(). A
            // dead reader never reaches leave(), so keeping it here would
            // stick the seat in the roster forever (the ghost the operator
            // kept seeing); and an evicted reader that does run leave(), late,
            // is no longer listed, so it cannot free this connection's seat.
            if let Some(entry) = r.members.get_mut(identity) {
                entry.0 = name.to_string();
                entry.1 = kind;
                entry.2 = std::collections::HashSet::from([session]);
            }
        } else {
            r.members.insert(
                identity.to_string(),
                (
                    name.to_string(),
                    kind,
                    std::collections::HashSet::from([session]),
                ),
            );
        }
        let members = r.member_list(&self.profile_cards());
        let _ = r.tx.send(ServerFrame::Members { members });
        true
    }

    /// Deregister one connection (`session`, as given to `join`) on
    /// disconnect; frees the seat when none of its connections is left. A
    /// connection a newer one evicted is no longer listed, so its leave, in
    /// whatever order it comes, does not touch the seat.
    pub fn leave(&self, room: &str, identity: &str, session: u64) {
        let mut rooms = self.rooms.lock_or_recover();
        let Some(r) = rooms.get_mut(room) else { return };
        if let Some(entry) = r.members.get_mut(identity) {
            entry.2.remove(&session);
            if entry.2.is_empty() {
                r.members.remove(identity);
                r.seat_principals.remove(identity);
            }
        }
        let members = r.member_list(&self.profile_cards());
        let _ = r.tx.send(ServerFrame::Members { members });
    }

    /// Append a posted message, trim history, and broadcast it. The current
    /// chat mode is enforced here: `is_admin` bypasses gating (so the admin can
    /// always speak, e.g. while paused). Returns `Err(PostReject)` if the mode
    /// forbids this sender. `scope` scopes `op_id` replays
    /// (`Hub::operation_scope`); the separate `sender_principal` is the
    /// verified principal id that wrote the message (`None` when the door
    /// verified nobody), which a rename follows.
    pub fn post(
        &self,
        room: &str,
        body: PostMessage,
        is_admin: bool,
        scope: &OperationScope,
        sender_principal: Option<&str>,
    ) -> Result<Message, PostReject> {
        // A sealed (deleted) loca does not come back on a post. Without this,
        // `room_mut` (get-or-create) would resurrect a tombstoned room the
        // moment anyone with a stale invite posted to it — the room the master
        // shut would silently reopen. A permanent seal cannot be cleared by
        // inviting or posting; create a different loca for new work.
        if self.is_deleted(room) {
            return Err(PostReject::Deleted);
        }
        let mut body = body;
        if let Some(b) = self.dm(room) {
            // A side that left the building leaves the direct message read-only.
            if self.dm_read_only(&b) {
                return Err(PostReject::Archived);
            }
            // Every word in a direct message is said to the other person: it wakes
            // an agent listening for mentions without an `@`, and an `@all`
            // left over from another loca cannot widen it.
            if let Some(peer) = self.dm_peer_of(room, &body.sender) {
                body.target = Some(peer);
            }
        } else if body.sender_type == SenderType::Agent
            && body.target.as_deref() == Some("all")
            && names_people_but_not_all(&body.text)
        {
            // An agent that names people (`@ayse @mehmet ...`) but sent the
            // `all` target would wake every agent at the table. The named
            // people are reached by their `@name` already; `@all` in the text
            // is how an agent really calls everyone.
            body.target = None;
        }
        let op_id = body.op_id.filter(|id| !id.is_empty());
        if let Some(id) = op_id.as_deref() {
            match self.store.message_by_operation(room, &scope.key, id) {
                Ok(Some(message)) => return Ok(message),
                Ok(None) => {}
                Err(_) => return Err(PostReject::Storage),
            }
            // An operation the previous version stored under its REST key
            // answers the re-send only when it is provably this sender's.
            if let Some(legacy) = scope.legacy.as_deref() {
                match self.store.message_by_operation(room, legacy, id) {
                    Ok(Some(message)) => {
                        match Self::legacy_operation_owner(scope, legacy, &message) {
                            LegacyOwner::Sender => return Ok(message),
                            LegacyOwner::Other => {}
                            LegacyOwner::Unknown => return Err(PostReject::OperationUnclear),
                        }
                    }
                    Ok(None) => {}
                    Err(_) => return Err(PostReject::Storage),
                }
            }
        }
        // Resolve cited attachments BEFORE anything persists: each id must be an
        // available upload in THIS room. Rejecting up front means a durable
        // message can never cite a blob the server can't serve, and no
        // cross-room hash guess resolves. The resolved refs ride on the message
        // (durable JSON) and are flipped pending→referenced after the commit.
        let attachments: Vec<protocol::Attachment> = if body.attachments.is_empty() {
            Vec::new()
        } else {
            if body.attachments.len() > MAX_ATTACHMENTS_PER_MESSAGE {
                return Err(PostReject::TooManyAttachments {
                    max: MAX_ATTACHMENTS_PER_MESSAGE,
                });
            }
            let mut resolved: Vec<protocol::Attachment> =
                Vec::with_capacity(body.attachments.len());
            for id in &body.attachments {
                // Dedup: citing the same file twice is one reference, and keeps
                // each (message, sha) unique so the in-tx claim never sees a
                // spurious zero-row insert.
                if resolved.iter().any(|a| &a.id == id) {
                    continue;
                }
                match self.store.resolve_room_attachment(room, id) {
                    Some(a) => resolved.push(a),
                    None => return Err(PostReject::UnknownAttachment),
                }
            }
            resolved
        };
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
            let key = (scope.key.clone(), id.to_string());
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

        // A reply addresses the AUTHOR of the message it answers, as a separate
        // recipient alongside any explicit `target` — so a reply that also
        // targets/@mentions someone else wakes BOTH. `msg_addresses` keys wake
        // on `target`/`@name` and never looked at `reply_to`, so a bare reply
        // used to be a silent UI-only link. Resolve the author from the room's
        // in-memory tail first (you usually reply to what you can see), then the
        // durable store — the reply UI allows answering a message older than the
        // 200-message tail, and those must wake their author too; a cross-room
        // or unknown id resolves to nobody (message_owner is room-scoped), and a
        // self-reply never addresses the sender. `target` is left untouched.
        let reply_to_sender = match body.reply_to {
            None => None,
            Some(rid) => {
                let owner = match r.history.iter().rev().find(|m| m.id == rid) {
                    Some(m) => Some(m.sender.clone()),
                    // Only consult the store on a tail miss. A lookup ERROR must
                    // reject the post (503) — never silently drop the wake and
                    // report success. An unknown id is a real absence (Ok(None))
                    // and safely stays recipient-less.
                    None => self
                        .store
                        .message_owner(room, rid)
                        .map_err(|_| PostReject::Storage)?,
                };
                owner.filter(|owner| owner != &body.sender && !owner.is_empty())
            }
        };
        let msg = Message {
            attachments: attachments.clone(),
            kind: body.kind,
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            room: room.to_string(),
            sender: body.sender,
            sender_type: body.sender_type,
            target: body.target.filter(|t| !t.is_empty()),
            text: body.text,
            reply_to: body.reply_to,
            reply_to_sender,
            ts: self.next_condition_generation(now, r.last_msg_ms),
            sender_principal: sender_principal.map(str::to_string),
        };
        // Nothing leaves a direct message or a group loca: a caretaker summons would
        // copy its words into Care, where every rank reads (PRINCIPLES
        // "Birebir", "Groups").
        let caretaker_summons: Vec<CareSignal> =
            if room == self.home_room.as_str() || self.is_dm(room) || self.is_group_loca(room) {
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
                        owner_principal_id: None,
                        group: None,
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
                owner_principal_id: None,
                group: None,
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

        // (Lead is no longer set by parsing chat. "Speaking must never create a hidden side effect":
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
        // that a restart would forget (PRINCIPLES: "Messages are not lost" — a
        // successful reply means the word landed). Memory-only mode returns Ok.
        let operation = op_id.as_deref().map(|id| (scope.key.as_str(), id));
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
            // The in-tx authoritative claim aborts the message when a cited blob
            // was swept between the pre-check and the write. Distinguish that
            // (a caller-input problem → 400) from a genuine storage failure
            // (→ 503) by re-resolving: if any attachment no longer resolves, the
            // vanish race is why the write failed.
            if !attachments.is_empty()
                && attachments
                    .iter()
                    .any(|a| self.store.resolve_room_attachment(room, &a.id).is_none())
            {
                return Err(PostReject::UnknownAttachment);
            }
            return Err(PostReject::Storage);
        }
        // The cited blobs' pending→referenced flip already committed INSIDE the
        // message-insert transaction above (Store::write_attachment_refs), so
        // the message and its references are durable together — nothing to do
        // here. `attachments` was resolved up front purely to reject bad ids
        // and to set msg.attachments before persistence.
        if record_rate {
            r.post_times
                .entry(msg.sender.clone())
                .or_default()
                .push_back(now);
        }
        if let Some(id) = op_id.as_deref() {
            let key = (scope.key.clone(), id.to_string());
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
        // outbox. Copying the source message into Care chat history would leak
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
                        group: signal.group.clone(),
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
                    group: wake.group.clone(),
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
        // The Lead's task authority follows this principal, not the name.
        let lead_principal = lead.as_deref().and_then(|name| self.principal_named(name));
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
        if lead.is_none() && settings.extension_enabled("board") {
            return Err(LeadError::BoardOn);
        }
        settings.lead = lead.clone();
        settings.lead_principal = lead_principal;
        let text = match &lead {
            Some(name) => format!("{by} set @lead {name}."),
            None => format!("{by} set @lead none."),
        };
        let msg = Message {
            attachments: Vec::new(),
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
            reply_to_sender: None,
            sender_principal: None,
            kind: protocol::MessageKind::Announce,
            ts: (self.now_ms)(),
        };
        self.store
            .insert_message_with_room(&msg, &mode, &settings, None)
            .map_err(|_| LeadError::Storage)?;
        let r = Self::room_mut(&mut rooms, room, &self.default_settings);
        r.settings = settings.clone();
        r.history.push(msg.clone());
        let settings = self.shown_settings(settings);
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
    /// A loca's own number of seats reads as the number in force (see
    /// `identity::shown`).
    pub fn settings(&self, room: &str) -> RoomSettings {
        let settings = {
            let rooms = self.rooms.lock_or_recover();
            rooms
                .get(room)
                .map(|r| r.settings.clone())
                .unwrap_or_else(|| self.default_settings.clone())
        };
        self.shown_settings(settings)
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
            return Err(rusqlite::Error::InvalidParameterName(
                "room is permanently sealed".into(),
            ));
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
        let s = self.shown_settings(s);
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

    /// Every loca's ban list in one read, for the Building view. The caller
    /// filters by its own door: decide reach after this returns, never while
    /// the rooms lock is held (`enter_decision` takes it too).
    pub fn bans_by_room(&self) -> Vec<(String, Vec<String>)> {
        let deleted = self.deleted.lock_or_recover().clone();
        let rooms = self.rooms.lock_or_recover();
        let mut out: Vec<(String, Vec<String>)> = rooms
            .iter()
            .filter(|(room, r)| !r.banned.is_empty() && !deleted.contains(*room))
            .map(|(room, r)| {
                let mut banned: Vec<String> = r.banned.iter().cloned().collect();
                banned.sort();
                (room.clone(), banned)
            })
            .collect();
        out.sort();
        out
    }

    /// Apply a moderation action. Returns the updated mod state. For kick/ban a
    /// `Kicked` frame is broadcast so the targeted client closes its socket.
    pub fn moderate(
        &self,
        room: &str,
        action: protocol::ModAction,
        name: &str,
        actor: &RequestAuthority,
    ) -> Result<protocol::ModState, ModerationError> {
        use protocol::ModAction::*;
        // Admission and moderation cannot race a lower-rank removal of a
        // newly issued superior invite. Checks are repeated under both locks.
        let _lifecycle = self.member_lifecycle.lock_or_recover();
        let mut ranks = self.moderation_ranks.lock_or_recover();
        if !self.may_moderate_with_ranks(room, actor, action, name, &ranks) {
            return Err(ModerationError::Forbidden);
        }
        // A sealed loca is not reopened by a moderation action.
        if self.is_deleted(room) {
            return Ok(self.mod_state(room));
        }
        match action {
            Mute => self.store.set_ban(
                room,
                name,
                "mute",
                (self.now_ms)(),
                actor.moderation_rank(),
                actor.principal_id.as_deref(),
            )?,
            Unmute => self.store.clear_ban(room, name, "mute")?,
            Ban => {
                self.store.set_ban(
                    room,
                    name,
                    "ban",
                    (self.now_ms)(),
                    actor.moderation_rank(),
                    actor.principal_id.as_deref(),
                )?;
                // set_ban persisted the restriction and invite cut-off in one
                // transaction; nothing fallible remains before the mirrors.
                // The connection is closed below by the ban's own `Kicked`.
                self.end_invites_cache_for(room, name);
            }
            Unban => self.store.clear_ban(room, name, "ban")?,
            Kick | Release => self.revoke_invites_for(room, name)?,
        }
        let restriction = match action {
            Mute | Unmute => Some("mute"),
            Ban | Unban => Some("ban"),
            _ => None,
        };
        if let Some(kind) = restriction {
            let key = (room.to_string(), name.to_string(), kind.to_string());
            if matches!(action, Mute | Ban) {
                ranks.insert(key, actor.moderation_rank());
            } else {
                ranks.remove(&key);
            }
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
                // PRINCIPLES: "kick — connection closes and invitation stops". Kick is
                // not just a disconnect: it ends the invite, otherwise the same
                // token walks right back in. Take the seat AND the invite.
                let _ = r.tx.send(ServerFrame::Kicked {
                    name: name.to_string(),
                    banned: false,
                });
                // Moderation targets a NAME (that is what the operator sees);
                // the seat map is identity-keyed, so drop whichever seat wears
                // that label.
                r.members.retain(|_id, (n, _, _)| n != name);
                let members = r.member_list(&self.profile_cards());
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
                let members = r.member_list(&self.profile_cards());
                let _ = r.tx.send(ServerFrame::Members { members });
                // The door shuts, reading included — persist so a restart does
                // not quietly reopen it (PRINCIPLES: "a restart does not kill the room").
                drop(rooms);
                return Ok(self.mod_state(room));
            }
            Unban => {
                r.banned.remove(name);
            }
            Release => {
                // Take back the seat, not the belonging. Their invites for THIS
                // loca end and the connection closes; membership of the
                // building is untouched, so the next call-in is one click.
                // Drop the roster entry directly (Kick/Ban do) so a dead socket
                // leaves no ghost — the whole point of "the work is done."
                let _ = r.tx.send(ServerFrame::Kicked {
                    name: name.to_string(),
                    banned: false,
                });
                r.members.retain(|_id, (n, _, _)| n != name);
                let members = r.member_list(&self.profile_cards());
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
    /// PRINCIPLES: "archived = read-only, no domain mutation." One predicate
    /// so every write path asks the same question instead of each guessing.
    pub fn is_writable(&self, room: &str) -> bool {
        !self.is_deleted(room) && !self.is_archived(room)
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
                    settings: self.shown_settings(s.clone()),
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

    /// Is `principal_id` this loca's appointed operator? Answered from the
    /// in-memory mirror, never from SQLite.
    pub fn is_operator_principal(&self, room: &str, principal_id: &str) -> bool {
        self.operators
            .lock_or_recover()
            .get(room)
            .is_some_and(|current| current == principal_id)
    }

    /// Open a loca. `extensions` is the chosen set (already normalized by the
    /// caller); `None` keeps the legacy set, so an older client that sends
    /// only a name gets the loca it always got. A chosen set is written to the
    /// new loca's Journal.
    #[cfg(test)]
    pub fn create_loca(
        &self,
        room: &str,
        actor: &RequestAuthority,
        extensions: Option<Vec<String>>,
    ) -> Result<Option<LocaOperatorAssignment>, OperatorAssignmentError> {
        self.create_loca_in(room, actor, extensions, None)
            .map_err(|error| match error {
                CreateLocaError::Operator(error) => error,
                CreateLocaError::Group(_)
                | CreateLocaError::Identity(_)
                | CreateLocaError::Name(_) => OperatorAssignmentError::AuthorityRequired,
            })
    }

    /// Open a loca, in a group when `group` is given. Only the Master or a
    /// Smaster who is a member of that group opens a group loca, and the
    /// loca is tied to its group in the same commit that creates it.
    #[cfg(test)]
    pub fn create_loca_in(
        &self,
        room: &str,
        actor: &RequestAuthority,
        extensions: Option<Vec<String>>,
        group: Option<&str>,
    ) -> Result<Option<LocaOperatorAssignment>, CreateLocaError> {
        self.create_loca_with(room, actor, extensions, group, &IdentityEdit::default())
    }

    /// Open a loca with its identity (description, seats, icon, colour,
    /// labels) set in the same write. An empty `identity` opens exactly the
    /// loca `create_loca_in` opens; a non-empty one is written to the new
    /// loca's Journal.
    pub fn create_loca_with(
        &self,
        room: &str,
        actor: &RequestAuthority,
        extensions: Option<Vec<String>>,
        group: Option<&str>,
        identity: &IdentityEdit,
    ) -> Result<Option<LocaOperatorAssignment>, CreateLocaError> {
        if !actor.is_building_admin() {
            return Err(OperatorAssignmentError::AuthorityRequired.into());
        }
        let numbers = self.seat_numbers();
        let identity =
            identity::validated(identity, numbers.ceiling).map_err(CreateLocaError::Identity)?;
        if self.is_deleted(room) || room.starts_with(Self::DM_PREFIX) {
            return Err(OperatorAssignmentError::Conflict.into());
        }
        // Founding a loca in a group takes turns with archiving or deleting
        // that group: the check below must still hold when the loca is bound.
        let group_turn = group.map(|_| self.member_lifecycle.lock_or_recover());
        if let Some(group) = group {
            if self.is_reserved_room(room) || room == self.home_room() {
                return Err(CreateLocaError::Group(GroupError::Excluded));
            }
            self.may_create_in_group(group, actor)
                .map_err(CreateLocaError::Group)?;
        }
        let mut settings = self.default_settings.clone();
        settings.extensions = extensions.clone();
        identity::apply(&mut settings, &identity);
        let opened_identity = identity::opened_line(&settings, numbers);
        let mut rooms = self.rooms.lock_or_recover();
        if rooms.contains_key(room) {
            return Err(OperatorAssignmentError::Conflict.into());
        }
        loca_name::check_new(room, rooms.keys().map(String::as_str))
            .map_err(CreateLocaError::Name)?;
        let assignment = if self.store.is_persistent() && !self.admin_open() {
            let founder = actor
                .principal_id
                .as_deref()
                .ok_or(OperatorAssignmentError::AuthorityRequired)?;
            Some(
                self.store
                    .create_room_with_operator(room, &settings, founder, group, self.now())
                    .map_err(map_loca_operator_error)?,
            )
        } else {
            // The deliberately open sandbox has no authenticated founder. It
            // may still use SQLite to preserve its room state across restarts.
            self.store
                .save_room(room, &ChatMode::Free, &settings)
                .map_err(|_| OperatorAssignmentError::Storage)?;
            if group.is_some() {
                self.store
                    .set_room_group(room, group, actor.principal_id.as_deref(), self.now())
                    .map_err(|_| OperatorAssignmentError::Storage)?;
            }
            None
        };
        // Tied to its group before anyone can see the room exists.
        if let Some(group) = group {
            self.bind_new_group_loca(room, group);
        }
        if let Some(assignment) = assignment.as_ref() {
            self.operators
                .lock_or_recover()
                .insert(room.to_string(), assignment.principal_id.clone());
        }
        rooms.insert(room.to_string(), Room::with(ChatMode::Free, settings, 1));
        drop(rooms);
        drop(group_turn);
        if let Some(chosen) = extensions {
            let by = actor
                .display_name
                .clone()
                .unwrap_or_else(|| "master".to_string());
            let kind = actor.kind.unwrap_or(SenderType::User);
            // The loca exists either way; a lost Journal line is logged, not
            // turned into a failed create the caller would retry into 409.
            if let Err(error) =
                self.append_journal(room, by, kind, extensions::opened_line(&chosen))
            {
                tracing::error!(room, %error, "could not journal the opening extensions");
            }
        }
        if let Some(line) = opened_identity {
            let by = actor
                .display_name
                .clone()
                .unwrap_or_else(|| "master".to_string());
            let kind = actor.kind.unwrap_or(SenderType::User);
            if let Err(error) = self.append_journal(room, by, kind, line) {
                tracing::error!(room, %error, "could not journal the opening identity");
            }
        }
        Ok(assignment)
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

    /// The Building's ranks — the Master and every Smaster — with the locas
    /// each one is seated in right now. Ranks hold no Lobby membership, so the
    /// resident directory never lists them; this is how the Building view
    /// shows everyone who is part of the Building.
    pub fn building_ranks(&self) -> Vec<RankPresence> {
        let mut ranks: Vec<RankPresence> = Vec::new();
        let mut add = |name: String, role: &'static str, unused: bool| {
            if !ranks.iter().any(|r| r.name == name && r.role == role) {
                ranks.push(RankPresence {
                    name,
                    role,
                    locas: Vec::new(),
                    unused,
                });
            }
        };
        for profile in self.profiles() {
            match profile.role {
                BuildingRole::Master => add(profile.display_name, "master", false),
                BuildingRole::Smaster => {
                    let unused = !self.principal_ever_used(&profile.id);
                    add(profile.display_name, "smaster", unused)
                }
                BuildingRole::Member => {}
            }
        }
        // Memory-only mode persists no profiles; the live registry still knows
        // every Smaster.
        for record in self.smasters.lock_or_recover().values() {
            add(record.name.clone(), "smaster", false);
        }
        let deleted = self.deleted.lock_or_recover().clone();
        let rooms = self.rooms.lock_or_recover();
        let dms = self.dms.lock_or_recover().clone();
        let mut names: Vec<&String> = rooms
            .keys()
            .filter(|r| !deleted.contains(*r) && !dms.contains_key(*r))
            .collect();
        names.sort();
        for room in names {
            for (identity, (label, _, _)) in &rooms[room].members {
                let Some(role) = seat_rank(identity) else {
                    continue;
                };
                // There is one Master, so its seat is its whatever the label;
                // a Smaster seat is matched by the label its session wears.
                let found = ranks
                    .iter_mut()
                    .find(|r| r.role == role && (role == "master" || r.name == *label));
                match found {
                    Some(rank) => rank.locas.push(room.clone()),
                    None => ranks.push(RankPresence {
                        name: label.clone(),
                        role,
                        locas: vec![room.clone()],
                        unused: false,
                    }),
                }
            }
        }
        ranks.sort_by(|a, b| (a.role != "master", &a.name).cmp(&(b.role != "master", &b.name)));
        ranks
    }

    /// `building_ranks` as a viewer may read it: the Master sees every seat;
    /// anyone else never sees a group loca it cannot enter named as "where".
    pub fn building_ranks_for(&self, viewer: &RequestAuthority) -> Vec<RankPresence> {
        let hidden = self.group_locas_hidden_from(viewer);
        let mut ranks = self.building_ranks();
        if !hidden.is_empty() {
            for rank in &mut ranks {
                rank.locas.retain(|room| !hidden.contains(room));
            }
        }
        ranks
    }

    /// A Master or Smaster renames its own profile. Identity, credentials,
    /// sessions and rank stay; only the label changes, everywhere it is worn
    /// live: the Smaster registry, open sessions and every loca seat. Past
    /// messages follow the person: what it said under the old name now reads
    /// the new one, in every loca, and open feeds are told to relabel.
    pub fn rename_profile(
        &self,
        actor: &RequestAuthority,
        requested: &str,
    ) -> Result<PrincipalIdentity, ProfileRenameError> {
        if !actor.is_building_admin() {
            return Err(ProfileRenameError::AuthorityRequired);
        }
        let Some(principal_id) = actor.principal_id.clone() else {
            return Err(ProfileRenameError::AuthorityRequired);
        };
        let name = validate_display_name(requested).map_err(ProfileRenameError::Name)?;
        let current = actor.display_name.clone().unwrap_or_default();
        let _claim = self.name_claims.lock_or_recover();
        let _lifecycle = self.member_lifecycle.lock_or_recover();
        let ranks = self.moderation_ranks.lock_or_recover();
        if ranks
            .iter()
            .any(|((_, restricted_name, _), _)| restricted_name == &current)
        {
            return Err(ProfileRenameError::RestrictionActive);
        }
        let folded = crate::store::search_fold(name);
        let worn_by_other =
            |other: &str| other != current && crate::store::search_fold(other) == folded;
        // A caretaker's name is taken even before that caretaker is admitted:
        // a call to it is relayed to Care (`addressed_caretakers`).
        if self.caretaker_name_folds_to(&folded)
            || self
                .members
                .lock_or_recover()
                .values()
                .any(|m| worn_by_other(&m.name))
            || self
                .smasters
                .lock_or_recover()
                .values()
                .any(|record| record.principal_id != principal_id && worn_by_other(&record.name))
        {
            return Err(ProfileRenameError::NameTaken);
        }
        let relabel = current != name;
        match self.store.rename_principal(&principal_id, name, relabel) {
            Ok(true) => {}
            Ok(false) => return Err(ProfileRenameError::NameTaken),
            Err(_) => return Err(ProfileRenameError::Failed),
        }
        for record in self.smasters.lock_or_recover().values_mut() {
            if record.principal_id == principal_id {
                record.name = name.to_string();
            }
        }
        for session in self.sessions.lock_or_recover().values_mut() {
            if session.principal_id.as_deref() == Some(principal_id.as_str()) {
                session.name = name.to_string();
            }
        }
        let mut seat_keys = vec![format!("sm:pr:{principal_id}")];
        if actor.is_master() {
            seat_keys.push("@master".into());
        }
        // Lock order: name_claims -> member_lifecycle -> moderation_ranks
        // (held since the top) -> rooms, and under rooms only the store's
        // connection, the same order `post` uses when it resolves a reply's
        // author.
        let mut rooms = self.rooms.lock_or_recover();
        for (room_id, room) in rooms.iter_mut() {
            // What older clients are told is exactly what they were told
            // before: `Renamed` when some person's message under the old
            // name is in this tail, judged before anything is relabelled.
            // They relabel by name; the id frame below is what is true.
            let tell_by_name = !current.is_empty()
                && relabel
                && room.history.iter().any(|message| {
                    message.sender_type == SenderType::User && message.sender == current
                });
            if relabel {
                // Exactly this principal's messages, and exactly the replies
                // that answer one of them: ownership is the recorded writer,
                // never the label. A replied-to message that has left the
                // in-memory tail is looked up in storage by id.
                let owns = |message: &Message| {
                    message.sender_principal.as_deref() == Some(principal_id.as_str())
                };
                let owned_here: std::collections::HashSet<u64> = room
                    .history
                    .iter()
                    .filter(|message| owns(message))
                    .map(|message| message.id)
                    .collect();
                let mut ids = Vec::new();
                let mut reply_ids = Vec::new();
                let mut stored_owner: HashMap<u64, bool> = HashMap::new();
                for message in room.history.iter_mut() {
                    if owns(message) && message.sender != name {
                        message.sender = name.to_string();
                        ids.push(message.id);
                    }
                    let (Some(label), Some(target)) =
                        (message.reply_to_sender.as_deref(), message.reply_to)
                    else {
                        continue;
                    };
                    if label == name {
                        continue;
                    }
                    let answers_renamer = if owned_here.contains(&target) {
                        true
                    } else if let Some(&known) = stored_owner.get(&target) {
                        known
                    } else {
                        match self.store.message_sender_principal(room_id, target) {
                            Ok(writer) => {
                                let owned = writer.as_deref() == Some(principal_id.as_str());
                                stored_owner.insert(target, owned);
                                owned
                            }
                            Err(error) => {
                                tracing::warn!(%error, room = %room_id, message = message.id,
                                    "reply label left unchanged: replied-to writer unreadable");
                                false
                            }
                        }
                    };
                    if answers_renamer {
                        message.reply_to_sender = Some(name.to_string());
                        reply_ids.push(message.id);
                    }
                }
                if !ids.is_empty() || !reply_ids.is_empty() {
                    let _ = room.tx.send(ServerFrame::Relabelled {
                        to: name.to_string(),
                        ids,
                        reply_ids,
                    });
                }
            }
            if tell_by_name {
                let _ = room.tx.send(ServerFrame::Renamed {
                    from: current.clone(),
                    to: name.to_string(),
                });
            }
            let mut touched = false;
            for key in &seat_keys {
                if let Some(seat) = room.members.get_mut(key) {
                    seat.0 = name.to_string();
                    touched = true;
                }
            }
            if touched {
                let members = room.member_list(&self.profile_cards());
                let _ = room.tx.send(ServerFrame::Members { members });
            }
        }
        drop(rooms);
        Ok(self
            .store
            .active_principal(&principal_id)
            .unwrap_or(PrincipalIdentity {
                id: principal_id,
                display_name: name.to_string(),
                kind: SenderType::User,
                role: actor.building_role.unwrap_or(BuildingRole::Member),
            }))
    }

    /// May `actor` apply `action` to the participant labelled `target`?
    ///
    /// PRINCIPLES: the upper layer covers the lower one and the lower one
    /// cannot undo the upper one's decision. Loca moderation is for the
    /// table's participants, not for Building ranks: nobody mutes, kicks or
    /// bans the Master, and only the Master does so to a Smaster. Restriction
    /// provenance and invite issuer rank also constrain the action.
    pub fn may_moderate(
        &self,
        room: &str,
        actor: &RequestAuthority,
        action: protocol::ModAction,
        target: &str,
    ) -> bool {
        self.may_moderate_with_ranks(
            room,
            actor,
            action,
            target,
            &self.moderation_ranks.lock_or_recover(),
        )
    }

    pub fn may_end_invite(&self, actor: &RequestAuthority, invite: &InviteRecord) -> bool {
        actor.moderation_rank() >= invite_issuer_rank(&invite.issued_by)
    }

    fn may_moderate_with_ranks(
        &self,
        room: &str,
        actor: &RequestAuthority,
        action: protocol::ModAction,
        target: &str,
        ranks: &ModerationRanks,
    ) -> bool {
        use protocol::ModAction::*;
        let kind = match action {
            Mute | Unmute => Some("mute"),
            Ban | Unban => Some("ban"),
            _ => None,
        };
        if kind
            .and_then(|kind| ranks.get(&(room.to_string(), target.to_string(), kind.into())))
            .is_some_and(|rank| *rank > actor.moderation_rank())
        {
            return false;
        }
        if matches!(action, Kick | Ban | Release)
            && self.invites.lock_or_recover().values().any(|invite| {
                invite.room == room && invite.name == target && !self.may_end_invite(actor, invite)
            })
        {
            return false;
        }
        if matches!(action, Unmute | Unban) {
            return true;
        }
        match self.building_rank_of(room, target) {
            Some(BuildingRole::Master) => false,
            Some(BuildingRole::Smaster) => actor.is_master(),
            _ => true,
        }
    }

    /// The Building rank wearing the label `name`, as far as the server can
    /// prove it: a persisted profile first, then the live Smaster registry,
    /// then the seat key of whoever sits in `room` under that label (which
    /// also covers memory-only mode, where no profiles are persisted).
    fn building_rank_of(&self, room: &str, name: &str) -> Option<BuildingRole> {
        if let Some(profile) = self
            .profiles()
            .into_iter()
            .find(|profile| profile.display_name == name)
        {
            return Some(profile.role);
        }
        if self
            .smasters
            .lock_or_recover()
            .values()
            .any(|record| record.name == name)
        {
            return Some(BuildingRole::Smaster);
        }
        let rooms = self.rooms.lock_or_recover();
        let seat = rooms.get(room).and_then(|r| {
            r.members
                .iter()
                .find(|(_, (label, _, _))| label == name)
                .map(|(identity, _)| identity.clone())
        })?;
        if seat == "@master" {
            Some(BuildingRole::Master)
        } else if seat.starts_with("sm:") {
            Some(BuildingRole::Smaster)
        } else {
            None
        }
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
        // Held across the write so two appointments cannot leave the mirror
        // naming the one the database replaced.
        let mut operators = self.operators.lock_or_recover();
        let assignment = self
            .store
            .appoint_loca_operator(room, target_principal_id, actor_id, self.now())
            .map_err(map_loca_operator_error)?;
        operators.insert(room.to_string(), assignment.principal_id.clone());
        Ok(assignment)
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
        let mut operators = self.operators.lock_or_recover();
        let revoked = self
            .store
            .revoke_loca_operator(room, &current.principal_id, self.now())
            .map_err(map_loca_operator_error)?;
        operators.remove(room);
        Ok(revoked)
    }

    /// End an appointment because a rule requires it (the operator left the
    /// loca's group), not by anyone's choice: no actor's appointment rights
    /// are involved. The record keeps the revocation.
    pub(crate) fn end_loca_operator_by_rule(
        &self,
        room: &str,
    ) -> Result<(), OperatorAssignmentError> {
        let Some(current) = self.store.loca_operator(room) else {
            return Ok(());
        };
        let mut operators = self.operators.lock_or_recover();
        self.store
            .revoke_loca_operator(room, &current.principal_id, self.now())
            .map_err(map_loca_operator_error)?;
        operators.remove(room);
        Ok(())
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
        let scoped = self
            .session_identity(session_token)
            .filter(|s| s.loca.as_deref() == Some(room));
        let principal = scoped
            .as_ref()
            .and_then(|s| s.principal_id.as_deref())
            .or(authority.principal_id.as_deref());
        if let Some(principal_id) = principal {
            let current = self.operators.lock_or_recover().get(room).cloned();
            if let Some(current) = current {
                return current == principal_id;
            }
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
        // Written always; pushed to open sockets only while the Journal is on
        // in this loca (a line the server writes itself, such as a group
        // move, can land while it is off).
        if r.settings.extension_enabled("journal") {
            let _ = r.tx.send(ServerFrame::Journal {
                entry: entry.clone(),
            });
        }
        Ok(entry)
    }

    // ---- notes (living, keyed project state) ----

    pub fn loca_memory(&self, room: &str) -> rusqlite::Result<Option<protocol::LocaMemory>> {
        self.store.loca_memory(room)
    }

    pub fn loca_memory_version(&self, room: &str) -> rusqlite::Result<u64> {
        self.store.loca_memory_version(room)
    }

    pub fn loca_memory_snapshot(
        &self,
        room: &str,
    ) -> rusqlite::Result<Option<protocol::LocaMemorySnapshot>> {
        self.store.loca_memory_snapshot(room)
    }

    pub fn memory_persistence_available(&self) -> bool {
        self.store.memory_persistence_available()
    }

    pub fn wiki_snapshot(
        &self,
        room: &str,
    ) -> Result<crate::store::WikiSnapshot, crate::store::WikiError> {
        self.store.wiki_snapshot(room)
    }

    pub fn tick_wiki(&self) {
        if !self.store.is_persistent() {
            return;
        }
        let result = self.store.wiki_maintenance_rooms();
        match result {
            Ok(rooms) => {
                for room in rooms {
                    let result = if self.is_writable(&room) {
                        self.store
                            .schedule_wiki_review(&room, self.now())
                            .map(|_| ())
                    } else {
                        self.store.cancel_wiki_review(&room)
                    };
                    if let Err(error) = result {
                        tracing::warn!(?error, "Wiki maintenance scheduling failed");
                    }
                }
            }
            Err(error) => tracing::warn!(?error, "Wiki maintenance scan failed"),
        }
    }

    pub fn wiki_page(
        &self,
        room: &str,
        slug: &str,
    ) -> Result<crate::store::WikiPageSnapshot, crate::store::WikiError> {
        self.store.wiki_page(room, slug)
    }

    pub fn claim_wiki_review(
        &self,
        room: &str,
        principal: &str,
    ) -> Result<Option<crate::store::WikiBatch>, crate::store::WikiError> {
        self.store.claim_wiki_review(room, principal, self.now())
    }

    pub fn finish_wiki_review(
        &self,
        room: &str,
        principal: &str,
        update: &crate::store::WikiCommit,
        lease: &crate::store::WikiLease,
    ) -> Result<u64, crate::store::WikiError> {
        self.store
            .finish_wiki_review(room, principal, update, lease, self.now())
    }

    pub fn wiki_batch_status(&self, room: &str) -> Result<Option<String>, crate::store::WikiError> {
        self.store.wiki_batch_status(room)
    }

    pub fn renew_wiki_review(
        &self,
        room: &str,
        principal: &str,
        lease: &crate::store::WikiLease,
    ) -> Result<u64, crate::store::WikiError> {
        self.store
            .renew_wiki_review(room, principal, lease, self.now())
    }

    pub fn configure_wiki(
        &self,
        room: &str,
        editor: &str,
        enabled: bool,
        interval: u64,
    ) -> Result<(), crate::store::WikiError> {
        let identities = self
            .store
            .active_principals_named(editor)
            .map_err(|_| crate::store::WikiError::Storage)?;
        let [identity] = identities.as_slice() else {
            return Err(crate::store::WikiError::InvalidInput);
        };
        if identity.kind != SenderType::Agent {
            return Err(crate::store::WikiError::InvalidInput);
        }
        if enabled
            && !self
                .store
                .wiki_editor_runtime_ready(room, &identity.id, self.now())?
        {
            return Err(crate::store::WikiError::Conflict);
        }
        self.store
            .configure_wiki(room, &identity.id, enabled, interval)
    }

    pub fn wiki_native_ready(
        &self,
        room: &str,
        principal: &str,
        runtime: &str,
    ) -> Result<(), crate::store::WikiError> {
        self.store
            .wiki_native_ready(room, principal, runtime, self.now())
    }

    pub fn wiki_context_for(
        &self,
        room: &str,
        principal: &str,
    ) -> Result<serde_json::Value, crate::store::WikiError> {
        self.store.wiki_context_for(room, principal)
    }

    pub fn wiki_context_requests(
        &self,
        room: &str,
        principal: Option<&str>,
        operator: bool,
    ) -> Result<serde_json::Value, crate::store::WikiError> {
        if !operator && principal != Some(self.wiki_snapshot(room)?.editor_principal.as_str()) {
            return Err(crate::store::WikiError::NotEditor);
        }
        let mut value = self.store.wiki_context_requests(room)?;
        let invited = self.invites_of(room);
        if let Some(requests) = value["requests"].as_array_mut() {
            requests.retain(|r| {
                invited
                    .iter()
                    .any(|i| Some(i.name.as_str()) == r["target"].as_str())
            });
        }
        Ok(value)
    }

    pub fn select_wiki_context(
        &self,
        room: &str,
        principal: &str,
        selection: &crate::store::WikiContextSelection,
    ) -> Result<(), crate::store::WikiError> {
        self.store
            .select_wiki_context(room, principal, selection, self.now())
    }

    pub fn accept_wiki_context(
        &self,
        room: &str,
        principal: &str,
        ack: &crate::store::WikiContextAck,
    ) -> Result<(), crate::store::WikiError> {
        self.store
            .accept_wiki_context(room, principal, ack, self.now())
    }

    pub fn offer_wiki_context(
        &self,
        room: &str,
        actor: Option<&str>,
        operator: bool,
        target: &str,
        selection: &crate::store::WikiContextSelection,
    ) -> Result<(), crate::store::WikiError> {
        let snapshot = self.wiki_snapshot(room)?;
        if !operator && actor != Some(snapshot.editor_principal.as_str()) {
            return Err(crate::store::WikiError::NotEditor);
        }
        let principals = self
            .store
            .active_principals_named(target)
            .map_err(|_| crate::store::WikiError::Storage)?;
        let [identity] = principals.as_slice() else {
            return Err(crate::store::WikiError::InvalidInput);
        };
        if identity.kind != SenderType::Agent
            || !self.invites_of(room).iter().any(|i| i.name == target)
        {
            return Err(crate::store::WikiError::InvalidInput);
        }
        self.select_wiki_context(room, &identity.id, selection)
    }

    pub fn commit_wiki(
        &self,
        room: &str,
        principal: &str,
        update: &crate::store::WikiCommit,
    ) -> Result<u64, crate::store::WikiError> {
        self.store.commit_wiki(room, principal, update, self.now())
    }

    pub fn remove_wiki_page(
        &self,
        room: &str,
        principal: &str,
        slug: &str,
        removal: &crate::store::WikiPageRemoval,
    ) -> Result<u64, crate::store::WikiError> {
        self.store
            .remove_wiki_page(room, principal, slug, removal, self.now())
    }

    pub fn edit_wiki_page(
        &self,
        room: &str,
        actor: &str,
        slug: &str,
        edit: &crate::store::WikiPageEdit,
    ) -> Result<u64, crate::store::WikiError> {
        self.store
            .edit_wiki_page(room, actor, slug, edit, self.now())
    }

    pub fn loca_memory_metadata(&self) -> rusqlite::Result<Vec<protocol::LocaMemoryMetadata>> {
        self.store.loca_memory_metadata()
    }

    pub fn loca_memory_entries(
        &self,
        room: &str,
        after_id: u64,
        limit: usize,
    ) -> Result<protocol::LocaMemoryEntryPage, crate::store::MemoryReadError> {
        self.store.loca_memory_entries(room, after_id, limit)
    }

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
            .replace_note(room, &previous, &updated)
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
    pub fn note_history(&self, room: &str, key: &str) -> rusqlite::Result<Vec<Note>> {
        self.store.note_history(room, key)
    }

    /// Room memory search: full message archive (DB) + current notes.
    pub fn search(
        &self,
        room: &str,
        q: &str,
        limit: usize,
    ) -> rusqlite::Result<(Vec<Message>, Vec<Note>)> {
        let msgs = self.store.search_messages(room, q, limit)?;
        let ql = crate::store::search_fold(q);
        let notes: Vec<Note> = {
            let rooms = self.rooms.lock_or_recover();
            rooms
                .get(room)
                .map(|r| {
                    r.notes
                        .values()
                        .filter(|n| {
                            crate::store::search_fold(&n.title).contains(&ql)
                                || crate::store::search_fold(&n.body).contains(&ql)
                                || crate::store::search_fold(&n.key).contains(&ql)
                        })
                        .cloned()
                        .collect()
                })
                .unwrap_or_default()
        };
        Ok((msgs, notes))
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
        self.messages_after_within(room, since, 0, u64::MAX, limit)
    }

    /// [`Hub::messages_after`], leaving out everything said before `from_ts`
    /// and everything with an id above `through_id`.
    fn messages_after_within(
        &self,
        room: &str,
        since: u64,
        from_ts: u64,
        through_id: u64,
        limit: usize,
    ) -> rusqlite::Result<Vec<Message>> {
        if let Some(messages) = self
            .store
            .messages_after(room, since, from_ts, through_id, limit)?
        {
            return Ok(messages);
        }
        let rooms = self.rooms.lock_or_recover();
        Ok(rooms
            .get(room)
            .map(|r| {
                r.history
                    .iter()
                    .filter(|m| m.id > since && m.ts >= from_ts && m.id <= through_id)
                    .take(limit)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default())
    }

    pub fn members(&self, room: &str) -> Vec<Member> {
        let cards = self.profile_cards();
        let rooms = self.rooms.lock_or_recover();
        rooms
            .get(room)
            .map(|r| r.member_list(&cards))
            .unwrap_or_default()
    }

    /// Loca list for a caller. An invite opens ONE loca — so the list an invite
    /// holder sees is that one loca, not the whole building's floor plan (which
    /// leaked every loca's name, member count, mode and last line to anyone
    /// holding any invite). `reach` returns true for a loca the caller may
    /// enter; the master/building-key reaches all, an invite reaches its own.
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
        let groups = self.group_names_by_room();

        let rooms = self.rooms.lock_or_recover();
        let mut list: Vec<RoomSummary> = rooms
            .iter()
            .filter(|(name, _)| visible.contains(name.as_str()))
            .map(|(name, r)| {
                let humans = r
                    .members
                    .values()
                    .filter(|(_, k, n)| !n.is_empty() && *k == SenderType::User)
                    .count();
                let agents = r
                    .members
                    .values()
                    .filter(|(_, k, n)| !n.is_empty() && *k == SenderType::Agent)
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
                    group: groups.get(name).map(|(_, group)| group.clone()),
                    group_id: groups.get(name).map(|(id, _)| id.clone()),
                    // Only callers `reach` lets in see these, like the rest.
                    title: r.settings.title.clone(),
                    description: r.settings.description.clone(),
                    icon: r.settings.icon.clone(),
                    color: r.settings.color.clone(),
                    labels: r.settings.labels.clone(),
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

/// Does this text call everyone with `@all`, as a whole word in any letter
/// case? The same words the mentions filter reads (`msg_addresses`).
pub(crate) fn text_calls_all(text: &str) -> bool {
    text.to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '@' || c == '-' || c == '_'))
        .any(|token| token == "@all")
}

/// Does this text address specific people with `@name`, and not everyone with
/// `@all`? Names are the identity-name characters (letters, digits, `-`, `_`,
/// `.`); an `@` inside a word (an e-mail address) is not a mention.
pub(crate) fn names_people_but_not_all(text: &str) -> bool {
    let mut named = false;
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
            if name.eq_ignore_ascii_case("all") {
                return false;
            }
            if !name.is_empty() {
                named = true;
            }
            i = end;
            prev = chars.get(end.saturating_sub(1)).copied();
            continue;
        }
        prev = Some(c);
        i += 1;
    }
    named
}

#[cfg(test)]
mod tests;
