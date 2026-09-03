//! Shared wire types for agent-room: messages, participants, and the WS/REST
//! frames that the server, the web client, and the skill helper all speak.

use serde::{Deserialize, Serialize};

/// Who sent a message. `agent` = a Claude Code instance (via the skill),
/// `user` = a human (the operator or any other person in the room).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SenderType {
    Agent,
    User,
}

/// A file shared in a room. The bytes live in the content-addressed blob store
/// (`id` == `sha256`); a message carries only this reference, never the binary,
/// so the WS/JSON frame stays small.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Attachment {
    /// Content id (equals `sha256`); used to fetch the bytes.
    pub id: String,
    pub sha256: String,
    /// Display name, sanitized on ingest; never used as a filesystem path.
    pub name: String,
    /// MIME type the server sniffed from the bytes (not the client's claim).
    pub mime: String,
    pub size: u64,
}

/// A single message on a room's wall. Everyone connected to the room sees
/// every message; `target` only signals *who is invited to reply*.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Monotonic, sortable id (assigned by the server).
    pub id: u64,
    pub room: String,
    /// Display name of the sender (its username in the room).
    pub sender: String,
    pub sender_type: SenderType,
    /// Routing hint: `"all"` invites every agent to reply, `"<username>"` is a
    /// direct address, `None` is a plain wall post (no reply expected).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub text: String,
    /// What kind of utterance this is. Chat by default; an announcement is
    /// something the loca needs to know rather than a turn in the
    /// conversation, and the UI gives it its own shape so it is not scrolled
    /// past like small talk.
    #[serde(default, skip_serializing_if = "MessageKind::is_default")]
    pub kind: MessageKind,
    /// Id of the message this one replies to (thread context), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<u64>,
    /// Author of the message this one replies to, resolved by the server at post
    /// time. A reply addresses that author as a SEPARATE recipient (in addition
    /// to any explicit `target`), so replying wakes them exactly like an
    /// `@mention`. Server-derived and not persisted; `None` when this is not a
    /// reply, the replied-to message is unknown, or a self-reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to_sender: Option<String>,
    /// Unix milliseconds.
    pub ts: u64,
    /// Files shared with this message (image / PDF / text refs into the blob
    /// store). Empty for an ordinary text message; omitted from the wire when so.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
}

/// A participant currently connected to a room.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Member {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: SenderType,
}

/// Body of `POST /rooms/{id}/messages`. The server fills in `id`/`ts`/`room`.
#[derive(Debug, Clone, Deserialize)]
pub struct PostMessage {
    /// `say` (default) or `announce`.
    #[serde(default)]
    pub kind: MessageKind,
    pub sender: String,
    pub sender_type: SenderType,
    #[serde(default)]
    pub target: Option<String>,
    pub text: String,
    #[serde(default)]
    pub reply_to: Option<u64>,
    /// Caller-generated operation id. Replaying the same id as the same
    /// effective identity returns the first message without a second effect.
    #[serde(default)]
    pub op_id: Option<String>,
    /// Ids (== sha256) of already-uploaded attachments to cite on this message.
    /// Each must have been uploaded to THIS room (POST .../attachments) and not
    /// yet swept; the server resolves each to its stored ref and flips it
    /// `pending → referenced`. An unknown/foreign id rejects the whole post.
    #[serde(default)]
    pub attachments: Vec<String>,
}

/// One allowed social mark on a message. Reactions are visible to the loca,
/// while the live event is directed only to the message owner.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MessageReaction {
    pub message_id: u64,
    pub emoji: String,
    pub actors: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SetMessageReaction {
    pub emoji: String,
    pub active: bool,
    #[serde(default)]
    pub reactor: String,
    #[serde(default)]
    pub reactor_type: Option<SenderType>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MessageReactionEvent {
    pub message_id: u64,
    pub emoji: String,
    pub actors: Vec<String>,
    pub owner: String,
    pub reactor: String,
    pub active: bool,
    pub ts: u64,
}

/// How a room's chat is gated right now. The admin sets this; the server
/// enforces it hard (unauthorized posts are rejected).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum ChatMode {
    /// Anyone may post anytime (default).
    #[default]
    Free,
    /// Only names on the allowlist may post.
    Restricted { allow: Vec<String> },
    /// Turn-based: only `order[turn]` may post; the server advances `turn`
    /// after each accepted message.
    RoundRobin { order: Vec<String>, turn: usize },
    /// Nobody but the admin may post.
    Paused,
}

/// A room as listed by `GET /rooms`.
#[derive(Debug, Clone, Serialize)]
pub struct RoomSummary {
    pub room: String,
    pub members: usize,
    /// Presence split for the sidebar: `.N` humans, `*M` agents at the table.
    #[serde(default)]
    pub humans: usize,
    #[serde(default)]
    pub agents: usize,
    pub mode: ChatMode,
    /// Closed (read-only) rooms are shown differently and are the only ones
    /// that may be deleted.
    #[serde(default)]
    pub archived: bool,
    /// Last message preview for the sidebar ("sender: text…").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<String>,
    /// Last message cursor. The web client compares this with its per-loca
    /// read cursor before fetching the small unread tail for sidebar badges.
    #[serde(default)]
    pub last_id: u64,
    /// The building's private governance/caretaker loca.
    #[serde(default)]
    pub special: bool,
}

/// Body of `PUT /rooms/{id}/mode` (admin-only; requires the admin token).
#[derive(Debug, Clone, Deserialize)]
pub struct SetMode {
    pub mode: ChatMode,
}

/// Body of `POST /rooms/{id}/lead` — name a lead, or `null` to end it. An
/// explicit operator action, not a parsed chat message.
#[derive(Debug, Clone, Deserialize)]
pub struct SetLead {
    #[serde(default)]
    pub lead: Option<String>,
}

/// Who receives automatic Reminder messages for this loca. Delivery still
/// has exactly one owner; `loca-care` is the availability fallback when the
/// selected coordinator has no healthy runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReminderRecipient {
    #[default]
    Lead,
    All,
    Person {
        name: String,
    },
}

/// Admin-tunable per-room settings (currently the rate limit). Managed live by
/// the admin; the server seeds defaults from env at boot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomSettings {
    /// This loca's lead, if one has been named.
    ///
    /// A lead advises; it does not command. Work still comes from an operator
    /// — the lead cannot hand out görevler, override a mode, or moderate. What
    /// it does is see the whole room: it notices two agents on the same file,
    /// says what should probably go first, and reports back what happened here.
    /// Agents weigh its word; they do not obey it. When the lead and the
    /// operator disagree, the operator wins.
    ///
    /// Named through an explicit operator action, announced in the open so
    /// everybody present learns of it at the same moment, and delivered as a
    /// direct wake to the new lead. Lasts until another is named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead: Option<String>,
    /// Max messages a single sender may post within `rate_window_secs`.
    /// `0` disables rate limiting for the room.
    pub rate_limit: u32,
    /// Sliding-window length in seconds.
    pub rate_window_secs: u32,
    /// "Live" (WhatsApp) mode: when true, the room is in active discussion —
    /// every connected client receives every message even if it connected with
    /// `filter=mentions`. The operator toggles this; off = async, token-frugal
    /// (each client honours its own filter). Default off.
    #[serde(default)]
    pub live: bool,
    /// Archived ("closed"): the room becomes read-only — nobody may post, but
    /// members stay and the history/notes remain readable. Reversible; only an
    /// archived room may be deleted. Distinct from `ChatMode::Paused`, which is
    /// a temporary "everyone hush" during an active session.
    #[serde(default)]
    pub archived: bool,
    /// Seconds of silence after which live mode auto-disables (0 = never).
    /// Keeps a forgotten live room from waking every agent forever.
    #[serde(default = "default_live_timeout")]
    pub live_timeout_secs: u32,
    /// This loca's operators: humans responsible for THIS loca — they may
    /// create/manage tasks and moderate here (and only here). The grand
    /// operator (admin token) manages this list and outranks it everywhere.
    #[serde(default)]
    pub operators: Vec<String>,
    /// Maximum number of chat fragments coalesced into one runtime turn.
    /// Original messages remain individually persisted and visible.
    #[serde(default = "default_turn_max_messages")]
    pub turn_max_messages: u32,
    /// Quiet time after the latest fragment before a runtime turn is flushed.
    #[serde(default = "default_turn_idle_ms")]
    pub turn_idle_ms: u32,
    /// Hard age from the first fragment. Unlike the idle window this deadline
    /// never slides, so continuous typing cannot starve delivery.
    #[serde(default = "default_turn_max_wait_ms")]
    pub turn_max_wait_ms: u32,
    /// Seconds before an explicitly declared wait may produce a care signal.
    #[serde(default = "default_care_wait_secs")]
    pub care_wait_secs: u32,
    /// Minimum seconds before the same unresolved signal can repeat.
    #[serde(default = "default_care_cooldown_secs")]
    pub care_cooldown_secs: u32,
    /// Bounded attempts before the signal is escalated instead of repeated.
    #[serde(default = "default_care_max_attempts")]
    pub care_max_attempts: u32,
    /// Recent source-room messages copied into a privacy-bounded care context.
    #[serde(default = "default_care_context_messages")]
    pub care_context_messages: u32,
    /// Operator-selected Reminder recipient: the current room lead or one
    /// named person. This does not change task ownership or room authority.
    #[serde(default)]
    pub care_recipient: ReminderRecipient,
    /// Optional reminders after room inactivity; `0` disables each class.
    #[serde(default)]
    pub care_goal_secs: u32,
    #[serde(default)]
    pub care_task_secs: u32,
    #[serde(default)]
    pub care_silence_secs: u32,
}

fn default_live_timeout() -> u32 {
    120
}

fn default_turn_max_messages() -> u32 {
    4
}

fn default_turn_idle_ms() -> u32 {
    5_000
}

fn default_turn_max_wait_ms() -> u32 {
    15_000
}

fn default_care_wait_secs() -> u32 {
    120
}

fn default_care_cooldown_secs() -> u32 {
    300
}

fn default_care_max_attempts() -> u32 {
    2
}

fn default_care_context_messages() -> u32 {
    8
}

impl Default for RoomSettings {
    fn default() -> Self {
        RoomSettings {
            lead: None,
            rate_limit: 10,
            rate_window_secs: 30,
            live: false,
            archived: false,
            live_timeout_secs: 120,
            operators: Vec::new(),
            turn_max_messages: default_turn_max_messages(),
            turn_idle_ms: default_turn_idle_ms(),
            turn_max_wait_ms: default_turn_max_wait_ms(),
            care_wait_secs: default_care_wait_secs(),
            care_cooldown_secs: default_care_cooldown_secs(),
            care_max_attempts: default_care_max_attempts(),
            care_context_messages: default_care_context_messages(),
            care_recipient: ReminderRecipient::Lead,
            care_goal_secs: 0,
            care_task_secs: 0,
            care_silence_secs: 0,
        }
    }
}

/// Body of `PUT /rooms/{id}/settings` (admin-only). Fields left `None` keep
/// their current value.
#[derive(Debug, Clone, Deserialize)]
pub struct SetSettings {
    #[serde(default)]
    pub rate_limit: Option<u32>,
    #[serde(default)]
    pub rate_window_secs: Option<u32>,
    #[serde(default)]
    pub live: Option<bool>,
    #[serde(default)]
    pub archived: Option<bool>,
    /// Auto-turn-off for live mode after this many seconds of silence.
    /// 0 disables the timer. Default 120s.
    #[serde(default)]
    pub live_timeout_secs: Option<u32>,
    /// Replace this loca's operator list (grand operator only).
    #[serde(default)]
    pub operators: Option<Vec<String>>,
    /// Runtime turn packet size. `1` disables coalescing.
    #[serde(default)]
    pub turn_max_messages: Option<u32>,
    /// Quiet window after the latest fragment.
    #[serde(default)]
    pub turn_idle_ms: Option<u32>,
    /// Hard deadline from the first fragment.
    #[serde(default)]
    pub turn_max_wait_ms: Option<u32>,
    #[serde(default)]
    pub care_wait_secs: Option<u32>,
    #[serde(default)]
    pub care_cooldown_secs: Option<u32>,
    #[serde(default)]
    pub care_max_attempts: Option<u32>,
    #[serde(default)]
    pub care_context_messages: Option<u32>,
    #[serde(default)]
    pub care_recipient: Option<ReminderRecipient>,
    #[serde(default)]
    pub care_goal_secs: Option<u32>,
    #[serde(default)]
    pub care_task_secs: Option<u32>,
    #[serde(default)]
    pub care_silence_secs: Option<u32>,
}

/// A living note: a keyed, editable piece of project state. Unlike a chat
/// message (append-only), a note is *updated in place* — "what changed about
/// the project" lives here. Keyed by `key`, unique within a room.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    /// Stable identity within the room, e.g. "api-schema", "deploy-status".
    pub key: String,
    pub title: String,
    /// Markdown/plain body — the current state of this note.
    pub body: String,
    /// Names allowed to write this note. Empty = anyone may write.
    /// Only the operator manages this list; enforcement is *soft* (see server).
    #[serde(default)]
    pub can_write: Vec<String>,
    /// Who last wrote it, and when (unix ms).
    pub updated_by: String,
    pub updated_at: u64,
    /// Monotonic bump on every write; lets clients detect changes / order.
    pub rev: u64,
}

/// Body of `POST /rooms/{id}/notes` (create) — the note must not exist yet.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateNote {
    pub key: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    /// Compatibility claim from older clients. Authenticated HTTP routes
    /// replace it with the actor derived from the session/davet credential.
    pub by: String,
    #[serde(default)]
    pub by_type: Option<SenderType>,
    #[serde(default)]
    pub can_write: Vec<String>,
}

/// Body of `PUT /rooms/{id}/notes/{key}` (update) — the note must exist.
/// Fields left `None` are unchanged. `can_write` is only honored when the
/// request carries operator authority — which the server derives from the
/// admin token, never from the request body.
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateNote {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    /// Compatibility claim; the route overwrites it with canonical identity.
    pub by: String,
    #[serde(default)]
    pub by_type: Option<SenderType>,
    /// Operator-only reassignment of who may write. `None` = leave as is.
    #[serde(default)]
    pub can_write: Option<Vec<String>>,
}

/// Body of `POST /sessions` — binds an identity to a server-issued session
/// token so `sender` can be derived server-side instead of trusted from the
/// request body (see PRODUCTION.md, "session-bound identity").
/// `runtime`/`capabilities` are registry metadata for future routing.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateSession {
    pub name: String,
    #[serde(default)]
    pub kind: Option<SenderType>,
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// Response of `POST /sessions`.
#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    pub participant_id: String,
    pub session_token: String,
    pub name: String,
    /// Whether this session carries the master's administrative authority.
    pub admin: bool,
    /// Unix-ms expiry for short-lived sessions; `None` means no expiry.
    pub expires_at: Option<u64>,
}

/// One-use code that pairs a browser with the master's seat without putting
/// the building's root key in the browser.
#[derive(Debug, Clone, Serialize)]
pub struct PairingInfo {
    pub pairing_code: String,
    /// Lifetime of the admin session this one-use code will mint.
    pub session_ttl_hours: u64,
    /// Absolute Unix-ms deadline for using the one-use pairing code itself.
    pub pairing_expires_at: u64,
}

/// A task — conversation's GUEST, never its centre (PRINCIPLES #3/#5).
///
/// A task is a DECLARATION: an operator making a piece of work official —
/// "this is real, it has an owner". The agent then TAKES it (üzerine alır)
/// and finishes it; the operator may contest/cancel/reassign. Most work
/// still flows through plain conversation — a task is for work worth
/// declaring, never the required path for doing things. No queue, no lease.
/// A line in the loca's journal: something that was already done.
///
/// The counterpart to [`Task`], and deliberately its opposite. A task is
/// declared by an operator and points forward — it is work that has not
/// happened yet. A journal entry is written by whoever did the work and points
/// backward. Nobody assigns it, nobody closes it, and nothing edits it: the
/// journal is append-only so that "what actually happened" cannot be quietly
/// rewritten later.
/// How a message is meant to be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageKind {
    /// An ordinary turn in the conversation.
    #[default]
    Say,
    /// Something the loca must know: a release, a rotation, a breaking change.
    /// Rare by design — an announcement that arrives every few minutes stops
    /// being one.
    Announce,
}

impl MessageKind {
    pub fn is_default(&self) -> bool {
        matches!(self, MessageKind::Say)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalEntry {
    pub id: u64,
    pub room: String,
    /// Who did it — the server derives this, so it cannot be claimed for
    /// someone else.
    pub by: String,
    pub by_type: SenderType,
    /// One line, in the doer's own words.
    pub text: String,
    pub at: u64,
}

/// What an agent sends to record a piece of finished work.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateJournalEntry {
    pub text: String,
    /// Ignored when a session is bound; the server always prefers the identity
    /// it issued over anything the body claims.
    #[serde(default)]
    pub by: Option<String>,
    #[serde(default)]
    pub by_type: Option<SenderType>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: u64,
    pub room: String,
    pub title: String,
    /// The operator whose signature created it.
    pub created_by: String,
    /// The message this grew out of, if any (conversation stays the source).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_message: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned_to: Option<String>,
    pub status: TaskStatus,
    pub created_at: u64,
    /// Last explicit task-state change. Ordinary room chat is not progress.
    #[serde(default)]
    pub progress_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    /// Declared by an operator; waiting for its agent to take it.
    Open,
    /// The agent has taken it upon themselves (üzerine aldı).
    Taken,
    Done,
    Cancelled,
}

/// Body of `POST /rooms/{id}/tasks` (operator authority required).
#[derive(Debug, Clone, Deserialize)]
pub struct CreateTask {
    pub title: String,
    #[serde(default)]
    pub from_message: Option<u64>,
    #[serde(default)]
    pub assigned_to: Option<String>,
    /// Who is creating (soft in dev; session/admin overrides apply).
    pub by: String,
}

/// Body of `PATCH /rooms/{id}/tasks/{tid}`.
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateTask {
    #[serde(default)]
    pub status: Option<TaskStatus>,
    #[serde(default)]
    pub assigned_to: Option<String>,
    pub by: String,
}

/// The loca's one active outcome. A goal says why the room is working; tasks
/// remain optional, explicitly linked paths rather than an automatic queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    pub id: u64,
    pub room: String,
    pub outcome: String,
    /// The next observable proof of movement toward the outcome. Optional: a
    /// goal remains an outcome, not a hidden task list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<String>,
    /// Per-goal care threshold. `None` inherits the room setting; `0` disables
    /// goal-staleness attention for this goal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_after_secs: Option<u32>,
    /// The operator whose explicit action created it.
    pub created_by: String,
    pub completion: GoalCompletion,
    /// Relevant only for `all_tasks`; every id must resolve to a task in this
    /// loca and reach `done` before deterministic completion.
    #[serde(default)]
    pub task_ids: Vec<u64>,
    pub status: GoalStatus,
    pub created_at: u64,
    /// Last explicit goal change or progress on one of its linked tasks.
    /// It deliberately does not follow room chat activity.
    #[serde(default)]
    pub progress_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalCompletion {
    /// The operator judges that the stated outcome has been reached.
    #[default]
    Manual,
    /// All explicitly linked tasks being `done` closes the goal.
    AllTasks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalStatus {
    Active,
    Achieved,
    Cancelled,
}

/// Body of `POST /rooms/{id}/goals` (operator authority required).
#[derive(Debug, Clone, Deserialize)]
pub struct CreateGoal {
    pub outcome: String,
    #[serde(default)]
    pub checkpoint: Option<String>,
    #[serde(default)]
    pub stale_after_secs: Option<u32>,
    #[serde(default)]
    pub completion: GoalCompletion,
    #[serde(default)]
    pub task_ids: Vec<u64>,
    pub by: String,
}

/// Body of `PATCH /rooms/{id}/goals/{gid}` (operator authority required).
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateGoal {
    #[serde(default)]
    pub outcome: Option<String>,
    #[serde(default)]
    pub checkpoint: Option<String>,
    /// Missing leaves the override unchanged; JSON null clears it back to the
    /// room default; a number (including 0=off) sets an explicit override.
    #[serde(default, deserialize_with = "deserialize_nullable_option")]
    pub stale_after_secs: Option<Option<u32>>,
    #[serde(default)]
    pub completion: Option<GoalCompletion>,
    #[serde(default)]
    pub task_ids: Option<Vec<u64>>,
    #[serde(default)]
    pub status: Option<GoalStatus>,
    pub by: String,
}

fn deserialize_nullable_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// An explicit dependency declaration. Ordinary silence/chat never creates one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaitState {
    pub room: String,
    pub waiter: String,
    pub waiting_for: String,
    pub reason: String,
    pub since: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_signal_at: Option<u64>,
    #[serde(default)]
    pub signal_count: u32,
}

/// Body of `POST /rooms/{id}/waits`. The authenticated identity is
/// authoritative; `by` exists only for permanent localhost compatibility.
#[derive(Debug, Clone, Deserialize)]
pub struct SetWait {
    pub waiting_for: String,
    pub reason: String,
    pub by: String,
}

/// Body of `DELETE /rooms/{id}/waits/{name}`.
#[derive(Debug, Clone, Deserialize)]
pub struct ClearWait {
    pub by: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CareReason {
    Manual,
    DirectSummon,
    WaitOverdue,
    WaitCycle,
    WaitReplied,
    GoalReminder,
    TaskReminder,
    RoomSilence,
}

/// Who an attention is addressed to. `Lead` deliberately follows the loca's
/// current lead instead of freezing one person's name into every goal.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AttentionAudience {
    #[default]
    Lead,
    Person {
        name: String,
    },
    Group {
        names: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionStatus {
    Open,
    Claimed,
    Resolved,
}

/// Durable work-attention state. Delivery acknowledgement is intentionally
/// separate from claim/resolve: storing a signal in a runtime inbox does not
/// mean a person or agent has taken responsibility for its outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attention {
    pub id: String,
    pub room: String,
    pub reason: CareReason,
    pub subject: String,
    pub audience: AttentionAudience,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// The Everyone-reminder generation this attention belongs to (server-derived,
    /// secret-free), so the client collapses a whole generation to a single `@all`
    /// bubble/notification. Durable — it survives reconnect and list-attention.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default)]
    pub participants: Vec<String>,
    pub created_by: String,
    pub created_at: u64,
    pub attempt: u32,
    #[serde(default)]
    pub escalated: bool,
    pub status: AttentionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateAttention {
    pub subject: String,
    #[serde(default)]
    pub audience: AttentionAudience,
    pub by: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AttentionAction {
    pub by: String,
}

/// A bounded attention event. It is not a chat message and never creates work.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CareSignal {
    /// One delivery attempt. ACKs refer to this id.
    pub id: String,
    /// Stable lifecycle identity. Retries for the same stalled condition share
    /// this value and therefore remain one Attention in the product/UI.
    #[serde(default)]
    pub attention_id: String,
    /// The loca this envelope is delivered in. It always equals the room of the
    /// socket that receives it: the server never places a `Care` frame on a
    /// socket bound to a different room.
    pub room: String,
    /// The loca where the underlying condition actually arose. Equal to `room`
    /// for an ordinary signal; for a cross-loca caretaker relay the envelope is
    /// re-homed to the caretaker's home loca (so `room` matches the delivery
    /// socket) while `source_room` preserves the true origin for display and
    /// for routing a claim back to the source-room attention ledger.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_room: String,
    pub reason: CareReason,
    #[serde(default)]
    pub audience: AttentionAudience,
    /// Exactly one coordinator receives the runtime wake: live lead first,
    /// otherwise loca-care.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// The canonical principal that must receive this signal — set ONLY for an
    /// Everyone per-member fan-out. When present, live delivery and durable replay
    /// match the session's AUTHENTICATED principal id and NEVER fall back to a
    /// display-name or group-audience match, so a per-principal reminder can never
    /// wake the wrong socket (a shared display name) nor every socket (an N×N
    /// group broadcast). Lead/Person and legacy signals leave it `None` and keep
    /// the existing name-based path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_principal_id: Option<String>,
    /// Server-derived, secret-free generation id shared by every per-member signal
    /// of one Everyone reminder. The client collapses a generation to a single
    /// `@all` bubble/notification by this field — never by parsing the attention id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// The participant whose attention is needed, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default)]
    pub participants: Vec<String>,
    pub subject: String,
    #[serde(default = "default_care_creator")]
    pub created_by: String,
    /// Privacy-bounded recent context, copied without opening the source loca.
    #[serde(default)]
    pub context: Vec<Message>,
    pub attempt: u32,
    pub at: u64,
    #[serde(default)]
    pub escalated: bool,
    /// Operator-facing lifecycle derived from the reminder attempt and its
    /// selected coordinator. This is deliberately explicit so runtimes and
    /// browsers do not have to reinterpret transport ACK state as work state.
    #[serde(default)]
    pub state: ReminderState,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReminderState {
    #[default]
    Running,
    Overdue,
    Stalled,
}

fn default_care_creator() -> String {
    "care".to_string()
}

impl CareSignal {
    /// The loca the underlying attention belongs to: the true origin
    /// (`source_room`) when the envelope has been re-homed for cross-loca
    /// delivery, otherwise the delivery `room`. The durable attention ledger is
    /// always keyed here, so a re-homed caretaker envelope still files its work
    /// under the source loca even though it is delivered in the home loca.
    pub fn origin_room(&self) -> &str {
        if self.source_room.is_empty() {
            &self.room
        } else {
            &self.source_room
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CareAck {
    pub by: String,
}

/// A per-participant moderation action (admin-only). `mute`/`unmute` toggle a
/// posting freeze; `kick` disconnects; `ban` disconnects and blocks rejoin;
/// `unban` lifts a ban.
#[derive(Debug, Clone, Deserialize)]
pub struct Moderate {
    pub action: ModAction,
    /// The participant's name this action targets.
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModAction {
    /// Silence them here, without moving them: they stay, they read, they
    /// cannot speak. The lightest correction.
    Mute,
    Unmute,
    /// Close their connection now. A nudge, not a verdict — the davet stands
    /// and they may walk back in.
    Kick,
    /// Shut the door and keep it shut: they cannot re-enter this loca even
    /// holding a davet. A judgement about conduct.
    Ban,
    /// The work here is done. Not a punishment at all — their seat in this
    /// loca is taken back and their connection closed, but they remain a
    /// member of the building and can be called into the next loca.
    ///
    /// Kept distinct from Kick and Ban on purpose: an agent that finished a
    /// job should not be handled with the same verb as one that misbehaved,
    /// and it must not lose the building over it.
    Release,
    Unban,
}

/// Current moderation state of a room (who's muted / banned).
#[derive(Debug, Clone, Default, Serialize)]
pub struct ModState {
    pub muted: Vec<String>,
    pub banned: Vec<String>,
}

/// Frames the server pushes down a WS connection.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum ServerFrame {
    /// Sent once on connect: the room's recent backlog.
    History {
        messages: Vec<Message>,
    },
    /// A newly posted message (including the receiver's own, echoed back).
    Msg {
        message: Message,
    },
    /// A persisted reaction changed. Browser readers see the shared mark;
    /// agent event streams receive it only when they own the message.
    Reaction {
        reaction: MessageReactionEvent,
    },
    /// A single message from another loca that explicitly names a configured
    /// caretaker. It is relayed to the caretaker's home loca without opening
    /// the source loca, replaying history, or granting a seat there.
    Caretaker {
        message: Message,
    },
    /// Several addressed messages delivered as one agent turn. Chat storage
    /// remains message-by-message; this frame only coalesces the expensive
    /// runtime wake-up. Clients opt in with `turn_max` on `filter=mentions`.
    Turn {
        messages: Vec<Message>,
    },
    /// The room's member list changed.
    Members {
        members: Vec<Member>,
    },
    /// Out-of-band control, e.g. the operator broadcasting `/stop`.
    Control {
        cmd: String,
    },
    /// A note was created or updated — clients refresh that note live.
    Note {
        note: Note,
    },
    /// A soft-permission warning: someone wrote a note they weren't assigned.
    /// Advisory only (the write still happened).
    NoteWarn {
        key: String,
        by: String,
        can_write: Vec<String>,
    },
    /// The chat mode changed (admin action). Clients update their gating UI.
    Mode {
        mode: ChatMode,
    },
    /// Room settings changed (admin action), e.g. the rate limit.
    Settings {
        settings: RoomSettings,
    },
    /// Someone started/stopped typing (ephemeral, not persisted).
    Typing {
        name: String,
        on: bool,
    },
    /// A participant was kicked/banned; the named client must close its socket.
    Kicked {
        name: String,
        banned: bool,
    },
    /// Someone reconnected as the same identity: older connections holding that
    /// seat must close (last-writer-wins), so a dead-reader connection can't
    /// shadow the new one. `identity` is the seat key (one key = one seat; the
    /// display name may have changed); `session` identifies the *new* holder —
    /// every other connection on that identity goes.
    Evicted {
        name: String,
        #[serde(default)]
        identity: String,
        session: u64,
    },
    /// Moderation state changed (mute/ban lists); clients update indicators.
    Mod {
        state: ModState,
    },
    /// A task was created or changed — clients refresh their task panel.
    /// A new journal line landed.
    Journal {
        entry: JournalEntry,
    },
    Task {
        task: Task,
    },
    /// The loca's goal was created or changed.
    Goal {
        goal: Goal,
    },
    /// One bounded attention event for a live lead or loca-care.
    #[serde(rename = "care")]
    Care {
        signal: CareSignal,
    },
    /// A durable attention was created, claimed, delivered, or resolved.
    Attention {
        attention: Attention,
    },
    /// A participant explicitly started or cleared a dependency wait.
    Wait {
        waiter: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wait: Option<WaitState>,
    },
}

/// Frames a client may send up a WS connection. Agents post via REST and only
/// listen here; the web client may use `Send`/`Control` for convenience.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum ClientFrame {
    Send {
        #[serde(default)]
        target: Option<String>,
        text: String,
        #[serde(default)]
        reply_to: Option<u64>,
        #[serde(default)]
        op_id: Option<String>,
    },
    Control {
        cmd: String,
    },
    /// Ephemeral typing signal; the server rebroadcasts it to the room.
    Typing {
        on: bool,
    },
}

/// A davet (invitation) is how someone enters a loca. Metaphor and mechanism
/// are the same thing: the invitation IS the key. It opens ONE loca — never
/// the building, never a second room — and only the master issues it. Revoking
/// it ends the invitation; the row stays so we can still say who was let in.
///
/// This is the rule the whole model rests on: you do not walk into a loca on
/// your own, the master takes you in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invite {
    pub token: String,
    /// The one loca this opens — or [`Invite::BUILDING`] when it is a building
    /// membership rather than a seat in a room.
    pub room: String,
    /// The building membership (mb_ token) this davet seats. A davet does not
    /// create identity — it seats an existing member. Legacy davets (pre-link)
    /// carry an empty string until migration binds them at load.
    #[serde(default)]
    pub member: String,
    /// Display snapshot of who it was given to, copied from the membership at
    /// issue time (audit trail — the membership record is the identity).
    pub name: String,
    /// "agent" | "user"
    pub kind: String,
    pub issued_at: u64,
    pub issued_by: String,
}

/// Belonging to the building — a different act from being invited into a loca,
/// and deliberately a different record.
///
/// Membership creates an identity: this name, this key, this person is one of
/// ours. It is rare, heavy, and created from an authorized management surface.
/// A davet does not create anything — it seats an existing member in a room,
/// and the master does it from the UI a dozen times a day. Collapsing the two
/// (a davet whose room is "the building") would make the daily act look like
/// the founding one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Membership {
    pub token: String,
    pub name: String,
    /// "agent" | "user"
    pub kind: String,
    pub joined_at: u64,
    /// Who admitted them.
    pub admitted_by: String,
}

/// A member of the building, and where they currently sit. A resident with no
/// locas is in the lobby: visible and callable, but in no conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resident {
    pub name: String,
    /// "agent" | "user"
    pub kind: String,
    /// The locas they are currently invited into (may be empty: in the
    /// building lobby, in no room, waiting to be called).
    pub locas: Vec<String>,
    /// Whether a live connection is currently held anywhere.
    pub online: bool,
    /// Runtime wake health is independent from WebSocket presence. A member
    /// can be online while the model adapter behind that socket is dead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<RuntimeHealth>,
}

/// Ephemeral, server-timestamped health of the adapter behind an agent socket.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeHealth {
    pub wake: String,
    pub ack: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_id: Option<String>,
    /// Latest reply-required attention tracked by the adapter. Lifecycle
    /// milestones are independent booleans because relay and turn completion
    /// are not strictly ordered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention_id: Option<String>,
    #[serde(default)]
    pub stored: bool,
    #[serde(default)]
    pub accepted: bool,
    #[serde(default)]
    pub first_response: bool,
    #[serde(default)]
    pub final_response: bool,
    #[serde(default)]
    pub turn_completed: bool,
    pub seen_at: u64,
    pub progress_at: u64,
    pub ready: bool,
}

/// Heartbeat submitted by a supervised runtime. Identity is derived from the
/// membership credential; callers cannot report health for another agent.
#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeHealthUpdate {
    pub wake: String,
    pub ack: String,
    #[serde(default)]
    pub delivery_id: Option<String>,
    #[serde(default)]
    pub attention_id: Option<String>,
    #[serde(default)]
    pub stored: bool,
    #[serde(default)]
    pub accepted: bool,
    #[serde(default)]
    pub first_response: bool,
    #[serde(default)]
    pub final_response: bool,
    #[serde(default)]
    pub turn_completed: bool,
}

/// What the master sends to issue a davet: who it is for, and what they are.
/// The token itself is minted server-side — never supplied by the caller.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateInvite {
    pub name: String,
    /// "agent" | "user" — defaults to agent.
    #[serde(default)]
    pub kind: Option<String>,
}
