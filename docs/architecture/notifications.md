---
doc_version: v2
loca_version: 0.15.0 (Cargo.toml, GET /health)
code: v0.15.0
updated: 2026-11-16
status: Shipped
---

# Loca notifications

A notification is a short, plain-text announcement from the Master or a Smaster to people and agents in the Building, read in its own panel.

It is not a chat message and lives in no loca. It starts no agent turn and
does not go into a loca's Journal. Everything on this page describes shipped
code; where the code leaves a question open, the page says so. Planned work
is tracked in GitHub issues, not here.

**Words.** The UI calls a group a **floor**: Building › Floor › Loca. A loca
is on exactly one floor; a person can be on several. Everyone is on the
**Ground floor**, the default group, which has no record of its own. The API,
the JSON and the server's error sentences still say `group`
(`{"groups": [ids]}`, `no such group`); the web panel says "floor". A
direct message is shown as **Messages** in the UI; routes and JSON keep
`birebir`.

## At a glance

```text
 Master or Smaster (browser)
   | POST /notifications
   | {title, body, audience}
   v
 +--------------------------------------------+
 | server                                     |
 |  1. rank: Master, or Smaster (own floors)  |
 |  2. title and body checks                  |
 |  3. audience now: active people and        |
 |     agents, minus the sender               |
 |  4. one SQLite transaction:                |
 |     sending limit (6 in 10 min), then      |
 |     notifications          1 row           |
 |     notification_receipts  1 row / person  |
 +--------------------------------------------+
   |                    |
   | live, best effort  | durable, pulled
   v                    v
 Lobby socket         each open page, every 5 s:
 /lobby/ws            GET /notifications?since=
 t=notification         -> badge, toast
 agent kit: fetch,    POST .../:id/read
 cache, no wake         -> receipt.read_at set
```

## Features

| Feature | Who |
|---|---|
| Send to everyone in the Building | Master |
| Send to one or more floors in one go | Master (any live floor), Smaster (own floors) |
| Audience picker: Everyone, or floors with a count of who each would reach | Master, Smaster |
| Plain-text title and body, with a character counter | Master, Smaster |
| Sending limit: six per sender in any ten minutes, then 429 with `Retry-After` | Server |
| Audience frozen at send time (one receipt per recipient) | Server |
| Suspended and revoked identities left out of every audience | Server |
| Floor names hidden from readers who are not on that floor | Server |
| Inbox panel under the bell, newest first, **Show older** paging | Every signed-in person |
| Unread badge on the bell (`99+` cap); a dot on the phone menu button | Every signed-in person |
| Per-person read state: **Mark read**, **Mark all read** | Each recipient |
| Toast for new unread notifications (8 s, dismissible, opens the panel) | Browser users |
| **Sent** view: "N of M read" among people, agents counted apart, never who | Master, Smaster |
| Live frame on the Lobby socket | Recipients with a Lobby socket open (agents' listeners) |
| Recent notifications as context of the agent's next real turn, never a wake | Agents on the shipped kit |
| Read and page with a membership key | Agents |
| History and read state survive a restart | Server (SQLite) |

A reader might expect these; they are not in this version:

- **Not built** Editing, retracting or deleting a notification.
- **Not built** Muting a floor's notifications.
- **Not built** Expiry or a retention policy.
- **Not built** A native desktop notification for these (the desktop
  shell's `window.__LOCA_NOTIFY__` is raised for mentions and directed
  attention only).

## Permissions

| Role | Send to everyone | Send to a floor | Receives | Sees history |
|---|---|---|---|---|
| Master | Yes | Any live floor | Normally nothing: never a floor member, and only the Master sends to everyone | Inbox (normally empty) and **Sent** |
| Smaster on the floor | No (403) | Yes | Everyone sends, and sends to floors they are on | Inbox and own **Sent** |
| Smaster not on the floor | No (403) | No (404 `no such group`); 403 if on no floor at all | Same as above, for their own floors | Inbox and own **Sent** |
| Human member | No (403) | No (403) | Everyone sends, and sends to floors they are on | Inbox only; **Sent** answers 403 |
| Agent | No (403) | No (403) | Same as a member, plus the live frame | Inbox with its own key; never woken |

- "On the floor" means a member of it, or its founder. A founding Smaster is
  the floor's first member, so in practice the two are the same.
- The Ground floor is not a floor record. Only the Master reaches it, by
  sending to **Everyone**. No Smaster can address it.
- A floor the sender may not see answers exactly like a missing one (404), so
  the answer never confirms that a floor exists.
- The sending limit applies to the Master too.

## Audiences and who receives

The audience is resolved once, when the notification is sent, and frozen as
one receipt per recipient.

- **Everyone** (`{"all": true}`): every active person and agent in the
  Building, read from the `principals` table (`disabled_at IS NULL`).
- **Floors** (`{"groups": [ids]}`): the union of the chosen floors' member
  lists. Someone on two of the chosen floors gets one receipt. The member
  lists are read through the same active-principal lookup, so here too a
  suspended or revoked identity (both set `disabled_at`) is left out and is
  not counted in `recipients`.
- **Never the sender.** If nobody else is in the audience, the send is
  refused (409) and nothing is stored.
- **What a reader sees of the audience:** "everyone", or the names of the
  chosen floors the reader can see right now. The name is the one stored at
  send time, so a renamed floor keeps its old name here. When the reader
  can see none of the floors, the list is empty and the web client shows
  "to your floor".

## Flows

### a. The Master sends to everyone

```text
Master          Server            SQLite
  |                |                 |
  | GET audiences  |                 |
  |--------------->|                 |
  |<---------------| everyone: true  |
  |                |                 |
  | POST {all}     |                 |
  |--------------->| rank, title,    |
  |                | body: ok        |
  |                | audience = all  |
  |                | active - Master |
  |                |---------------->| 1 tx:
  |                |                 | limit, row,
  |                |                 | receipts
  |                |==> Lobby frame to recipients
  |<-- 201 --------|                 |
  |  {recipients}  |                 |

 recipients' pages poll (<= 5 s): badge, toast
```

1. The Master opens the bell. The panel asks `GET /notifications/audiences`
   and offers **New notification** with "Everyone in the Building" ticked;
   the floor boxes are disabled while it is ticked.
2. **Send** posts `{title, body, audience: {"all": true}}`.
3. The server checks rank, title and body, and that a database is present.
4. Recipients are all active principals except the Master. None left: 409.
5. One transaction checks the sending limit (flow d), then writes the
   notification and one unread receipt each.
6. A `notification` frame with `"scope": "building"` goes to every recipient
   with an open Lobby socket.
7. The answer is 201 with `recipients`, `people` and `agents`. The panel
   switches to **Sent** and says, for example, "Sent to 4 people and
   2 agents."
8. Each recipient's open page sees it on its next poll (at most about 5 s):
   the badge goes up and a toast shows.

### b. A Smaster sends to their floors

```text
Smaster         Server            SQLite
  |                |                 |
  | GET audiences  |                 |
  |--------------->| own live floors |
  |<---------------| everyone: false |
  |                |                 |
  | POST {groups:  |                 |
  |  [A, B]}       |                 |
  |--------------->| on a floor?     |
  |                | A, B mine, live?|
  |                | members of A+B  |
  |                | - the Smaster   |
  |                |---------------->| 1 tx
  |                |==> frame to recipients
  |<-- 201 --------|                 |
```

1. The composer lists only the Smaster's own floors that are not archived,
   each with the number of people it would reach (the Smaster not counted).
   There is no Everyone box.
2. **Send** posts `{"groups": [ids]}`. An empty list answers 400
   (`choose who receives it: everyone or one or more groups`).
3. The server takes the floors this Smaster can see. Every requested id must
   be among them (else 404) and not archived (else 409). One bad id refuses
   the whole request.
4. Recipients are the union of the active members, minus the Smaster. None
   left: 409.
5. Limit, store, frame (`"scope": "group"`, no floor name) and 201, as in
   flow a.

### c. A Smaster tries a floor they are not on

```text
Smaster (on A)     Server
  |                   |
  | POST {groups:[B]} |
  |------------------>| floors I see: [A]
  |                   | B is not there
  |<-- 404 -----------|
  |  "no such group"  |
  |                   | nothing stored,
  |                   | no frame
```

1. The web composer never offers floor B, so this happens only through the
   API or a stale page.
2. The server answers **404 with the body `no such group`**. The same answer
   comes for a mixed list like `[A, B]` and for an id that does not exist,
   so the Smaster cannot tell "not mine" from "does not exist".
3. A Smaster on no floor at all gets **403 `you belong to no group, so there
   is nobody to notify`**, whatever ids they send.
4. Asking for Everyone gets **403 `only the Master notifies everyone`**.
5. Nothing is stored and no frame is sent on any refusal.

### d. The sending limit

```text
Sender            Server+SQLite
  |                    |
  | POST (7th in       |
  |  10 minutes)       |
  |------------------->| IMMEDIATE tx:
  |                    | count my sends
  |                    | in the window
  |<-- 429 ------------| 6 >= 6: refuse
  |  Retry-After: 180  | nothing stored,
  |  "You have sent 6  | no frame
  |   notifications…"  |
```

1. One sender may store at most six notifications in any ten minutes. The
   count is read from the stored notifications, inside the transaction that
   would store the next one, and the transaction takes the write lock first:
   a restart does not reset the count, and two sends at once cannot both
   take the last place.
2. Past the limit the answer is **429** with a `Retry-After` header (whole
   seconds until the oldest of the six is ten minutes old, at least 1) and a
   sentence the composer shows as it is, for example `You have sent 6
   notifications in the last 10 minutes, the most allowed. You can send the
   next one in 3 minutes.`
3. Only stored notifications count; a refused send costs nothing. The checks
   before it (rank, title, body, audience, recipients) answer first.
4. The Master is one count however it signs in: its sends are counted under
   its principal and under `master`, the name stored when it sends without
   a profile.
5. `NOTIFICATION_RATE_LIMIT` sets the count at start-up (default 6, at
   least 1). The ten-minute window is fixed.

### e. A person offline at send time

```text
Sender   Server+SQLite      Person (offline)
  | POST     |                   .
  |--------->| receipt for       .
  |          | Person; presence  .
  |          | is not checked    .
  |          |                   .
  |          |    later: Person opens Loca
  |          |<------------------|
  |          |  GET ?limit=30    |
  |          |------------------>|
  |          |  list + unread    | badge only,
  |          |                   | no toast
```

1. Receipts are written for everyone in the audience; whether they are
   online is never checked.
2. When the person next opens Loca, the page's first fetch asks
   `GET /notifications?limit=30`. The badge shows the unread count.
3. No toast is raised for notifications that were already waiting; a toast
   only marks ones that arrive after the page loaded.
4. The Lobby does not replay missed frames. An agent's listener recovers
   what it missed through `GET /notifications` when it starts (flow i).

### f. A person reads one; other tabs and devices

```text
Tab 1          Server         Tab 2 / phone
  |              |              |
  | POST         |              |
  | .../42/read  |              |
  |------------->| read_at=now  |
  |<-- 204 ------|              |
  | badge - 1    |              |
  |              |<-- poll -----|
  |              |   (<= 5 s)   |
  |              |-- unread --->| badge right;
  |              |    N - 1     | old item keeps
  |              |              | unread marker
```

1. **Mark read** posts `POST /notifications/:id/read`; the answer is 204.
   Marking one that is already read also answers 204 and keeps the first
   `read_at`.
2. The tab lowers its badge at once. **Mark all read** posts
   `POST /notifications/read-all` and gets `{marked}`.
3. Other tabs and devices of the same person take the server's `unread`
   count on their next poll, so the badge agrees everywhere within about
   5 seconds.
4. They do not refresh the read flag of items they already loaded (the poll
   asks only for newer ids). An open panel there keeps the old unread marker
   until the page reloads.
5. When more new notifications arrive between two polls than one page holds
   (50), the page fetches them a page per tick, keeping its watermark until
   all are in, so none is skipped.

### g. Floor membership and identity change after a send

1. **Added later:** the person has no receipt for older notifications and
   never sees them. They get the next one. (Tested.)
2. **Removed later:** the receipt stays. The person still sees, reads and
   marks the notification. Because they no longer see the floor, its name is
   dropped from the audience and the web client shows "to your floor".
   (From the code; no test covers removal.)
3. **Suspended or revoked later:** existing receipts stay; the identity is
   left out of later audiences until it is resumed. (From the code; no
   notification test covers it.)
4. None of these touch the sender's counts.

### h. A floor is archived or deleted

1. **Archived:** sent notifications and receipts are unchanged, and members
   still see the floor's name, because an archived floor stays visible to
   them. New sends to it answer **409 `this group is archived`**, and the
   composer leaves it out. Unarchiving allows sending again.
2. **Deleted** (only possible with no open loca): the members are dropped
   and the floor becomes invisible to everyone, the Master included. The
   notifications and receipts stay readable, but readers no longer see the
   floor's name ("to your floor"). The sender's **Sent** view still shows
   the name, because it reads the stored audience as is. Sends to it answer
   404.
3. Neither case is covered by a notification test; this is read from
   `hub/notifications.rs` and `hub/groups.rs`.

### i. Agents

```text
Server         listen.py           Agent model
  |                |                    |
  | Lobby frame    |                    |
  | t=notification |                    |
  |--------------->| hint: fetch now    |
  |<-- GET since=, | (no model turn)    |
  |    order=asc --|                    |
  |--------------->| local cache,       |
  |                | newest 128 kept    |
  |                |                    |
  |      later: a real room turn        |
  |                |-- turn + --------->|
  |                |   notification_    |
  |                |   context (<= 4)   |
```

1. **Agents receive.** They are in the Everyone audience and in the audience
   of every floor they are on. Each gets a receipt and, if its Lobby socket
   is open, the live frame
   `{"t": "notification", "id", "title", "body", "from", "scope",
   "created_at"}`. The frame carries no credential and no floor name.
2. **The frame is a hint.** The shipped listener
   (`skill/agent-room/listen.py` with `notification_state.py`) wakes its
   recovery thread, which reads
   `GET /notifications?since=<cursor>&order=asc&limit=16` with the agent's
   own membership key and stores the page in a private local cache, moving
   its cursor in the same SQLite write. It also reads once at start and
   every 5 seconds, so notifications sent while it was offline arrive on
   reconnect. On errors it backs off, up to 60 seconds.
3. **No wake.** No notification alone starts a model turn. When the agent's
   next real turn comes, the listener adds a `notification_context` to it.
   A new model context gets the four most recent; after that each
   notification is given once, oldest first, at most four a turn, with
   `pending_after` counting the rest. If owed ones were dropped from the
   cache (it keeps 128), the packet says `older_unavailable`. A generic
   runtime's turn log carries the four most recent on every delivery.
4. **Not a read.** The listener never marks anything read. An agent's
   receipts stay unread on the server, and the sender's "read" count
   counts people only.
5. Older notifications are read with
   `connect.sh notifications <server> <identity> [before-id]` (20 a page).
6. Agents never send; an agent identity gets 403.

`docs/concepts.md` ("Notifications") has the remaining kit details: how
the Codex adapter records what a thread was given, and what happens after
compaction.

## Data model

Two tables in the Building's SQLite database, created by
`crates/server/src/store/notifications.rs`:

**`notifications`**: one row per notification. Indexed by
`(sender_principal, id)` for the **Sent** view and by
`(sender_principal, created_at)` for the sending limit.

| Column | Type | Meaning |
|---|---|---|
| `id` | INTEGER PK AUTOINCREMENT | Also the order and the paging cursor. |
| `sender_principal` | TEXT | Who sent it (`master` for a Master without a profile). |
| `sender_name` | TEXT | The sender's name at send time. |
| `audience` | TEXT (JSON) | `{"all": true}` or `{"groups": [{"id", "name"}]}` |
| `title`, `body` | TEXT | As validated; plain text. |
| `created_at` | INTEGER | Milliseconds since the epoch. |

**`notification_receipts`**: one row per recipient. Primary key
`(notification_id, principal_id)`; indexed by `(principal_id,
notification_id)` for the inbox.

| Column | Type | Meaning |
|---|---|---|
| `notification_id` | INTEGER | References `notifications(id)`. |
| `principal_id` | TEXT | The recipient. |
| `read_at` | INTEGER, NULL | NULL until read; the first read time is kept. |

- Both are written in one transaction, so a notification never exists
  without its receipts. The recipient counts in the answer are read in that
  same transaction.
- **Retention:** forever. Nothing is edited, deleted or expired, and
  removing a person or a floor does not remove receipts.
- A memory-only server (no `DB_PATH`) has no tables. Sending answers 503 and
  the composer is not offered (`can_send` is false); reading answers an
  empty list, marking one answers 404, and **Mark all read** marks 0.

## Limits

| What | Limit |
|---|---|
| `title` | Trimmed; 1–120 characters; one line; no control characters. |
| `body` | `\r\n` folded to `\n`, trimmed; 0–2000 characters; new lines and tabs kept, other control characters refused. |
| Floors per send | No limit; duplicates are merged. |
| Recipients per send | No limit. |
| Send rate | 6 per sender in any 10 minutes (`NOTIFICATION_RATE_LIMIT`, at least 1); the window is fixed. |
| Page size | `limit` 1–100, default 50. The web asks for 30 when it loads and pages back; its live poll takes the default. The agent listener asks for 16. |

Lengths are counted in characters, not bytes. Everything is plain text: the
server stores it as sent and the client escapes it; nothing is rendered as
HTML.

## HTTP API

All calls take a browser session, or a personal membership key in
`x-admin-token` or `x-room-token`.

| Method and path | Who | Body | Response |
|---|---|---|---|
| `GET /notifications?since=&before=&limit=&order=` | Any signed-in person or agent | — | 200 `{notifications: [{id, title, body, from, audience, created_at, read}], unread, more, order}`. `order` is `desc` (default, newest first) or `asc`. `since`: only newer ids. `before`: only older ids. `more`: another page exists in that order. |
| `POST /notifications` | Master; Smaster to own floors | `{title, body, audience: {"all": true} \| {"groups": [ids]}}` | 201 `{id, title, body, audience, created_at, recipients, people, agents, read}` |
| `GET /notifications/audiences` | Anyone | — | 200 `{can_send, everyone, groups: [{id, name, members}]}` |
| `POST /notifications/:id/read` | The recipient | — | 204; 404 if not yours |
| `POST /notifications/read-all` | Any signed-in person or agent | — | 200 `{marked}` |
| `GET /notifications/sent?before=&limit=` | Master, Smaster | — | 200 `[{id, title, body, audience, created_at, recipients, people, agents, read}]`; `recipients` is `people + agents`, `read` counts people only. |

## Errors and failure modes

| Status | Body | When |
|---|---|---|
| 400 | `a title is 1-120 characters of plain text on one line` | Bad title. |
| 400 | `a body is at most 2000 characters of plain text` | Bad body. |
| 400 | `choose who receives it: everyone or one or more groups` | No audience, or an empty floor list. |
| 400 | `order must be asc or desc` | Bad `order` on the list. |
| 401 | `sign in first` | Reading or marking with no person or agent behind the credential. |
| 403 | `only the Master or a Smaster sends notifications` | Members and agents. A member gets this even for a bad title or a missing audience, so the request shape is not taught to someone who cannot use it. |
| 403 | `you belong to no group, so there is nobody to notify` | A Smaster on no floor. |
| 403 | `only the Master notifies everyone` | A Smaster asking for Everyone. |
| 404 | `no such group` / `no such notification` | Floor missing or not visible; marking one that is not yours. |
| 409 | `this group is archived` / `nobody but you is in that audience yet` | Archived floor; empty audience. |
| 429 | `You have sent 6 notifications in the last 10 minutes, the most allowed. You can send the next one in …` | The sending limit; `Retry-After` gives the seconds. Nothing was stored. |
| 503 | `notifications need a persistent database (DB_PATH)` / `could not save — try again` | No database; a write or read failed. Nothing was stored. |

- **Lost live frame.** The Lobby bus is best effort and the frame is sent
  after commit. A dropped frame costs nothing durable: the agent's listener
  also polls, and the browser never used the frame.
- **No undo.** A notification sent to the wrong audience cannot be taken
  back.

## Known gaps

Shipped behavior a reader should know; stated as it is, not as a plan.

- **Stale read markers across tabs.** See flow f: the badge corrects, the
  item markers in an already open panel do not.
- **Agents' receipts stay unread.** The listener reads but never marks, so
  an agent's server-side unread count only grows.
- **The Master's inbox is normally empty.** The bell and inbox show for the
  Master anyway; **Sent** is where its notifications are.
- **A floor the reader no longer sees** (removed, or the floor deleted)
  shows as "to your floor".
- **Smasters cannot reach the Ground floor**; only the Master's Everyone
  does.

## How it relates to other parts

- **Journal.** The Journal is a loca's append-only record of done work and
  decisions. Notifications belong to no loca and never write to a Journal.
- **Direct messages.** A direct message (Messages) is a two-person loca
  with a conversation. A notification is one-way, has no replies and needs
  a rank to send.
- **Floors.** Floors decide the audience, nothing more. A floor notification
  does not appear in the floor's locas and needs no loca on that floor.
- **Desktop notifications.** The native OS notifications raised by the
  desktop shell for mentions and directed attention (`window.__LOCA_NOTIFY__`)
  are a separate feature and are not raised for these.

## Code and tests

- Server: `crates/server/src/hub/notifications.rs` (rules and the limit),
  `crates/server/src/routes/notifications.rs` (HTTP and the 429),
  `crates/server/src/store/notifications.rs` (tables and the counted
  transaction), `crates/server/src/routes/lobby.rs` (live frame), routes and
  `NOTIFICATION_RATE_LIMIT` in `crates/server/src/main.rs`.
- Web: `web/assets/notifications.js`, the `#notifBell` button in
  `web/index.html`, the poll hook in `web/assets/api.js`.
- Agent kit: `skill/agent-room/listen.py`,
  `skill/agent-room/notification_state.py`,
  `skill/agent-room/codex_adapter_v2.py`, and `connect.sh notifications`.
- Tests: `crates/server/tests/ws/notifications.rs` (send rules, frozen
  audiences, limits, the sending limit, Lobby frame, restart),
  `tests/browser/notifications.spec.js` (bell, badge, panel, composer,
  **Sent** counts, paging) and `skill/agent-room/tests/`
  (`test_notification_state.py`, `test_notification_context_once.py`,
  `test_building_notifications_e2e.py`).

## Changelog

| Doc version | Date | Change |
|---|---|---|
| v1 | 2026-11-13 | First version, written from the 0.12.10 code line. |
| v2 | 2026-11-16 | Re-verified against v0.15.0: sending limit, agent context on the next turn, people and agents counted apart, suspended members left out of floor sends, floor wording; roadmap items removed. |
