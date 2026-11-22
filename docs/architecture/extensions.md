---
doc_version: v2
loca_version: 0.15.0 (Cargo.toml, GET /health)
code: v0.15.0
updated: 2026-11-16
status: Shipped
---

# Loca extensions

Extensions let each loca pick, from one built-in list, the capabilities it needs, so a loca can become the place for one job or project.

Chat is always on and is not an extension. This page describes what ships
in v0.15.0: seven extensions, the rules they follow, and how the server
enforces them. Where `PRINCIPLES.md` names something the code does not have,
the page says **Not built**; planned work is tracked in GitHub issues.

**Words.** The UI calls a group a **floor**: Building › Floor › Loca. The API
and the code still say `group`. Extensions are per loca and do not depend on
the floor. A direct message is shown as **Messages** in the UI; routes, JSON
and some server sentences keep `birebir`. **Care** is the ranks' own loca.

## How a loca gets its extensions

```text
 + New loca: "Open as" Loca or Lounge
   |  Loca ticks the defaults, Lounge none;
   |  add or untick more
   v
 each extension that is on = a tab
 (Live is a toggle, not a tab)
   |
   v
 This Loca > Extensions > Manage (changes)
```

The `+` screen has two presets. **Loca** ticks the catalog's defaults (Notes,
Journal, Loca Wiki); **Lounge** ticks none (chat only). Focus, Live and the
Vault are offered unticked. The Loca Board is not offered, because a new
loca has no Lead yet; the server refuses it on `POST /rooms` (409). The web
client sends the ticked list with `POST /rooms {"name", "extensions": [...]}`.

A loca saved before extensions existed has no `extensions` field and keeps
the five it already had (the frozen `LEGACY_EXTENSIONS`: Notes, Focus,
Journal, Loca Wiki, Live). `POST /rooms` without `extensions` gets the same
legacy set, so old scripts are unaffected. Extensions added to the catalog
later (the Vault, the Loca Board) are not switched on in such a loca, so
the tasks such a loca stored are kept but not shown until its Loca Board is
turned on.

## Rules every extension follows

| Rule | What it means |
|---|---|
| No outside code | An extension switches on a capability written and tested inside the server. Nothing is loaded or run from outside. |
| Off means off | A switched-off extension is refused by the server (403), reads and writes alike, not just hidden. Its records stay and come back when it is switched on again. |
| Who changes it | The loca's appointed Operator (always a person), the Master and Smasters. Everyone who may enter the loca can read the set. |
| Changes are recorded | Every change writes a Journal line with who made it, e.g. `extensions: added Focus; removed Loca Wiki`. |
| A direct message has none | A private two-person loca is conversation only. Its set is always empty and nobody can add one. |
| Core rules win | No extension lets an agent ask for or write a secret, open a queue or assign work. The one exception to "agents do not assign work" is the Lead's bounded task authority, which is a core rule and not an extension. |

## How it is built

The catalog is one constant, `EXTENSIONS` in `crates/protocol/src/lib.rs`.
Each `ExtensionSpec` has an `id` (stored and used in the API), a `name`, an
optional `tab`, a one-line `summary`, `default_on_new`, an optional
`tab_key` (the web tab it shows), and two lists that are not sent: the
`/rooms/:id/<segment>` routes it owns and the loca-socket frames only it
sends. A new extension is a new entry; nothing else in the list changes.

A loca's set lives in `RoomSettings.extensions`, normalized to catalog order
without duplicates. Unknown ids, including `chat`, answer 400. The logic is
in `crates/server/src/hub/extensions.rs`.

The `extension_fence` middleware (wired in `main.rs`, written in
`crates/server/src/routes/extensions.rs`) reads the first path segment under
`/rooms/:id/` and asks the catalog which extension owns it: `notes`,
`goals` (Focus), `tasks` (Loca Board), `journal`, `wiki`, `vault`. If that
extension is off it answers:

```text
403 this extension is off in this loca:
<id> (<name>). Its records are kept; the
loca's operator, the Master or a Smaster
can turn it back on.
```

A caller who may not enter the loca passes through to the normal door
answer, so the fence never tells outsiders what a loca has switched on.

Details that follow from the rules:

- Live has no routes. `PUT /rooms/:id/settings {"live": true}` is refused
  with the same 403 while Live is off, and removing Live also ends live mode.
- The Loca Board's refusal says so first: `The Loca Board is off in this
  loca — this extension is off in this loca: board (Loca Board). …`.
- Loca search returns no notes while Notes is off.
- A closed loca's set cannot change (409 `this loca is closed — read-only`).
- The Journal line is written even when Journal itself is off. It is pushed
  to open sockets only while Journal is on, and shows once Journal is
  switched back on.
- A loca opened with a chosen set starts with a Journal line such as
  `extensions: opened with Notes, Journal, Loca Wiki`, or `opened with chat
  only`.
- Care has the normal set; only a direct message is excluded.
- Tests hold every `/rooms/:id/...` route of the router to being owned by
  an extension or listed as core, and the table in `docs/concepts.md` to
  matching the catalog.

The web client (`web/assets/extensions.js`) keeps no list of its own: names,
the create screen and which tab each extension shows come from the catalog.
It shows tabs and the live toggle only for what is on, never fetches data for
an extension that is off, and offers **Manage** (or **View** for those who
cannot change it) under This Loca › Extensions.

### API

| Method and path | Who | Purpose |
|---|---|---|
| `GET /extensions` | Anyone | `{catalog, defaults}`: the catalog and the new-loca defaults. |
| `GET /rooms/:id/extensions` | Anyone who may enter | `{room, enabled, catalog, can_manage, birebir, stored_tasks}`; `birebir` is true for a direct message; `stored_tasks` counts the loca's tasks, kept while the Loca Board is off. |
| `PUT /rooms/:id/extensions` | Operator, Master, Smaster | Replace the set: `{"enabled": [ids]}`. Answers the view plus `added` and `removed`. |
| `PUT /rooms/:id/extensions/:ext` | Operator, Master, Smaster | Add one. Same answer. |
| `DELETE /rooms/:id/extensions/:ext` | Operator, Master, Smaster | Remove one. Its records stay. Same answer. |

| Status | Body | When |
|---|---|---|
| 400 | `unknown extension: <id>` | An id not in the catalog. |
| 403 | `only this loca's operator, the Master or a Smaster changes its extensions` | Anyone else. |
| 403 | `a birebir has no extensions` | A direct message. |
| 409 | `this loca is closed — read-only` | A closed or sealed loca. |
| 409 | `the Loca Board needs a Lead: select a Lead first` | The Loca Board turned on with no Lead. |
| 409 | `{error, unfinished, choices}` | The Loca Board turned off while tasks are not done, with no `?open_tasks=`. |
| 400 | `open_tasks is keep, done or remove` | Any other `open_tasks`. |
| 503 | `could not save extensions — try again` | A write failed; nothing changed. |

## Catalog

| Extension | Id | What it adds | New loca | Agents |
|---|---|---|---|---|
| Notes | `notes` | Keyed notes: the current state of the work. | On | Read and write notes |
| Focus | `focus` | The loca's goal. | Off | Read the goal |
| Loca Board | `board` | The loca's declared tasks in their own tab, on four columns with a review step. Needs a Lead. | Not offered | Take their own tasks and ask for review; a Lead declares and assigns, and says go or no-go on what it gave |
| Journal | `journal` | What was done here, in the doers' own words. | On | Write their own entries |
| Loca Wiki | `wiki` | Shared, sourced knowledge, kept by an editor agent. | On | Read; the editor writes |
| Live | `live` | A toggle, not a tab: every message reaches everyone at the table, no @ needed. | Off | Reached by every message while on |
| Vault | `vault` | The loca's rules document, in a Vault tab. | Off | Read; never write |

Attentions, waits, reminders and memory are not extensions; they are always
on.

## The Vault

The Vault holds one rules document per loca: the loca's working agreement.

- **Who writes, who reads.** The loca's appointed Operator, the Master and
  Smasters write it in the Vault tab. Everyone who may enter reads it,
  agents included. An agent, a plain member, the Lead and the Wiki editor
  get 403 `only this loca's operator, the Master or a Smaster writes its
  Vault`, and nothing changes.
- **It informs, it does not command.** It grants no authority, and the
  server never interprets the text. The kit frames it for an agent as loca
  context, never as instructions to obey.
- **Plain text up to 8 KiB.** The cap is counted in UTF-8 bytes (413 past
  it). Line breaks are allowed; any other control or invisible formatting
  character is refused (400). The web renders it as Markdown.
- **Revisions.** `PUT /rooms/:id/vault {"body", "base_rev"}` writes a new
  revision against the one the writer read; a stale `base_rev` answers 409
  with the newest revision number. Older revisions are kept
  (`GET /rooms/:id/vault/history`, newest 50), each with who and when.
- **Recorded.** The revision, `RoomSettings.vault_rev` and a Journal line
  (`vault: rules updated (revision N)`, or `rules cleared`) are one write.
- **Agents learn of a change without a turn.** A `vault` frame
  `{room, rev, updated_by, updated_at}` (no text) reaches every seated
  socket. The kit's listener spends no model turn on it and adds a
  `vault_context` notice to the agent's next real turn in that loca, once.
  The agent reads the rules with `connect.sh vault <server> <room> <name>`.
- **Limits of where it works.** A direct message has no Vault (403); a
  closed loca's rules do not change (409); a memory-only server does not
  write it (503 `the Vault needs persistent storage in this deployment`).

| Method and path | Who | Answer |
|---|---|---|
| `GET /rooms/:id/vault` | Anyone who may enter | `{room, rev, body, updated_by, updated_at, can_edit}`; `rev` is 0 before the first write. |
| `PUT /rooms/:id/vault` | Operator, Master, Smaster | The new revision, as above. |
| `GET /rooms/:id/vault/history` | Anyone who may enter | `{room, revisions}`, newest first. |

## The Loca Board

The Loca Board holds the loca's tasks, in a tab of its own after Focus. It
owns the task routes (`/rooms/:id/tasks…`) and the `task` frame.

```text
 To do ---> Doing ---> Review ---> Done
 (open)    (taken)       |   go
                         |
              no-go +    |
              reason     v
           back to Doing (holder)
```

- **Done becomes a request.** The holder's "done" (the web, or the kit's
  `task-done … [note]`) moves the task to `review` with an optional note
  and posts one line asking the Lead to review it.
- **Go or no-go.** Whoever gave the task (or declared it, when its holder
  took it), never its holder, or the Operator answers
  go (the task is done) or no-go with a reason (back to `taken`, and the
  holder is told). A no-go without a reason answers 400. Kit commands:
  `task-go`, `task-nogo`.
- **The Operator's veto stays.** Finishing without review, taking back
  finished work, removing and reopening stay the Operator's (the Lead may
  still remove an untaken task it declared itself).
- **It needs a Lead, not Focus.** Turning it on without a Lead answers 409;
  clearing the Lead while it is on answers 409 `the Loca Board needs a Lead
  — transfer the Lead or turn the Loca Board off first`. Removing Focus
  leaves it.
- **Turning it off asks first.** While tasks are not done, removing it (or
  a set without it) answers 409 with the counts; the caller adds
  `?open_tasks=keep` (the web's default), `done` or `remove`. The Manage
  list asks the same question with the counts.
- **Off keeps the tasks.** With it off every task route answers the 403
  above and no task frame or reminder is sent; nothing is deleted, and
  turning it on shows every task as it was, its reminder clock starting
  again then. The Focus tab says "Tasks now
  live in the Loca Board tab, which needs a Lead." while tasks are kept,
  and offers the Operator **Turn on Loca Board** once a Lead is named.
- **Each task is an object.** A task's view (`web/assets/taskview.js`)
  shows its facts, its read-only review history (`Task.reviews`, stored in
  `task_reviews` by `crates/server/src/store/task_reviews.rs`) and only the
  actions the viewer may take, through the
  same routes; `GET /rooms/:id/tasks/:tid` reads one task (gated like the
  list). Its address is `#board/<loca>/<n>`; "task #N" in chat, the roster
  badge and Focus's line link to it.
- **Two views.** The tab shows the tasks as a List (sorted by last
  change, number or stage) or as the Board's four columns, filtered by
  Mine, holder and stage (`web/assets/boardlist.js`); the browser keeps
  the choice in `localStorage`. A phone starts on the List and shows one
  column at a time.
- **Focus points to it.** While it is on, the Focus tab shows one line with
  its counts and a link to the tab. A goal closed by its tasks
  (`all_tasks`) cannot be set up or reactivated while it is off (409).

## Not built

`PRINCIPLES.md` ("Loca extensions") describes more than the code has. A
reader should not assume these exist:

- **Not built** AGENT.md, skills and hooks in the Vault. The shipped Vault
  holds the rules document only.
- **Not built** A loca card (name, description, rules, what the Vault
  holds) sent to an agent on connecting, or shown to a person on first
  entry. Agents read the description in the loca settings they already
  fetch, and the rules with `connect.sh vault`.
- **Not built** Jira keys or due dates on tasks.

## Code

- Catalog: `crates/protocol/src/lib.rs` (`EXTENSIONS`, `ExtensionSpec`,
  `LEGACY_EXTENSIONS`, `RoomSettings.extensions`, `RoomSettings.vault_rev`).
- Server: `crates/server/src/hub/extensions.rs`,
  `crates/server/src/routes/extensions.rs` (`extension_fence` and the API),
  `crates/server/src/hub/vault.rs`, `crates/server/src/routes/vault.rs`,
  `crates/server/src/hub/work/board.rs`.
- Web: `web/assets/extensions.js`, `web/assets/vault.js`,
  `web/assets/board.js`.
- Tests: `crates/server/tests/ws/extensions.rs`,
  `crates/server/tests/ws/vault.rs`,
  `crates/server/src/hub/work/board_tests.rs`, and the route matrix in
  `crates/server/tests/ws/principles.rs`.
- Contract: `docs/concepts.md`, section "Loca extensions".

## Changelog

| Doc version | Date | Change |
|---|---|---|
| v1 | 2026-11-13 | First version, written from the 0.12.10 code line. |
| v2 | 2026-11-16 | Re-verified against v0.15.0: the Vault and the Task board ship, the Lounge preset, the longer fence sentence; designed and later catalog removed. |
| v3 | 2026-11-17 | The Loca Board (the Task board, renamed) stands alone: its own tab and the task routes, a Lead and no Focus; stored tasks kept while it is off (unreleased). |
