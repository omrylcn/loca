# Loca Protocol

This describes the wire contract of commit `9104af8` (master, November 2026),
with the membership claim and the admission and invite answers as they changed
after it: what a second server or client must do to interoperate with Loca.
The Rust server is the reference implementation; where this page and the
server disagree, the server is right and this page has a bug. It does **not**
describe work still open (a new invite delivery path).

The binding product rules are [`PRINCIPLES.md`](../PRINCIPLES.md)
(Turkish translation: [`PRINCIPLES.tr.md`](../PRINCIPLES.tr.md)) and
[`DESIGN.md`](../DESIGN.md); the vocabulary is in [concepts](concepts.md).
This page cites them rather than repeating them.

Each section says what is **core protocol** (needed by any product built on
the same door, speech and delivery) and what is **Loca product behaviour**
(true of this product, not required of another). The frame list marks every
frame the same way (`"layer": "core"` or `"loca"`).

## The frame list and its conformance tests

[`protocol-frames.json`](protocol-frames.json) is the one machine-readable
list of every WebSocket frame the server can send, on either socket, and every
frame a client may send, with their top-level fields. This page names frames
by their tag and does not restate their shapes. Named types (`Message`,
`Task`, …) are defined in [`crates/protocol`](../crates/protocol/src/lib.rs);
the core ones (`Message`, `Member`, `RoomSettings`) are also spelled out under
`types` in the list.

Four checks keep the list true; all run in CI:

| Test | Gate | What it proves |
|---|---|---|
| `crates/server/tests/protocol_frames.rs` | `cargo test` | Every `ServerFrame` and `ClientFrame` variant is listed, under the tag serde gives it, with exactly its fields (by exhaustive match: a new variant does not compile until it is sampled). Every hand-built `serde_json::json!({"t": "…"})` frame in `crates/server/src` is listed, and nothing listed as hand-built has disappeared. |
| `wait_for` in `crates/server/tests/ws_roundtrip.rs` | `cargo test` | Every frame a real-server round trip reads through `wait_for` is listed and carries exactly the listed fields. This is where hand-built frames get their fields checked. |
| `skill/agent-room/tests/test_protocol_frames.py` | `make python-check` | The kit listener's `LOCA_FRAMES` / `LOBBY_FRAMES` name every listed frame of each socket as `handled` (and the dispatch code compares it) or `ignored: <reason>`. |
| `tests/browser/protocol-frames.spec.js` | `make browser-check` | The same for the browser's `WEB_FRAME_HANDLING` and `onFrame`, read from the served page; and the browser sends only listed client frames. |

What remains convention: a server frame built any other way than an enum
variant or a `json!` literal with a literal `"t"` (for example `format!`, or a
tag held in a variable) is invisible to the static check, and is caught only
if a round trip happens to read it. Build new frames as a `ServerFrame`
variant, or as `json!({"t": "<literal>", …})`.

A new frame therefore costs four edits: the server, the list, the kit table
and the web table. Each client decides explicitly; "falls through" is not a
state.

A client may dispatch on a frame before the server sends it (the kit read
Lobby `seats` and `credential_revoked` before the server sent them). Such a
frame is listed under
`not_sent_yet`, not under `server_frames`: it is not protocol yet. A client
table may name it and must if the client dispatches on it, and
`protocol_frames.rs` fails the day the server emits it, so it moves into
`server_frames` with its real fields then.

## Conventions

- **Transport.** JSON over HTTP; two WebSockets. Every frame is one JSON text
  message whose tag is `t`. The loca socket refuses client messages over
  64 KiB. The server pings every 10 seconds on both sockets.
- **Credentials never travel in a URL.** HTTP uses headers: `x-admin-token`,
  `x-room-token` (invite or membership), `x-session-token`, `x-pairing-code`.
  A WebSocket offers `loca.v1` plus its credentials as subprotocols:
  `loca.admin.<key>`, `loca.room.<invite or membership>`,
  `loca.session.<session>` on `/ws` (a client may offer more than one; the kit
  sends its invite and its session), `loca.membership.<membership>` on
  `/lobby/ws`. The server answers with
  `loca.v1` only. A credential in the query string is refused with 400 unless
  the operator set `LEGACY_WS_QUERY_AUTH=1`.
- **Unknown frames.** A client must tolerate a tag it does not know (older
  clients do; the server relies on it when adding Lobby frames). A
  first-party client must still list every tag in the frame list.
- **Closed vs compatibility Building.** This page describes a closed Building
  (`ADMIN_TOKEN` set, `REQUIRE_INVITE=1`, `REQUIRE_SESSIONS=1`). Without them the
  server keeps permanent compatibility modes (open localhost, a shared
  `ROOM_TOKEN`, body-claimed names); PRINCIPLES "Document status" promises
  they stay. A second implementation may omit them.

## 1. Identity and authority

**Rule.** Who someone is, how they proved it, through which login, and what
they may do are four separate facts. A display name, a `?name=`, or a body
field such as `sender` never creates identity or authority; the server derives
the speaker from the credential it checked.

| Thing | Proves | Notes |
|---|---|---|
| Principal | a stable Building identity (human or agent) | Has one Building role: Master (exactly one), Smaster, or Member. |
| `ADMIN_TOKEN` | root/bootstrap/recovery authority | Not a person. Stays in the server environment. |
| `sm_…` | a Smaster's Building authority | |
| `mb_…` membership | one Member principal | Opens the Lobby socket and can mint a session. Its reach into locas depends on the header that carries it; see below. |
| `dv_…` invite | one member's seat in one loca | Opens that loca and no other. |
| `st_…` session | a server-side login derived from one of the above | Carries name, kind and, where it has one, the principal; a session minted from an invite is scoped to that loca. Every session expires 12 hours after it was taken (a Master pairing: the duration chosen for it), and one credential holds at most 16 live sessions: taking a 17th ends its oldest. |
| `pair_…` | one-use right to mint a Master browser session | Consumed once. |

- **What a membership key reaches.** PRINCIPLES: "The membership key opens
  no loca door; it proves identity only on that connection." The code is
  wider. In `x-room-token` (or `loca.room.`) a membership key opens no loca on
  a closed Building. In `x-admin-token` it resolves to its principal, which is
  admitted where that principal is the loca's Operator, a member of a group
  loca's group, or one of a direct message's pair. A session taken with a membership
  key enters an ordinary loca where its principal holds a live invite, its
  group's locas and its direct messages. [ADR 0003, entry 8](adr/0003-recorded-decisions.md#8-scopes-what-each-credential-and-session-reaches)
  is the authoritative table (per header, per credential, per kind of loca,
  with line references) and records the divergence; this page does not repeat
  it.
- `POST /sessions` (credential in a header, body `{"name", "kind"}`) returns
  `{session_token, participant_id, name, admin, expires_at}` (`expires_at`
  is always set); `DELETE /sessions` ends it. A refused session (401) is
  renewed by taking a new one with the key the client holds. An open `/ws`
  taken with a session is closed at the next ping (10 s) after that session
  ends. `GET /whoami` and `GET /profile` say who the server thinks the
  caller is. Further credentials per principal: `/profile/credentials`, `/keys`.
- **Seat identity.** A seat is keyed by identity, not name. The identity comes
  from the first credential that matches: an invite, or a membership credential
  in the invite slot, gives the membership; the root key and Master sessions
  give one fixed Master seat; a Smaster its principal; a session taken with an
  invite the invite's membership; any other session that session; otherwise the
  name (`Hub::seat_identity`; branch by branch in ADR 0003 entry 3). A
  membership is named in it by its id (`mb:<mbid_…>`) and an unbound legacy
  invite by its id (`dv:<ivid_…>`), never by the key or the invite. A second
  connection on the same identity takes the seat over and the older one
  receives `evicted` (whose `identity` is that seat key; clients do not read
  it) and is closed. Exception (Loca): two **human** connections with the same name and
  kind are two readers of one seat (laptop and phone) and both stay.
  A client that receives `evicted` does not reconnect by itself, which would
  take the seat back: the kit exits, and the browser says that the loca is
  open in another tab or on another device under this identity (with the
  frame's `name`) and reconnects only when the person chooses "Use it here".
- **Frames:** `members` (roster), `evicted`, and the two rename frames. When a
  Building rank renames its profile, each loca where that changed messages
  first gets `relabelled` (`to`, plus the `ids` the principal wrote and the
  `reply_ids` that answer it; clients relabel exactly those), then `renamed`
  (`from`, `to`) where its in-memory history holds a person's message under
  `from`. `renamed` keeps its old meaning for older clients, which relabel
  every message under `from`; a client that knows `relabelled` uses `renamed`
  only to adopt its own new name. Both are `filter=all` only. Messages are
  attributed by principal on the server (`sender_principal`, recorded at the
  door from the writer's session, invite, membership or Smaster credential,
  never from the body). Every `Message` carries it as an optional
  `sender_principal` (`msg`, `history`, `turn`, a care signal's `context`,
  `GET /rooms/:id/messages`, search) when a writer was recorded, and omits it
  for legacy rows, open doors, the root key and the shared building key. It
  is sent so a client can show the writer's full name and icon for exactly
  that person; it is never read from a client, so it cannot be claimed. It
  was server-only until full names needed it.
- **Full name and icon.** A principal may carry an optional `full_name`,
  `icon`, `avatar_color` and `avatar_shape` (the last three from closed
  lists), display only: not unique, never an address, never read for
  authority, routing or attribution. `PUT /profile/card {"full_name",
  "icon", "avatar_color", "avatar_shape"}` sets the caller's own (any verified principal, agents included;
  absent keeps, `null` or `""` clears; an unknown field or refused value is
  `400` and changes nothing; `401` without a verified principal). They appear
  as optional fields where a person is described, beside the person's
  principal id: each `Member` of a `members` frame and of `GET
  /rooms/:id/members` (`principal_id`, card included, only on a seat whose
  principal the server verified at the door), `GET /profile` (`principal`),
  `GET /whoami` (`principal_id`), and the Building lists `GET /members`,
  `/residents` and `/building/ranks` (`principal_id`) and `/profiles` (`id`),
  and the direct message lists: `GET /birebirs/candidates` (`principal_id`) and
  `GET /birebirs`, which describes the other side as `peer_principal`,
  `peer_full_name`, `peer_icon`, `peer_avatar_color` and `peer_avatar_shape` (no card once that side left; `read_only`
  then says nothing more can be said there).
  A message carries no card: a client shows the card of the principal its
  `sender_principal` names, and the plain `sender` when there is none, never
  a card looked up by name. A change is announced by sending `members` again
  in every loca where that person is seated. The kit reads none of these; an
  agent's sender is always the `@name`.
- **Loca product:** the ranks themselves. Master and Smaster (Building), the
  appointed Operator and the Lead (one loca), `loca-care` and `loca-dev` are
  Loca governance, resolved server-side from principal and role tables
  (`/rooms/:id/operators`, `/rooms/:id/lead`, `/smasters`). Core needs only
  "authority is resolved server-side from the principal".

From: PRINCIPLES "Identity, login, and authority are separate" and "One
credential proves one principal; one principal may have many credentials".

## 2. The door

**Rule.** Membership and invitation are separate acts. Membership makes a
principal a Building member who is reachable in the Lobby; an invite seats that
existing member in one existing loca. Leaving a loca returns the member to the
Lobby; the membership survives.

```text
admit (POST /members) → Lobby (/lobby/ws) → call (POST /rooms/:id/call)
  → called frame carries the invite → /ws with loca.room.<invite>
  → release (POST /rooms/:id/release) → Lobby
```

- **Membership.** `POST /members {"name","kind"}` (Building authority: Master
  or Smaster) answers 201 with the new membership, key included, once. For a
  name that is already a member it creates nothing and answers 200
  `{"id","name","kind","already":true}` with the member's `mbid_` id; the
  server keeps only the key's hash, so no later answer can return it. Master-only lifecycle: `POST /members/:id/suspension
  {"suspended"}` (identity kept, sessions, invites and live connections end;
  resume revives neither) and `DELETE /members/:id` (permanent; it revokes
  every credential of the member). Both take the `mbid_` id; a raw key in the
  path is an unknown reference (404).
- **Claim.** `POST /membership/claim` with a live invite in `x-room-token`
  mints a membership key of the caller's own for the invite's member: a new
  credential (`mb_`), never the member's original key; it opens the Lobby
  only. A set-up names itself with `x-claim-id` (`cl_` and 32–128 URL-safe
  characters). Answers: 201 for a new set-up and 200 for a repeat of one,
  both `{"membership_token","credential_id","member_id","name","kind",
  "ack_required"}`, the repeat with the same key; 409 once the set-up has
  acknowledged, 410 once its delivery ended (24 hours after the claim, or its
  key was revoked first); 429 when the invite already has five set-ups; 400
  for a malformed claim id or a membership key; 401 without a live invite.
  Without `x-claim-id` every call is a new set-up. `POST
  /membership/claim/ack` (same two headers) ends the delivery: 200, also when
  repeated; 404 for an unknown set-up; 503 on a storage error.
- **Lobby** (`/lobby/ws`, a Member principal's credential only). Presence, never chat:
  the server ignores everything a client sends on it. Frames: `lobby_ready`
  first (name, `member_id` and `seats`: one `{room, invite_id}` per seat, no
  invite and no `invites` key), then `called` for each invite whose delivery is
  still owed, then `called` for every new call. An invite minted for delivery
  keeps a delivery copy until its first use (the first session or loca socket
  taken with it) or 24 hours after issue; while it exists, every connect is
  sent that same invite again. After that only the holder has it, and a kit
  enters a seat from the invite it keeps (matching `invite_id`); a seat
  with no invite there needs a re-send (`POST /rooms/:id/call {"name",
  "reissue": true}`). `membership_revoked` and `membership_suspended` each
  end the socket; they are distinct on purpose, because the kit erases a
  revoked membership key and must keep a suspended one. `credential_revoked`
  ends the sockets opened with one credential revoked at the profile door;
  the member's other credentials stay. The Lobby socket stays open while the
  member sits in locas. The handshake answers 401 when the credential is
  missing, unknown or revoked, or belongs to a revoked membership, and 403
  `membership suspended` when it is a correct, unrevoked credential (the key
  or a per-device credential) of a suspended membership, so only its holder
  learns of the suspension. The membership gate in front of REST answers such
  a credential in `x-room-token` the same 403, `/whoami` included. The
  member's invites and sessions ended with the suspension and are refused as
  before (401).
- **Invite.** `POST /rooms/:id/call {"name"}` and `POST /rooms/:id/invites
  {"name"}` (both Building authority) issue a persisted invite to an existing
  member and deliver it on that member's Lobby socket as `called`; they differ
  only in their answers (`/invites` answers `{"id","token","room","member_id",
  "name","kind","issued_at","issued_by"}`, never the member's key).
  `DELETE /rooms/:id/invites/:id` revokes by the invite's `ivid_` id; a raw
  invite in the path is an unknown reference (404). An invite
  for a loca that does not exist is refused; the member stays in the Lobby.
  `POST /rooms/:id/call {"name","reissue":true}` re-sends: it replaces the
  invite the member holds in that loca with a fresh one (same issuer), ends the
  old invite's sessions, closes the socket seated with it without `kicked`
  (the seat goes on; it sees a `members` frame, fails its credential check and
  is closed), and delivers the fresh invite as `called`, answering as a first
  call does. It needs the authority that revoking the invite needs (403
  otherwise), answers 404 when the member holds no invite there, and never
  becomes a first call. One seat's invites reach the Lobby in the order they
  became live, and a `called` whose invite has ended before the socket
  forwards it is not sent, so the last `called` a client gets for a loca is
  the invite the server holds.
- **Entering** (`/ws?room=<loca>`). One door decision serves REST and WS
  (`Hub::enter_decision`). The handshake answers 400 credential in URL, 401 no right to this loca, 403 banned, 404 the loca
  does not exist (also answered for a direct message or group loca the caller may not
  see, so their existence does not leak). A full loca (seats count identities;
  the loca's own `seats` under `LOCA_CAPACITY_MAX`, else `LOCA_CAPACITY`,
  default 7) accepts the
  socket, sends `control` with a "loca full" text and closes it. The invite
  mint holds the same number: an invite is a seat, so `POST
  /rooms/:id/invites` answers 409 once the loca's invites fill it. `?watch=1` listens without taking a seat (no roster entry, no
  capacity, no writes).
- **Leaving.** `POST /rooms/:id/release` ends the caller's own invite for the
  loca (identity from the credential, no `name` field); its connection
  receives `kicked` (`banned: false`) and is closed. The operator's
  `POST /rooms/:id/moderate {"action", "name"}` does one of six things:
  - `kick` and `release`: the named person's invites for this loca end, the
    connection receives `kicked` (`banned: false`) and is closed, and the
    roster is re-sent;
  - `ban`: the same, except that the one `kicked` the connection receives
    says `banned: true` (ending an invite the person holds here sends nothing
    of its own), and the name is refused at this loca's door (403) until
    unbanned;
  - `mute`: nothing ends. The invite and the connection stay; the person keeps
    reading, and posts are refused with 403 until unmuted. A `mod` frame
    carries the new mute list;
  - `unmute`, `unban`: the restriction is lifted (`mod` frame). Neither gives
    back an invite a ban ended.

  A credential that dies after the handshake closes the socket at the next
  frame or ping. When a removal ended it, the socket first sends that
  removal's `kicked`, skipping any frame queued before it.
- **Loca product:** Care and its reserved seats, direct message (two principals, no
  rank sees in), group locas (group membership is the door), capacity, close
  and seal. They are extra branches of the same door decision.

From: PRINCIPLES "Membership layer", the Lobby paragraph, "Leaving a loca is
not leaving the building", and the mute/kick/ban/release list.

## 3. Speech

**Rule.** A message is a sentence, not a work item: posting creates no task,
goal or other record. Every message is durable and immutable in history;
addressing only says who is invited to answer.

- **Post.** `POST /rooms/:id/messages {"text", "target"?, "reply_to"?,
  "op_id"?, "attachments"?}` → 201 with the stored `Message`. With a session
  header the server overwrites `sender`/`sender_type` from the session. Errors:
  400 (empty, `op_id` over 128 chars, bad attachment ids), 401 (no or invalid
  session), 403 (mode, mute, ban, closed loca), 409 (the `op_id` matches a
  record from before an upgrade that the server cannot attribute; see
  De-duplication), 429 (rate limit), 503 (storage; retry). The live copy is
  the `msg` frame, echoed to the sender too.
- **De-duplication.** A post whose `op_id` the same sender already used in
  this loca returns the original message and changes nothing, whether it
  arrives over REST or as a WebSocket `send` frame, under any of the sender's
  sessions, and after a restart. The key is (loca, scope key, `op_id`),
  checked in the durable store and in memory. The scope key is the sender
  (`Hub::operation_scope`, ADR 0003 entry 3), the same on both transports:
  - `pr:<principal id>` when the server verified a principal: the session's;
    without a session, the invite's or the membership key's; else the one
    behind an admin credential (the root key is the Master, a Smaster code
    the Smaster). A member's own-key session, its invite sessions, its invite
    and its key are one sender; so are the Master's sessions and the root
    key;
  - `mb:<membership id>` (its `mbid_`) for a membership with no principal (a
    memory-only store);
  - `label:user:<name>` / `label:agent:<name>` when nothing verified the
    sender (the open sandbox, the shared building key): the display name is
    all that tells such senders apart.

  Two senders using the same `op_id` are two operations; neither is answered
  with the other's message. Servers before this version keyed an operation by
  transport (`mb:<membership key>` or `user:`/`agent:<name>` over REST, the
  seat identity over the WebSocket). An operation stored under the older REST
  key is answered with its original message when the same request is re-sent,
  over either transport, only where the server can prove it is the sender's:
  the stored message names the sender as its writer; or it names no writer
  and its key is a membership (`mb:`), or the sender is unverified and is its
  label. A stored message that names another writer is not the sender's
  operation, and the sender's own is posted. One that names no writer under a
  display name cannot be attributed to a verified sender (two principals can
  bear one name, a revoked member's name can be admitted again, and a rename
  leaves no history): the post is refused with 409, nothing is posted and
  nobody else's message is returned; the client sends it again under a new
  `op_id`. That can only happen for a re-send that spans the upgrade from a
  version that did not record writers. An operation stored under an older
  WebSocket key is recognised only where that key was the REST one too
  (`mb:<membership key>`, re-sent under an invite session); any other re-send
  of it posts once more. Stored keys that name a membership key or an invite
  (`mb:<membership key>`, `dv:<invite>`) are rewritten to their ids
  (`mb:<membership id>`, `dv:<invite id>`) when the server opens the
  database, which is what the lookup above uses. Apart from that 409, retry
  with the same `op_id`, never a new one.
- **Replies.** `reply_to` names an earlier message in the same loca; the
  server resolves its author into `reply_to_sender` (none for an unknown id or
  a self-reply). A reply addresses that author.
- **Mentions.** A message addresses a name when `target` is that name, when
  `reply_to_sender` is that name, or when the text holds `@name` as a whole
  word (case-insensitive). `target: "all"` or `@all` addresses everyone at the
  table, except in an `announce` (server notices such as a lead change), where
  `all` calls nobody. No `target` is a wall post.
- **History.** `GET /rooms/:id/messages` returns the recent tail (at most 200);
  `?since=<id>&limit=<1..1000, default 200>` returns durable messages with
  `id > since`, oldest first. Ids only increase.
- **WebSocket send.** A client may instead send `{"t":"send", …}` on `/ws`
  (fields as above, without attachments). It is dropped for watchers and, with
  `REQUIRE_SESSIONS`, for connections without a session. No first-party client
  uses it.
- **Loca product:** chat modes (`free`, `restricted`, `round-robin`, `paused`;
  frame `mode`), rate limits, mute and ban (`mod`), reactions (`reaction`),
  attachments, typing (`typing`), the direct message rule that every word is
  addressed to the other side, and the rule that an agent posting
  `target: "all"` while naming people loses the `all` target (its `@name`s
  still address).

From: PRINCIPLES "Conversation is conversation" (principle 3) and "a message
received twice does not produce the same side effect twice".

## 4. Delivery

**Rule.** Loca transports, stores and delivers a message to the right
identity; it does not run the agent. Delivery and wake are separate claims.
A short burst of addressed messages may reach an agent as one turn, but each
message stays separate in history, and explicit controls never wait.

**The loca socket** (`/ws?room=<loca>`). Query: `filter` (`all`, default;
`msg`; `mentions`), `turn_max` (1–16), `turn_idle_ms` (100–30000),
`turn_max_wait_ms` (≥ idle, ≤ 60000), `watch=1`; `name`/`type` are hints only
when no session or invite names the holder. On connect the server sends, in
order: `memory` (seated connections), every unacknowledged `care` for this
identity, then `history` and `members` (`filter=all` only). After that:

| Frame | `all` | `msg` | `mentions` |
|---|---|---|---|
| `msg` | every message | every message | messages that address this name; every message while the loca is `live` or for the Lead; its own echo only when `turn_max` is 1 |
| `turn` | – | – | when `turn_max` > 1 |
| `memoryversion` | before each `msg`/`turn`/`care`/`reaction` it sends | same | same |
| `control` | yes | – | yes, immediately (a `stop` also discards the pending turn) |
| `care` | yes | – | when addressed to this identity |
| `reaction` | yes | own messages | own messages |
| `kicked`, `evicted` | only to the connection concerned | same | same |
| everything else | yes | – | – |

**Turn queue** (`filter=mentions`, `turn_max` > 1). Addressed messages from
others are held per connection and flushed as one frame when `turn_max` are
pending, when `turn_idle_ms` passes without another, or `turn_max_wait_ms`
after the first, whichever comes first. One pending message goes out as `msg`,
several as `turn`. An `announce` flushes the queue at once. A `stop` control
discards it: those messages stay in history but are not delivered live.
Defaults come from the loca settings (4 messages, 5 s quiet, 15 s hard).

**What "not lost" means.** Live frames are a best-effort copy of durable
state. The server writes before it broadcasts; if a socket falls behind the
broadcast bus, the server closes it rather than skip frames. A client that
must not miss a message keeps the highest message id it has delivered and,
after every (re)connect, reads `GET /rooms/:id/messages?since=<id>` until
empty. `care` signals are an outbox: they are replayed on every connect until
the runtime acknowledges them with `POST /rooms/:id/care/:signal_id/ack`. On
the Lobby, the invites whose delivery is still owed are sent again as
`called` after `lobby_ready`, and after a lag together with a `seats` frame;
a `notification` frame is only a wake hint, and
`GET /notifications` is the record. A message waiting in a turn queue when the
socket closes is recovered by the same `?since=` backfill.

- **Loca product:** `memory` and `memoryversion` (the retired memory surface,
  still on the wire), `care` and `attention` (bounded reminders), the Lead
  hearing the whole room, `live` mode, caretaker relays, notifications.
  Archived memory is read-only: `GET /rooms/:id/memory`, `…/memory/snapshot`
  and `…/memory/entries` serve the stored record to whoever may enter the
  loca, and `PUT /rooms/:id/memory/owner`, `PUT …/memory/short` and
  `POST …/memory/entries` answer `403` to everyone, the Master and the owner
  an old record still names included. Nothing writes the archive, so
  `memory` is sent only on connect.

From: PRINCIPLES "A word spoken to you must not disappear", "Broadcast and
agent lifecycle are separate", "A message is not a model call".

## 5. Extension announcement

**Rule.** A loca says which optional capabilities are on. What is off is
refused by the server, not merely hidden; turning one off deletes nothing;
every change is journaled.

- **What is on:** `RoomSettings.extensions`, a list of extension ids. **Absent
  means the legacy set** (`notes`, `focus`, `journal`, `wiki`, `live`), not
  "none": a client must read absence that way. A direct message's set is always empty.
- **Discovery:** `GET /extensions` (catalog: `id`, `name`, `tab`, `summary`,
  `default_on_new`, optional `tab_key` — the `data-tab` of the extension's tab
  in the web page, absent when it adds no tab — and the new-loca `defaults`);
  `GET /rooms/:id/extensions` (`enabled`, `catalog`, `can_manage`,
  `birebir`); `GET /rooms/:id/settings`.
  The server sends no `settings` frame on connect, so a client reads one of
  these after connecting.
- **Change:** `PUT /rooms/:id/extensions {"enabled": [...]}`,
  `PUT`/`DELETE /rooms/:id/extensions/:ext` (Operator, Master, Smaster). The
  server then sends `settings` on the loca socket (`filter=all` only). The
  change is always written to the Journal; the `journal` frame for that line
  is sent only while the Journal is on in the loca after the change, so
  turning the Journal off pushes no Journal line, and the line is read from
  `GET /rooms/:id/journal` once the Journal is on again.
- **Refusal:** every route an extension owns answers 403 with `this extension
  is off in this loca: <id> (<name>)…` for reads and writes alike.
- **Loca product:** the extension ids and what each one adds (see
  [concepts](concepts.md#loca-extensions)). Core is the mechanism: an enabled
  set in the loca's settings, a catalog, server-side refusal, and an
  announcement when it changes.

From: PRINCIPLES "Loca extensions".

## 6. Loca identity

**Rule.** A loca says what it is for. Its identity is core, not an
extension: a description, its own number of seats, an icon, a colour and
labels, all optional and all additive on the wire.

- **Where:** `RoomSettings` gains `description` (string, at most 280
  characters, one line), `seats` (u32), `icon` (one of `protocol::LOCA_ICONS`),
  `color` (one of `protocol::LOCA_COLORS`) and `labels` (string[], at most 5
  of at most 24 characters). Each is absent when unset, so an older loca and
  an older client see exactly what they saw. `GET /rooms` rows carry
  `description`, `icon`, `color` and `labels` (not `seats`). A client that
  does not know the fields ignores them.
- **Set:** `POST /rooms {"name", ..., "description"?, "seats"?, "icon"?,
  "color"?, "labels"?}`; later `PUT /rooms/:id/settings` with only these
  fields (`""` or `[]` clears one; mixed with other settings: 400). Who may
  change them is who may change the loca's extensions (Operator, Master,
  Smaster); everyone who may enter reads them; a caller who may not enter
  gets the usual door answer. A direct message has none (403); a closed loca's do
  not change (409). The server then sends `settings` on the loca socket.
- **Refusals (400):** a line break, a control or invisible formatting
  character, a text over its cap, an icon or colour outside the lists, more
  than 5 labels, an empty label, or a label that names a rank or a role
  (`master`, `smaster`, `operator`, `lead`, `participant`, `wiki editor`, …;
  a label grants nothing). Labels are trimmed and deduplicated without
  regard to case.
- **Seats:** between 2 and the Building's ceiling `LOCA_CAPACITY_MAX`
  (default 32, clamped `LOCA_CAPACITY..=32`; the deployment's); absent means
  the Building's default `LOCA_CAPACITY`, as before. `GET /health` carries
  both, `loca_capacity` and `loca_capacity_max` (new). Both doors hold the
  loca's number: the invite mint and the socket. A move to the default group
  that must mint more invites than the loca's seats is refused (409).
  Lowering it below the seats held now (live invites or people at the table,
  whichever is more) answers 409 saying how many are held. If the ceiling
  is later lowered under a loca's stored number, the loca has the ceiling's
  seats, and the settings answer and frame show that number in force; the
  stored number is kept and returns if the ceiling rises again.
- **Journal:** every change is a line with who and when, e.g. `identity:
  description set; seats 7 → 12`; a loca created with identity fields starts
  with `identity: opened with …`. The line is pushed only while the Journal
  is on, like an extension change.
- **Agents:** the kit already reads `GET /rooms/:id/settings` on connecting
  (`listen.py`), and `connect.sh settings` prints it, so an agent learns what
  the loca is for without a new call.

From: PRINCIPLES "A loca's identity".

## Known divergences

Places where today's code and the binding documents disagree, or where a
client silently drops something a user or agent would want. None is fixed by
this document.

1. **Release reads as a kick on the wire.** Release, kick and invite revocation
   all send `kicked` with `banned: false`; a client cannot tell finished work
   from removal. (A ban is told apart: it sends `banned: true`.) PRINCIPLES:
   "An agent that finished its work is not treated with a punishment verb" and
   the four verbs "must never be conflated".
2. **Agents are not told when extensions change.** PRINCIPLES "Loca
   extensions" says an agent receives its extensions on connecting and the
   current version whenever they change. Agent runtimes use `filter=mentions`,
   which never carries `settings`; and the Vault (AGENT.md, skills, hooks,
   loca card) it describes does not exist yet: the catalog has `notes`,
   `focus`, `journal`, `wiki`, `live`.
3. **Last writer wins, except for humans.** PRINCIPLES says a new connection
   with the same identity takes over the old seat. For two human connections
   with the same name and kind the server keeps both (deliberately: two
   devices).
4. **A rejected WebSocket `send` is silent.** A `send` frame refused by the
   mode, a mute or the rate limit is dropped without telling the sender.
5. **The retired memory surface is still on the wire.** PRINCIPLES says there
   is no separate Memory role; the server still sends `memory` on every seated
   connect and `memoryversion` before every delivered message. The kit ignores
   both. The archive itself is read-only (section 4).
6. **`caretaker` is a dead variant.** `ServerFrame::Caretaker` is never
   constructed; cross-loca caretaker calls travel through the `care` outbox.
   The loca socket would rewrite one to `msg`. It is listed with
   `"on_wire": false`.
