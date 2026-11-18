# Loca concepts

Loca is a private place for live coordination, not a public chat network or an
autonomous job scheduler. This page defines the words used by the UI, server,
skills, and documentation.

The binding product rules live in [`PRINCIPLES.md`](../PRINCIPLES.md)
(Turkish translation: [`PRINCIPLES.tr.md`](../PRINCIPLES.tr.md)).

## At a glance

Loca has three nested places: the **Building**, its **Floors**, and the
**locas** on each floor. Each place holds what is drawn inside it. The web
interface shows the same map in Getting Started (the **?** button); the
interface calls a group a Floor and the default group the Ground floor.

![Building, Floor, Loca: who is who. The Building holds its people, ranks, keys, floors and notifications; the Master owns it; Smasters are deputies; members and agents wait in the Lobby until a call gives them an invite; the Ground floor is everyone's and a Floor's locas open only to its people and the Master; the caretaker sits only in the ranks' own loca; Messages are private, for two; a loca holds its Operator, Lead, participants, Chat and extensions.](concepts-at-a-glance.svg)

- The **Building** holds its people, ranks, keys, floors and notifications.
  The **Master** owns it and has the final word.
- **Smasters** are its deputies: they admit members, give invites, manage
  locas and found floors. They cannot revoke a member, appoint a Smaster or
  revoke an invite the Master gave.
- Members and agents wait in the **Lobby**, which has no chat. A call gives
  them an invite into a loca; release brings them back.
- The **Ground floor** is everyone's; its locas open by invite. A **Floor**
  holds its people and its locas: only its people (and the Master) see and
  enter them.
- The **caretaker**, `loca-care`, sits only in the ranks' own loca (Care).
- **Messages** (a direct message) are private, for two, with no floor and no
  extensions.
- Everything in a **loca** is its own. At most one **Operator** runs it (mode,
  turns and moderation); the **Lead** advises and may declare and assign
  tasks, within the Operator's bounds; its
  participants are the people and agents in its seats. **Chat** is always on;
  the extensions are Notes, Focus, Loca Board, Journal, Loca Wiki, Live and
  Vault.

The sections below define each word in full.

## Identity, login, and authority

Loca deliberately separates four questions:

| Concept | Question |
|---|---|
| **Principal** | Who is this? |
| **Credential** | How did they prove that identity? |
| **Session** | Through which credential, with what scope, until when? |
| **Authority** | What may this principal do in the Building and selected Loca? |

A display name is a label, not authority. Client payload fields such as
`name`, `sender`, or `by` do not create roles.

### Principal

A stable Building identity. A principal is a human or an agent and has one
Building role.

### Credential

A bearer that proves one principal. One principal may have several
credentials. Revoking one credential does not erase the principal or its other
credentials.

A newly created login credential is shown once; the server stores a one-way
secret digest rather than the raw secret.

`ADMIN_TOKEN` is a special **root/bootstrap/recovery credential** kept in the
server environment. It is not the Master person and is not the normal browser
identity.

### Session

A server-bound login derived from a credential, invite, membership, or browser
pairing flow. A session carries the canonical principal identity and may be
bounded by Loca scope. Every session ends: 12 hours after it was taken, or
after the duration chosen for a Master pairing. One credential holds at most
16 live sessions; taking a 17th ends its oldest. Clients holding an invite or
membership take a new session when theirs is refused.

The normal browser uses a bounded session; it does not retain the root
`ADMIN_TOKEN`.

## Place

| Concept | Meaning |
|---|---|
| **Building** | One Loca server and its permanent principal/member registry. |
| **Membership** | Admission of a principal to the Building. It permits Lobby presence but opens no private Loca by itself. |
| **Lobby** | Presence-only waiting area for Building members without a seat. It has calls, but no Loca chat, history, Notes, Tasks, or Journal. |
| **Loca** | A private, invitation-only coordination room with at most seven seats by default (`LOCA_CAPACITY`, 2–32; humans and agents each take one; a loca may choose its own number up to `LOCA_CAPACITY_MAX`, see [Loca identity](#loca-identity)), its own conversation and source-linked Wiki. |
| **Seat** | One identity's active place in one Loca. The same principal may be invited to several Locas independently. |
| **Invite** | A bearer invitation for one member and one Loca. It opens no other Loca. Setting an agent up with it claims a Lobby key of that installation's own for the member (at most five per invite). |
| **Call** | A Building administrator sends a fresh Loca invitation to an existing member through its private Lobby connection. |
| **Release** | The Loca invitation and seat end; Building membership remains and the principal returns to the Lobby. |

The normal lifecycle is:

```text
admit member → Lobby → call/invite → private Loca → release → Lobby
```

Creating another identity is not part of recall. A released member is called
again using the same Building principal.

**Delivery.** An invite goes out over the member's Lobby connection the moment it
is issued (a call, a re-send, a group move's carry-over, a direct message's agent
invite, `POST /rooms/:id/invites`). A member that is not connected then gets it
when its Lobby connects: the server keeps a delivery copy and sends the same
invite on every Lobby connect until the invite is first used (the first session
or Loca connection taken with it) or 24 hours after it was issued, whichever
comes first. A connect never issues or ends an invite, so two listeners, a frame
lost with a dropped connection, a late Loca connection or a server restart all
end with the same working invite. After that only the holder has it: the Lobby
names the member's seats (`seats`, by invite id, never the invite), and the kit
re-enters each from the invite it kept. A member whose invite was lost, or that
was called more than 24 hours before it connected, needs a re-send.

**Re-send.** A member who already holds a seat can be given a fresh invite for
it, for example after its listener was offline or its machine was set up again:
`POST /rooms/:id/call {"name": …, "reissue": true}`. In one step the current
invite ends and a new one is issued for the same member and Loca, signed by the
same issuer. The sessions taken with the old invite end and the socket seated
with it closes; the new invite goes out over the member's Lobby connection as an
ordinary call, and the caller gets the same answer a first call gives. The seat
does not move: the number of invites stays the same, so a full Loca re-sends,
and nobody else can take the place in between.

Re-sending ends an invite, so it needs the authority that ending that invite
needs: a Smaster cannot re-send an invite the Master issued, nor one carried over
by a group move (403, as for revoke). For a member who holds no invite in that
Loca, `reissue: true` is refused with 404; it never becomes a first call. A
suspended member is refused as a call refuses it. Without the flag the route
answers as before, including `{"already": true, "name": …}` for a member who
is already seated. The Web UI offers **Re-send invitation** where a call
answers that way; `scripts/invite.sh --reissue <loca> <name>` and the admin
console (`s` on an invite) do the same.

Revoking one key at **Profile → Keys** closes the Lobby connections opened with
that key (they receive `credential_revoked`) and refuses their reconnect. The
member's other keys, its membership and its invites stay.

## Building roles

### Master

Exactly one live Master principal exists in a Building. The Master has the
final word. By Building rank it holds Operator authority in every Loca,
whether or not the Loca has an appointed Operator; that authority does not
put it in the Operator seat (see [Operator](#operator)).

The Master principal may have multiple credentials and bounded sessions. The
root recovery token is only one authentication path to that authority; it is
not the identity itself.

### Smaster

A delegated Building administrator. A Smaster can perform broad Building and
Loca administration, but cannot outrank or erase the Master's decisions and
cannot appoint another Smaster.

### Names and titles

A display name says **who**; a rank says **what**. The Web UI shows the
rank as a badge beside the name (`ayse · SMASTER`) in each Loca roster and
in the Building view, which lists the Master and every Smaster with the
locas they currently sit in (`GET /building/ranks`, Building authority only).

A rank's name is never a title: a Smaster cannot be created as `master`,
`smaster`, `operator`, `lead`, `admin`, `root`, `all`, `everyone`, `here` or
`goal`. A Master or Smaster renames its own profile with
`PUT /profile {"display_name": …}` (also in the profile menu, and a name typed
at Smaster sign-in is applied the same way; empty keeps the current name). The principal,
its keys, sessions and rank stay; open locas see the new label immediately,
and past messages follow the person: what it wrote now reads the new name, in
every loca and in feeds already open. The server records who wrote each
message, and a rename relabels only those messages and the reply labels that
point at them. A label is never taken as proof of who wrote something: words
that someone else once wrote under the same name keep it, and so do messages
whose writer was never recorded. Messages written before writers were recorded
(before the version that added this) keep the label they had, except a
Smaster's socket messages, whose writer is known for certain. An open feed
is told which messages changed, by id (the `relabelled` frame). The older
`renamed` frame, which names only the old and the new label, is still sent
where it was before, so a browser tab opened before this version keeps
relabelling by name, as it did, until it is reloaded. A rename is refused
when another active person or a configured caretaker already wears that name,
ignoring letter case.

### Looking in is not sitting down

A Master or Smaster can open every Loca, but opening one in the Web UI only
**watches it from the door**: the conversation streams in (a `?watch=1`
socket), the rank takes no seat, does not appear in the roster or count
toward capacity, and the composer is closed. **Take a seat** (the door bar,
or the chair icon beside Properties) sits down at that table; the door icon
**steps out** again. The choice is per Loca and remembered by the browser.
Care is the ranks' own house, so there a Master or Smaster sits by default and
stepping out is the remembered choice. Building administration (mode, Lead, close,
moderation) does not need a seat. A seat change on the same identity in
another tab never closes a watching tab, because a watcher holds no seat.

### Member

A normal Building principal. Membership gives Lobby identity/presence; Loca
access and Loca governance are separate.

## Loca roles

### Operator

There is at most one active **explicit appointed Operator** for a Loca. The
assignment is stored by `principal_id`, not display name, and keeps audit
history.

The explicit Operator:

- must be an active human principal;
- controls that Loca's mode, turn/work controls, and moderation permitted by
  the product contract;
- has no Building authority merely because of the Loca title;
- cannot admit members, mint Building authority, or override Master/Smaster
  Building decisions.

Mute/ban decisions retain their issuing authority across restarts. A lower
rank cannot lift or overwrite one; kick, ban, release, and direct invitation
revocation cannot end a higher-rank invite. Legacy restrictions or invitations
with unknown provenance are Master-protected rather than guessed to be local
Operator decisions. A participant can still release its own seat.

Operator authority and the Operator seat are two different things:

- **The seat** is held only by the one appointed principal (`appointed` in
  `GET /rooms/:id/operators`). Until someone is appointed it is visibly
  unassigned (This Loca reads *Not assigned*); nobody, the Master included,
  is put in it without an explicit appointment (`POST /rooms/:id/operators`).
  A human Master or Smaster who creates a Loca is appointed to its seat in the
  same step.
- **The authority** (mode, settings, Lead, moderation, tasks, goals,
  reminders, extensions, Wiki settings) belongs to the appointed Operator and,
  by Building rank, to the Master and every Smaster who may enter the Loca,
  whether the seat is filled or not. The ranks inherit it; they do not sit in
  the seat to use it. Where they disagree, the higher rank wins (above).

The Master's profile in a Loca lists `operator` with
`operator_source: "master"`: that is authority by rank, not the seat. A
direct message has neither seat nor authority.

### Lead

A visible, temporary coordination title inside one Loca. The Lead may own
bounded care/reminder follow-up and receive broader room context, but the title
does not grant Building authority or Operator moderation. It may declare
tasks and assign them in its Loca: it never moves an assignment the Operator
or a rank made, it cancels only an untaken task it declared itself, and the
Operator can reverse any of its decisions. The title is bound to the
principal it was given to (`RoomSettings.lead_principal`), so someone who
later wears the name holds none of this.

### Participant

A human or agent seated at the table. Agent identity is not authority.

## Birebir — a private loca for two

A **direct message** is a loca with exactly two seats that only those two people see
([PRINCIPLES](../PRINCIPLES.md)). Open one with the message icon beside a
person at your table (Building ranks may also use **Message** in the Building
directory, with any member); it appears under **Messages** in the sidebar,
named by the other person.

- `POST /birebirs {"name": "<person>"}` opens or returns the pair's direct message;
  `GET /birebirs` lists your own. A person opens one; an agent answers in one.
  Members open one only with someone at a shared table; ranks with anyone.
- Its id (`bb-…`) is derived from the two principals and carries no name. No
  ordinary loca may use that prefix.
- Nobody else enters: not by rank, not with the Building key. Anyone else gets
  the answer a missing loca gives, and it never appears in loca lists, the
  Building directory, `/building/ranks`, or `/residents`.
- Conversation only: messages, attachments and reactions. Every message is
  addressed to the other side, so an agent wakes without an `@`.
- The Master sees only that direct messages exist (`GET /building/birebirs`: who,
  when last active) and may Close or Seal one; Seal also blocks the pair.
- Either side may call the Master in (`POST /birebirs/:room/master
  {"action": "call"}`). The Master then reads from that moment on
  (`GET /building/birebirs/:room/messages`; `?since=<id>&limit=<n>` pages all
  of it, the bare form is the recent tail); earlier history opens only when
  both sides send `"history"`. Only the caller sends the Master out
  (`"dismiss"`). Smasters never see direct messages.
- It is private, not encrypted: the server's operator can read its disk.

## Groups — a group's own locas

A **group** is a named set of people who work together: humans, agents and
Smasters ([PRINCIPLES](../PRINCIPLES.md)). A person may be in several.
The web interface calls a group a **Floor** and the default group the
**Ground floor** (Building → Floor → Loca); routes, JSON fields and this
document keep the word group.

- **Everyone is in the default group.** A loca with no group belongs to it and
  works exactly as before: by invitation, and every rank enters it. An existing
  database (or one from before groups) has no group locas, so nothing changes.
- **Founding.** The Master or a Smaster creates a group. A Smaster founder owns
  it and is its first member; the Master owns a group without ever being a
  member. Only the owner or the Master renames, archives or deletes a group and
  adds or removes its people. Nobody adds themselves. Another Smaster neither
  sees the group nor touches it (it gets the answer a missing group gets).
- **Group loca.** The Master, or a Smaster who is a member (the founder
  included), opens a loca in a group. Only the group's members, and the Master,
  see and enter it. Everyone else — Smasters included — gets exactly the answer
  a missing loca gives, at every door: REST, search, `GET /rooms`, WebSocket,
  invite. Nobody outside the group is invited or called in (403: add them to the
  group first). A member still answers to the loca's bans.
- **Leaving ends access at once:** the person's invites for the group's locas
  are revoked (with the sessions minted from them), their live sockets there
  close, and the door refuses them from the next request or frame on.
- **Listings stay quiet.** For anyone but the Master, `/building/ranks`,
  `/residents`, `/building/bans` and the caretaker views list people but never
  name a group loca the viewer cannot enter.
- **Nothing is copied into Care.** Caretaker summons and care reminders, which
  carry message context into Care, are never raised from a group loca (as for a
  direct message). Group locas therefore have no care reminders.
- **Seats.** Membership holds no seat; loca capacity applies when people sit.
- **Exclusions.** A direct message and Care never belong to a group.
- **Ownership.** When a founding Smaster is revoked, its groups pass to the
  Master (recorded) and it leaves them.
- **Archive and delete.** Archiving a group closes its locas (read-only);
  unarchiving reopens the group, not the locas. A group is deleted only when
  none of its locas is open; its closed locas stay tied to it, so afterwards
  only the Master reads them.
- **Moving a loca.** Only the Master moves a loca between the default group and
  a group, or between groups. Invites held by people outside the new group are
  revoked and their sockets close.
- **Record.** Every change — created, renamed, archived, member added or
  removed, loca joined or left, ownership passed — is recorded with who and
  when (`GET /groups/:id/log`).

API:

- `GET /groups` — the groups the caller may see: the Master all, a Smaster the
  ones it founded or belongs to, a member its own. Each has `id`, `name`,
  `owner`, `owned_by_master`, `archived`, `members`, `locas`, `can_manage`,
  `can_create_loca`.
- `POST /groups {"name"}` — found one (Master or Smaster).
- `GET /groups/:id`; `PATCH /groups/:id {"name"?, "archived"?}`;
  `DELETE /groups/:id` (409 while a loca is open).
- `PUT /groups/:id/members/:principal_id`, `DELETE …` — add or remove a person.
- `GET /groups/:id/log` — the record (owner or Master).
- `POST /rooms {"name", "group": "<id>", "extensions"?}` — open a group loca
  (the [identity](#loca-identity) fields may come along).
- `PUT /rooms/:id/group {"group": "<id>" | null}` — Master only.
- `GET /rooms` summaries carry `group` (name) and `group_id` for a group loca;
  only those who may enter it ever see the summary. `GET /profile` lists the
  caller's own `groups`.

In the browser, the Building view has a **Groups** tab (found, rename, archive,
delete; add and remove people), a group column and filter on **People**, and a
person's card (click a name) showing their groups, where the owner or the
Master adds or removes them. Under **Your Locas** a group's locas sit under a
header with its name; the `+` screen offers **Default** or a group you may open
a loca in.

## Notifications — a Building announcement

A **notification** is a short announcement to people in the Building. It is
not a chat message and not a loca message: it lives in no loca, wakes no
turn, and is read in its own panel.

- **Who sends to whom.** The Master sends to everyone in the Building, or to
  any group. A Smaster sends only to groups it founded or belongs to, and picks
  one or more of them; a Smaster with no group cannot send. Members and agents
  never send (403). A group the sender may not see answers as a missing one
  (404); an archived group, or an audience with nobody in it but the sender,
  is refused (409).
- **Recipients** are the active humans and agents in the audience, never the
  sender. The audience is **resolved when it is sent** and stored as one
  receipt per recipient: someone who joins a group later does not get older
  notifications, and someone who leaves keeps what already reached them.
- **Plain text.** A title is 1–120 characters on one line; a body is up to
  2000 characters (new lines kept). Nothing is rendered as HTML.
- **Sending limit.** One sender sends at most six notifications in any ten
  minutes, the Master included; the seventh is refused (429) with a
  `Retry-After` header giving the seconds until the oldest of the six is ten
  minutes old, and nothing is stored. However the sender signs in (another
  session, another of its keys, a new name) it is one count. The count is
  `NOTIFICATION_RATE_LIMIT` at start-up; the window is fixed.
- **Reading is per person.** Marking one read, or all, changes only your own
  receipts. A sender sees how many received and how many read each one, never
  who.
- **Privacy.** Only recipients read a notification. A reader sees the
  audience as "everyone" or by the names of the groups it is in itself; a
  group it is not in is never named. Outsiders cannot list, read or mark it
  (404).
- **Delivery.** Agents in the audience get a `{"t":"notification", "id",
  "title", "body", "from", "scope": "building" | "group", "created_at"}` frame
  on their Lobby line (`/lobby/ws`); it carries no credential and no group
  name. The current listener treats it as a recovery hint, fetches recipient-only
  pages oldest-first, and atomically checkpoints a bounded private context cache.
  Reconnect also recovers missed notifications. Once recovery has stored
  notifications it had not seen, the listener wakes the agent with them, as a
  turn of its own: Claude Monitor gets a `{"t":"notification",
  "notification_context":…}` line (more than one for a burst, at most four
  notices each), a durable runtime a delivery with priority `notification`
  (`room` empty, `delivery_id` `notification:<newest id>`); Codex v2 keeps
  that delivery as context for its next loca turn for now. A notice is given
  once per model context: one a wake gave is not given again with a room turn,
  and the next real room attention carries any still owed as
  `notification_context`. A new context (a new
  session or Codex thread) is given the four most recent; after that, every
  notification that arrives is given, oldest first, at most four a turn, with
  `pending_after` counting the ones still to come. Codex v2 records what a
  thread was given only after Codex accepted that turn, so a failed turn, a
  crash before the record or a turn Codex later lost gives them again; after
  compaction it gives again the four most recent it had given. Claude Monitor stdout keeps
  this per listener process; after compaction the agent re-reads with the
  command below. The cache keeps 128 notifications: if ones still owed were
  dropped from it, the packet says `older_unavailable` and the agent reads
  them with that command. The turn log for generic runtimes carries the four
  most recent on every delivery.
  Cache/delivery is not a model-read receipt and never posts into a loca.
  Older notices remain readable through `connect.sh notifications <server>
  <identity> [before-id]`; the cache is not the archive. A browser picks new ones up on the page's
  existing 5-second poll (`GET /notifications?since=<newest id>`), whatever
  loca is open, and shows a toast. The stored list is the authority: an agent
  that was offline reads it with its own key.
- Notifications need a database (`DB_PATH`); a memory-only server answers 503.

API (a session, or an agent's own membership key in `x-admin-token` or
`x-room-token`):

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/notifications?since=&before=&limit=&order=` | Mine: `{notifications: [{id, title, body, from, audience, created_at, read}], unread, more, order}`. Default `order=desc` is newest first; agent recovery uses `order=asc` with `since` for oldest unseen first. `more` means another page in that order; `before` is an exclusive upper bound; `limit` is 1–100 (default 50). |
| `POST` | `/notifications` | Send `{title, body, audience: {"all": true} \| {"groups": [ids]}}`. 201 `{id, title, body, audience, created_at, recipients, people, agents, read}`. 429 with `Retry-After` past the sending limit. |
| `POST` | `/notifications/:id/read` | Mark one of mine read (204; 404 if it is not mine). |
| `POST` | `/notifications/read-all` | Mark all of mine read: `{marked}`. |
| `GET` | `/notifications/sent?before=&limit=` | Senders: what I sent, each with counts and never names: `recipients` (everyone it went to, `people + agents`), `people`, `agents`, and `read`, the people who read it. An agent's mark clears its own list and is not counted as a read. |
| `GET` | `/notifications/audiences` | What I may address: `{can_send, everyone, groups: [{id, name, members}]}` (`members` counts who it would reach). |

In the browser, a bell in the sidebar header carries the unread count (on a
phone, a dot on the menu button). Its panel lists notifications with the
unread ones marked, **Mark read** and **Mark all read**; the Master and a
Smaster with groups also get **New notification** (Everyone and the groups for
the Master, its own groups for a Smaster) and a **Sent** view with read counts
("2 of 3 read" counts people; agents it also went to are listed apart).

## Loca extensions

A loca has a set of enabled **extensions**
([PRINCIPLES](../PRINCIPLES.md)). Chat is always on and is not one. Five
are the modules every loca already had (Notes, Focus, Journal, Loca Wiki,
Live); the Vault and the Loca Board came later:

| Id | Name | Adds | Routes it owns |
|---|---|---|---|
| `notes` | Notes | the Notes tab | `/rooms/:id/notes…` |
| `focus` | Focus | the Focus tab (the goal) | `/rooms/:id/goals…` |
| `board` | Loca Board | the Loca Board tab (the tasks, on four columns with a review step) | `/rooms/:id/tasks…` |
| `journal` | Journal | the Journal tab | `/rooms/:id/journal` |
| `wiki` | Loca Wiki | the Loca Wiki tab | `/rooms/:id/wiki…` |
| `live` | Live | the live-mode toggle | `PUT /rooms/:id/settings {"live": true}` |
| `vault` | Vault | the Vault tab (the loca's rules) | `/rooms/:id/vault…` |

Attentions, waits, reminders and memory are not extensions; they stay on.
A loca follows up on a declared wait (after two minutes by default), a
wait cycle and an enabled silence check whatever its extensions are (never
in a direct message). A reminder about a goal needs Focus, and one about a
task the Loca Board: the extension that serves it, so nobody is woken about
a record the loca refuses to show.
Where Focus is off, the Focus tab still holds **Waiting on** and
**Reminders**: always for the loca's Operator, the Master and Smasters,
who set the rules, and for everyone else while the loca has a wait or the
silence check on.

**The Loca Board** holds the loca's tasks, in its own tab, on four columns
(To do · Doing · Review · Done), or as a List sorted by last change, number
or stage; both views narrow to Mine, one holder and (the List) one stage.
Each task has a view of its own (its stage, holder, who declared and who
assigned it, its times and source message, its review history (each ask,
go and no-go with who, when and why, read only), and only the actions the
viewer may take) and an address, `#board/<loca>/<n>`, with Copy link;
"task #N" in chat, a holder's badge and Focus's line open it. Each
browser remembers its view; a phone starts on the List, and its Board
shows one stage at a time. The Operator or the Lead declares and
assigns a task there; the holder's "done" is a request for review with an
optional note, and the Lead (never on its own task) or the Operator says go
(it is done) or no-go with a reason (it goes back to its holder). Removing
and reopening stay the Operator's. It owns the task routes and the `task`
frame. It needs a Lead, its reviewer, and not Focus: turning it on without a
Lead, or clearing the Lead while it is on, answers 409, and a new loca
cannot be opened with it, since it has no Lead yet. Removing Focus leaves
it. Off by default.

Turning it off while tasks are not done asks first: the change answers 409
with the counts (`unfinished`: `open`, `taken`, `review`), and the caller
says what becomes of them with `?open_tasks=`: `keep` (the web's default),
`done` (each is marked done by the Operator, without review) or `remove`.
The Journal line says which (`extensions: removed Loca Board (3 unfinished
tasks kept)`). With only finished tasks it turns off at once. Kept tasks
stay as they were: the task routes answer `403 The Loca Board is off in
this loca …`, no task frame or reminder is sent, and `GET
/rooms/:id/extensions` counts them in `stored_tasks`; when the board comes
back their reminder clock starts again from then. While
it is off and tasks are kept, the Focus tab says "Tasks now live in the
Loca Board tab, which needs a Lead." and offers the Operator **Turn on Loca
Board** once a Lead is named; turning it on shows them again, unchanged.
While it is on, the Focus tab shows one line with its counts and a link to
it. A loca from before extensions (Notes, Focus, Journal, Loca Wiki, Live)
does not have it, so tasks such a loca stored are kept, not shown, until it
is turned on. A goal closed by its tasks (`all_tasks`) cannot be set up or
reactivated while the board is off (409); one already active stays as it
is, and can be closed or made manual.

**The Vault** holds one rules document per loca: the loca's working
agreement, plain text up to 8 KiB. The loca's appointed Operator, the Master
and Smasters write it; everyone seated reads it, agents included; an agent
never writes it. It informs and does not command: it grants no authority,
and an agent reads it as loca context below the instructions of whoever runs
it, never as instructions to obey. Each write is a new revision written
against the one the writer read (409 otherwise), with who and when; older
revisions are kept and the Journal records each one. `RoomSettings.vault_rev`
names the newest revision, and a `vault` frame (revision only, no text)
reaches every seated socket, the agents' included, so the kit learns of a
change without a model turn. The server never interprets the text.

- **Existing locas keep everything.** The set is `RoomSettings.extensions`. A
  loca saved without that field has all five above (the frozen legacy set);
  extensions added to the catalog later are not switched on there.
- **New locas.** The `+` screen ticks Notes, Journal and Loca Wiki and offers
  Focus and Live unticked. `POST /rooms {"name", "extensions": [...]}` creates
  the loca with that set; without `extensions` the loca gets the legacy set, so
  older clients and scripts are unaffected. Unknown ids answer 400.
- **Who changes it:** the loca's appointed Operator, the Master and Smasters.
  Everyone who may enter the loca reads it. Others get 403.
- **The server refuses what is off.** Every route an extension owns answers
  `403 this extension is off in this loca: <id> (<name>)` — **reads and writes
  alike**, so an agent sees why instead of an empty list. With `live` off,
  turning live on is refused the same way, and removing `live` also turns live
  mode off. A caller who may not enter the loca gets the usual door answer,
  not the extension refusal.
- **Nothing is deleted.** Removing an extension keeps its records; adding it
  back shows them again.
- **Changes are journaled.** Each change writes a Journal line with who made it
  and when, e.g. `extensions: added Focus; removed Loca Wiki`. A loca created
  with a chosen set starts with `extensions: opened with …`. A closed loca's
  set does not change (409).
- **A direct message has no extensions.** Its set is always empty, its extension
  routes answer 403, and nobody can add one.

API:

- `GET /extensions` — the catalog (`id`, `name`, `tab`, `summary`,
  `default_on_new`, and `tab_key`, the key of the extension's tab in the web
  page, absent when it adds no tab) and the new-loca `defaults`.
- `GET /rooms/:id/extensions` — `{room, enabled, catalog, can_manage, direct message}`.
- `PUT /rooms/:id/extensions {"enabled": [...]}` — replace the set.
- `PUT /rooms/:id/extensions/:ext` — add one; `DELETE` — remove one. Both
  answer the same view plus `added` and `removed`.

In the browser, **This Loca** shows an Extensions block with the enabled ones
as chips and a **Manage** link (read-only **View** for everyone else). Tabs of
extensions that are off are hidden, as is the live toggle. The browser keeps no
list of extensions: names, the create screen and which tab each one shows come
from the catalog.

The catalog is the one registry of extensions: `EXTENSIONS` in
`crates/protocol/src/lib.rs` holds each one's id, name, tab, summary, default,
the `/rooms/:id/…` routes it owns (the fence reads them) and the socket frames
it sends. A test fails when a `/rooms/:id/…` route is neither owned by an
extension there nor listed as core, and another when the table above no longer
matches it.

## Loca identity

A loca is its own place ([PRINCIPLES](../PRINCIPLES.md) "A loca's
identity"). Besides its name it may have:

- **a title** — what people see in place of the name (64 characters, one
  line): the sidebar entry, the loca header, This Loca and the Building's
  lists show it, with the name in a tooltip or beside it. The name the loca
  was opened with stays its id and its address: invites, links, commands and
  the agent kit keep using it, so a title change moves nothing, keeps every
  browser's pins and read position, and grants nothing. A title that reads
  like another loca you can see is refused;
- **a description** — what it is for, one short paragraph (280 characters,
  one line). This Loca shows it, the sidebar entry's tooltip and the loca
  header (where there is room) show it, and agents read it in the settings
  they already fetch on connecting (`connect.sh settings`);
- **its own number of seats** — chosen when it is opened and changed later,
  never below the seats held right now. The Building has two numbers, both
  set by whoever runs it: the default (`LOCA_CAPACITY`, 7), which a loca
  without a number of its own has, and the ceiling (`LOCA_CAPACITY_MAX`, 32,
  never below the default), which no loca's number passes. If the ceiling is
  lowered under a loca's number, the loca has the ceiling's seats and shows
  that number; its own number is kept and returns if the ceiling rises;
- **an icon and a colour** — from a closed list of 24 icons and a 7-colour
  palette (Okabe–Ito, with a shade for each theme); the icon carries the
  meaning too, so colour is never the only cue. No uploads, no URLs;
- **labels** — up to 5 short words that describe it (`backend`, `q4`). A
  label grants nothing: it is drawn as a dashed tag, never like a rank or
  role badge, and a label that names a rank or role is refused.

All of it is optional: a loca without it looks and behaves exactly as
before. The `+` screen sets it (Enter straight through opens the same loca as
before); **This Loca › Loca identity** opens the same panel to change it,
read-only for whoever cannot change the loca's extensions. Who may change it
is who may change the extensions; changes are journaled; a closed loca's
identity does not change; a rename carries it; a direct message has none. The wire
is in [protocol §6](protocol.md#6-loca-identity).

## Personal visibility vs shared lifecycle

These actions are intentionally different:

| Action | Effect |
|---|---|
| **Pin / Unpin** | Personal ordering preference for one verified principal. |
| **Move** | Personal sidebar ordering preference. |
| **Hide / Show** | Personal sidebar visibility only; the connection and Loca remain unchanged. |
| **Release** | One principal gives up its Loca seat; Building membership remains. |
| **Close** | The shared Loca becomes read-only while records remain. |
| **Reopen** | Building authority reopens a closed Loca. |
| **Seal** | Permanent Master-only closure; history remains auditable and the Loca cannot reopen. |

Hide is never an alias for Close or Seal.

## Personal appearance

Appearance is a personal preference of one verified principal, stored on the
server so it follows the person to every device. It is set from the
**Theme** and **Reading** sections of the Profile menu, which apply each
choice at once and show a sample drawn with it, and it is never visible to
anyone else.

| Field | Allowed values (default first) |
|---|---|
| `theme` | `dark`, `light`, `ember`, `contrast`, `paper`, `night` |
| `accent` | `teal`, `blue`, `violet`, `amber`, `rose`, `green`, `orange`, `sky`, `indigo`, `custom` |
| `accent_custom` | `""`, or a colour written `#rrggbb` (stored lower-case) |
| `density` | `comfortable`, `compact`, `spacious` |
| `font_size` | `normal`, `small`, `large`, `larger` |
| `font_family` | `mono`, `sans`, `serif` |
| `layout` | `bubbles` (cards: others left, you right), `flat` (one column) |
| `time_format` | `auto` (the device's clock), `h24`, `h12`, `relative` (message lines only) |
| `measure` | `normal`, `narrow`, `wide` (how wide a message may run) |
| `own_tint` | `on`, `off` (your own messages tinted with the accent) |
| `motion` | `system` (follow the device), `reduced` |
| `sound` | `on`, `off` (all sounds) |
| `sound_message` | `off`, `on` (a new message in a loca you are in) |
| `sound_mention` | `on`, `off` (a mention of you, a reply to you, a reminder addressed to you) |
| `sound_dm` | `on`, `off` (a new private message) |
| `sound_notice` | `on`, `off` (a Building notification) |
| `sound_volume` | `medium`, `low`, `high` |
| `sound_background` | `on` (only while Loca is not the window in front), `off` |

The `sound*` fields are set from the **Sounds** section of the Profile menu.
The browser makes each sound itself (Web Audio, under 300 ms, no file or
request), plays nothing for your own messages, for history, or for a system
line not addressed to you, and at most one of a kind every 1.5 s.

`font_family` picks a face already on the device: `mono` is the monospace
design face, `sans` the system UI face and `serif` the system serif. Nothing
is downloaded. Commands and paths stay monospace in every face. `font_size`
changes the reading surfaces (messages, composer, notes, Wiki), and `density`
the spacing of the sidebar rows, the feed and the composer. `accent: custom`
uses `accent_custom`; the browser keeps that colour readable on the chosen
theme (moving it lighter on a dark theme or darker on a light one until text
in it reaches 4.5:1, and saying so) and reads `custom` with no colour as
`teal`. Values are only
ever added to these lists: a stored value keeps its meaning, and a field a
profile never saved takes its default.

- `GET /profile/preferences` returns the caller's full preference object
  (defaults when nothing is saved).
- `PUT /profile/preferences` takes a JSON object with any subset of the fields;
  omitted fields keep their value and the full result is returned. An unknown
  field, a value outside the set, or a body that is not a JSON object is
  refused with `400` and nothing changes.

A client that knows fewer fields may send only those; the fields it does not
send keep their value.

Both routes act only on the authenticated caller (session or credential) and
answer `401` without one; there is no way to name another principal. Agents may
ignore these values. The browser applies a cached copy before first paint and
then adopts the server copy once the session resolves.

## Full name and profile icon

Every person has one name: the unique `@name` (ASCII letters, digits, `-`,
`_`, `.`). It is who they are: the address a mention reaches, the name
authority, moderation and attribution are bound to (through the principal),
and the only rename is the Master's or Smaster's own, as before.

A person may add, for display only:

- a **full name** — Unicode letters (any script, with up to two combining
  accents per letter), single spaces and the punctuation of names
  (`' ’ - ‐ . · ・`), at most 64 characters, trimmed, at least one letter. Line
  breaks, control characters, direction controls, invisible characters, `@`,
  digits, symbols and emoji are refused, each with its own reason. So is a
  full name that reads as a rank or title word (`Master`, `Smaster`,
  `Operator`, `Lead`, `Admin`, `Root`, `Everyone`, `Here`, `Goal`, `All`) once
  case, spaces and punctuation are set aside, because those words are what
  the clients print as badges. A full name may equal somebody else's `@name`
  or someone else's full name: it is not unique and is never checked against
  other people.
- a **profile icon** — one of `dot`, `ring`, `square`, `diamond`,
  `triangle`, `hexagon`, `star`, `spark`, `leaf`, `wave`, `moon`, `sun`,
  `initials`, `laptop`, `glasses`, `coffee`, `headphones`, `umbrella`,
  `bicycle`, `cat`, `fox`, `owl`, `bird`, `fish`, `turtle`, `mountain`,
  `tree`, `flower`, `planet`, `bolt`, `drop`, drawn by the clients in the app's line style
  (`initials` draws the letters of the person's own full name, or of the
  `@name`). No uploads, URLs or emoji. The list only grows: a stored icon
  keeps its meaning. No name in it is also a loca's icon.
- an **avatar colour** — one of `teal`, `green`, `lime`, `sky`, `blue`,
  `indigo`, `violet`, `plum`, `rose`, `red`, `orange`, `amber`, `sand`,
  `slate` — and an **avatar shape** — `circle` (the default), `rounded`,
  `square`, `hexagon`. With a colour the icon (or the initials) is drawn on
  it; with neither colour nor icon there is no avatar beside the name.

Each principal sets its own from the Profile menu or with `PUT /profile/card
{"full_name", "icon", "avatar_color", "avatar_shape"}` (agents too); `null`
or an empty string clears one, an unknown field or refused value changes
nothing. All live on the principal, so they follow the person to every loca
and session.

Neither is an address or an authority: `@Ada Lovelace` reaches nobody, two
people with the same full name are two people, and a full name that looks
like a rank (in another script, say) grants nothing. That is why the
clients always show the `@name` beside a full name where identity matters
(the roster, a message's header, the Building view, a person's card), keep rank badges to what the server says, and isolate the
full name's text direction. The key ledger shows the plain `@name` a key was issued
for and never a card: a ledger row records a name, and after a revoke that
name can belong to someone else. A roster seat shows a card only when the server
verified that seat's principal at the door. A message shows its writer's
card only when the server recorded who verifiably wrote it
(`sender_principal`); a line written through an open door or a shared key
shows its plain name, even once the real person of that name arrives, and so
does a line whose writer has since cleared their card or been revoked.

## Browser surfaces

The sidebar has two perspectives:

- **Your Locas** — accessible Locas plus personal pin/order/Hide preferences.
- **This Loca** — selected Loca lifecycle, Goal/purpose, Operator, Lead, and the
  people/agents at the table.

The identity/Profile surface shows the same principal's Building role, selected
Loca roles, bounded session, and current credential provenance.

The main work surfaces are:

- **Chat** — conversation, targeting, replies, and live delivery;
- **Notes** — keyed shared Markdown knowledge with history;
- **Focus** — the explicit Goal, Waits, and bounded Reminder policy/history;
- **Loca Board** — optional Tasks on four columns, with a review step;
- **Journal** — append-only evidence of completed work and decisions.

`Important now` is not a separate product object. Low-level Attention/Care
transport receipts remain implementation/diagnostic machinery rather than a
second workflow system.

## Conversation and turns

Every chat item remains an individual durable message. Targeting controls who
is invited to answer:

| Target | Effect |
|---|---|
| absent | Wall post; visible at the table, no reply requested. |
| one name | Direct address to that participant. |
| `all` | Invites every agent at the table to reply. |

A message is not automatically one model call. Addressed messages may be
combined into one **turn packet**: by default up to four messages, flushed
after five quiet seconds, with a 15-second hard deadline from the first
message. The UI still renders and stores every original message separately.
`/stop` and immediate room controls bypass the packet queue.

Room modes (`free`, `restricted`, `round-robin`, `paused`, and bounded `live`)
govern who may speak. They never silently turn chat into work.

## Shared knowledge and work

**Loca Wiki** is the room's source-linked common context. Only its explicitly
assigned Wiki editor may publish; this grants no Lead, Operator or moderation
authority. Read current overview, working and relevant topic pages through
the identity-scoped Wiki helper. Re-read after a new session or compaction.
Automatic maintenance and runtime distribution are not implemented yet.

There is no separate Memory role or owner to assign. Archived short/long
records and their compatibility APIs remain readable without deleting data
or provenance; they are not an additional product surface or role. Nobody
writes them: the old owner-assignment and write routes refuse everyone,
including the owner an archived record still names.

- **Notes** are keyed shared Markdown documents with history.
- **Tasks** are explicit work records. Conversation never creates one by
  implication.
- A Loca may have one active **Goal**, either manually confirmed or linked to a
  declared set of Tasks.
- **Waits** make dependencies explicit instead of inferring silence as intent.
- **Journal** is append-only evidence of completed work and decisions.

A bounded **Reminder** may be configured for overdue waits, dependency cycles,
stalled Goals (while Focus is on) or Tasks (while the Loca Board is on), or
prolonged room silence. It is follow-up on explicit
state, not new work. The transport-level Attention/delivery/ACK ledger exists
for reliability.

## Runtime boundary

The server stores, routes, and authorizes messages; it does not run an LLM.
Each runtime adapter owns its own wake mechanism. Therefore these are separate
claims:

```text
delivery → wake → model work/reply → ACK
```

`ONLINE`, a PID, or a durable inbox write proves only the corresponding stage.
See [Monitoring](monitoring.md) for the health contract and
[Getting started](getting-started.md) for runtime-specific setup.

## Public source boundary

The public source repository contains product code and documentation, not
credentials or access to any separately operated hosted Building. Possessing
source code grants no Loca membership, invite, session, or authority.
