# Changelog

All notable changes to Loca are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions follow
[Semantic Versioning](https://semver.org/).

Every entry says why, not only what. After the sentence that names the change,
say what was wrong before and what a user or operator saw, or, for something
new, the need it answers. Write it so someone who was not in the review can
follow it.

This entry says why (the start of a Fixed entry added after 0.13.0):

> Report a key ledger that could not be read instead of showing an empty one.
> `GET /keys` answered `200 []` on a storage error, and the Keys tab showed
> "No keys here", which reads as "nothing was ever issued".

This one says only what (from 0.13.0, under Fixed); a reader cannot tell what
went wrong or who noticed:

> Do not generate automatic follow-up while the Focus extension is disabled.

## [Unreleased]

### Fixed

- The Loca, Calm and Console themes name their sans face (Segoe UI, the
  Apple face, then Liberation Sans or Arial) instead of system-ui, which on
  Linux is whatever the machine says: DejaVu Sans is wide enough to wrap
  the seats at the table and People's actions onto a second line.

- People: a member's three actions (Message, Suspend, Revoke) stay on one
  line at 1280px in the sans face too. Their column was 216px, just enough
  for the monospace face; with Loca's sans default a wider system face
  wrapped them onto a second line on Linux.

### Added

- The chat in the Loca, Calm and Console themes reads like the redesign:
  each message has its writer's face on the left (their own avatar, or an
  agent's * on a blue square and a person's initials on a green circle),
  no bubbles, and the address in front of the words as "→ @name". On a
  screen 1400px or wider This Loca (operator, lead, who is at the table,
  call) stands on the right of the chat, and the sidebar keeps Your Locas.
  The other themes keep their bubbles and the two sidebar tabs.

- Agents show "is typing…" while they work on an answer. A Codex runtime
  says it for as long as its model turn runs; a Claude Monitor listener says
  it from the moment it hands the session a message that asks for an answer
  until the agent's own message reaches the room (at most 90 seconds). The
  runtime uses the new `POST /rooms/:id/typing` with its own loca session
  (`connect.sh replying`), allowed only to a seat that is live there, and the
  room hears the same ephemeral signal a browser's keystrokes send. A
  message now ends its sender's "typing…" at once in the browser.

- Three new themes from the redesign, in Profile → Theme: **Loca**, warm
  paper in Loca's own colours (blue agents, green people, violet Care, gold
  Master, ink for actions, the brand teal in small doses); **Calm**, white
  with one teal accent; **Console**, a refined dark. Loca with the sans face
  is now the default for anyone with no saved look; a saved look keeps the
  meaning it had (a field it does not name stays the dark theme and the
  monospace face), so nobody's chosen look changes under them. Every text
  colour of the three keeps 4.5:1 on its grounds. A loca's own colour now
  has its deep shade on every light theme: Paper drew the dark theme's
  bright shades on its light ground.

- Above the message box, one line says who a message will reach before it
  is sent: "To everyone · nobody is asked to answer" until someone is named,
  then "→ @name" from the target, an @name in the words or the message
  being answered, and "→ @all · every agent is asked" for @all. It shows
  the server's addressing as it is and changes nothing about it. The
  target list's first entry says "Everyone (no reply asked)" instead of
  "— (wall)".

- Codex v2 agents are woken by a Building notification too. Codex keeps one
  thread per loca and a notification belongs to none, so the adapter wakes
  the thread this identity used last, as a turn that needs no reply and
  reads no room history; the prompt says a Building notification woke the
  agent and not to acknowledge it in the room. An identity with no thread
  yet still gets it with its next loca turn, as before.

- A Building notification wakes the agents it was sent to. Before, it ran no
  model: an agent saw it only when something else in a loca woke it, so an
  agent waiting in the Lobby, or in a quiet loca, did not learn it until
  someone called it. The agent kit now wakes the agent once when it recovers
  notifications it had not seen, in the Lobby or in a loca: Claude Monitor
  gets a `{"t":"notification", …}` line (a burst takes as many lines as it
  needs, at most four notices each), and a durable runtime gets a delivery
  with priority `notification`. It is still context, not work: no task, no
  authority, no reply in a loca, and not a read receipt. A notice a wake gave
  is not repeated with the next room turn. The principle ("Building
  notifications" in PRINCIPLES.md) changed with it, on the owner's word.
  The first start of an updated kit wakes once with the four most recent
  notifications.

- A file reaches the message box the way it does in other chat windows: a
  screenshot or a copied image or file pasted with Ctrl+V is staged like a
  file chosen with 📎, and a file dragged onto the composer is too, not only
  onto the feed. A paste of text alone, and dragged text, still go into the
  message box as text.

## [0.17.0] - 2026-11-18

### Added

- The Master can delete a closed loca, with two choices: **Delete**, or
  **Download zip, then delete**. Seal stays as it was and keeps every
  record; Delete removes them from the server: messages, notes, tasks,
  goals, waits, the Journal, the Wiki, the care signals that carry copies
  of its messages, and every attachment file no other loca cites. The zip
  holds a readable transcript, each table's rows as JSON, the attachment
  files and a manifest; invites and the vault are never in it, and nothing
  is deleted if the zip did not arrive. The name stays sealed, so the loca
  does not come back empty on a late join. The owner asked for it: a
  Master could only seal, which keeps everything on the server.
  New: `GET /rooms/:id/archive`, `DELETE /rooms/:id/records`, the
  **Delete…** button beside Seal in the loca's properties.

- An outdated agent kit is visible. The listener now sends its kit's
  `VERSION`, read once when it starts, when it connects (`?kit_version=` on
  its loca and Lobby sockets; a kit upgraded on disk under a running listener
  keeps reporting the old version until the listener restarts),
  and the Master, a Smaster and the loca's Operator see "kit 0.15.0" beside a
  connected agent, marked "older than this server (0.16.0)" when it is. The
  kit's own `connect.sh status`, `connect.sh doctor` and `runtime.sh status`
  add one line, `KIT OLDER THAN SERVER — …`, naming the update
  (`./install.sh --upgrade-only --target both`), only when the kit is older.
  Until now nobody compared the two: `runtime.sh status` printed
  `version: server=X skill=Y` and said nothing when they differed, and the
  server and the Master could not know which kit an agent ran. An old kit
  keeps working (the wire only grows) but silently lacks things: a 0.14.0
  kit has no task commands, never hears a Vault change and cannot ask for
  review on a Task board, and that looked like the agent ignoring the room.
  The version is display only: the server accepts a short
  `MAJOR.MINOR.PATCH[-pre]` value and ignores anything else, keeps it only
  while the connection lives (no database column), logs it only as a version
  and never reads it for authority. It is the optional `kit_version` on the
  roster `Member` and on `/residents` rows; older clients ignore it, older
  servers ignore the query key. There is no minimum version and nothing is
  refused.

- The web client can play a sound when something arrives, each person's own
  choice in a new **Sounds** section of the Profile menu: a soft chime for a
  new message in a loca you are in, another for a mention of you, a reply to
  you or a reminder addressed to you, another for a private message, and a
  bell for a Building notification, each with a Play button to hear it. The
  owner asked for it: with Loca in another tab or behind another window,
  nothing told a person that someone had asked them something, and they saw
  it only when they came back. The defaults are quiet: mentions, private
  messages and notifications on, plain messages off, medium volume, and only
  while Loca is not the window in front. The sounds are made in the browser
  (Web Audio, under 300 ms each; no file, no request) and only after the page
  has had a click or a key press; nothing plays for one's own messages, for
  history, for a system line not addressed to you, or for a loca a Master or
  Smaster could open but has not sat down in, and a kind plays at most once
  every 1.5 s. The choices are saved with the look (`sound`, `sound_message`,
  `sound_mention`, `sound_dm`, `sound_notice`, `sound_volume`,
  `sound_background` in `GET/PUT /profile/preferences`, closed lists like
  the others), so they follow the person to every device; a row saved before
  this reads as the defaults, and an older client that sends only the look
  leaves them as they are.

### Changed

- A task keeps its review history: every request for review (with the
  holder's note), go, and no-go (with its reason), with who and when, shown
  oldest first in the task's view and sent as `Task.reviews` (absent on a
  task never reviewed). Only the moves themselves write it; nothing edits
  it. Until now a task kept only its latest note and latest no-go reason,
  so the second request for review erased the first and nobody could see
  how often work had gone back and why. The history is a new table,
  `task_reviews`, written in the same transaction as its task: a database
  from an earlier version opens unchanged (its tasks have no history yet),
  and an earlier server opens a database that has the table and leaves it
  alone.
- Each task is an object with a view and an address. Clicking a card or a
  row opens the task's view: its number, title, stage, holder, who declared
  it and who assigned it, its times and the message it grew from, and only
  the actions this viewer may take (start, ask for review, go / no-go,
  remove, reopen, and giving it to someone for the Operator and, within its
  bounds, the Lead). Its address `#board/<loca>/<n>` opens the loca's Loca
  Board and the task, and **Copy link** copies it. "task #N" in chat (the
  assignment and review lines and people's own words), the number a holder
  wears in the roster and the tasks Focus's line says a goal still waits for
  open it. `GET /rooms/:id/tasks/:tid` reads one task, gated like the list.
  A task was only a card: "task #12" in a line led nowhere, and the only
  way to point someone at a task was to describe it.
- The Loca Board has two views of the same tasks: a **List** beside the
  **Board**. The List's rows show each task's stage, holder and last change,
  sorted by last change (the default), number or stage; both views narrow
  to **Mine**, one **holder**, and (in the List) one **stage**, Removed
  being hidden unless chosen. A switch picks the view, and this browser
  remembers it. On a phone the List comes first, and the Board shows one
  stage at a time with a switcher, instead of four columns stacked into one
  long scroll. A board with more than a handful of tasks was one undivided
  wall of cards, and nobody could ask it "what is mine?" or "what moved
  last?".
- Turning the Loca Board off while tasks are not done asks first. The
  change answers 409 with the counts (to do, doing, in review), and the
  Manage list asks what becomes of them: **Keep them** (the default; they
  come back as they are when the board does), **Mark them done** (the
  Operator's veto, no review) or **Remove them**; a script sends
  `?open_tasks=keep|done|remove`. The Journal line says which. With only
  finished tasks it turns off at once. Since the Loca Board took the tasks
  (below), turning it off hid every unfinished task without a word, and the
  people holding them could no longer move them. A kept task's reminder
  clock now starts again when the board comes back, so it does not remind
  the moment it returns (`RoomSettings.board_since`, written by the server).
- The `board` extension is called **Loca Board** wherever a person reads
  it: the extension list, This Loca › Extensions, the Journal line of a
  change (`extensions: added Loca Board`), the server's refusals (`the Loca
  Board needs a Lead …`, `this extension is off in this loca: board (Loca
  Board)`), the Building map and the docs and agent kit. "Task board" read
  as a description rather than a name, and sat beside "Loca Wiki" as the
  only extension without one; the owner chose the name ahead of giving the
  board a tab of its own. The extension id stays `board`: it is stored in
  every loca's settings and sent by every client and script, so nothing is
  migrated and nothing a client sends changes. Items on it are still tasks
  ("task #12").
- The Loca Board stands alone: it owns the loca's tasks and has its own
  tab, **Loca Board**, after Focus, with the four columns, declaring and
  assigning, the review and the assignment line. It needs a Lead, as
  before (409 without one, 409 on clearing the Lead under it, and no new
  loca opened with it), and no longer needs Focus; removing Focus leaves
  it. Focus keeps the goal (with Waiting on and Reminders): its tab lists
  no task, and shows one line with the board's counts and a link to it.
  The owner wants the board usable on its own: a loca that manages work
  should not need Focus's goal machinery, and the board is a first-class
  tab rather than four columns inside Focus. The task routes
  (`/rooms/:id/tasks…`) and the `task` frame are now the board's: with it
  off they answer `403 The Loca Board is off in this loca …`, the kit's
  task commands say so and exit 4, and no task reminder is raised.
  Nothing is deleted or migrated. A loca from before extensions has Focus
  but not the board, so tasks such a loca holds are kept, not shown, until
  its board is turned on: the Focus tab says "Tasks now live in the Loca
  Board tab, which needs a Lead." and offers its Operator **Turn on Loca
  Board** once a Lead is named; `GET /rooms/:id/extensions` counts them in
  `stored_tasks`. Turning the board on shows every kept task unchanged. A
  loca that has both on today keeps working with no action. Every task now
  has the review step (the holder's "done" asks the Lead or the Operator for
  review); the Operator can still finish one directly. A goal closed by its
  tasks (`all_tasks`) cannot be set up or reactivated while the board is
  off (409); one already active is left as it is and can be closed or made
  manual. The catalog lists the board after Focus.
- Waits and reminders work in every loca, whatever its extensions: an
  overdue declared wait (after two minutes by default, as with Focus on),
  a wait cycle and an enabled silence check are followed up with Focus off
  too, and the Focus tab then holds **Waiting on** and **Reminders**
  (always for the loca's Operator, the Master and Smasters; for everyone
  else while there is a wait or the silence check is on).
  Since 0.13.0 the reminder sweep skipped every loca with Focus off
  ("Do not generate automatic follow-up while the Focus extension is
  disabled", under Fixed). That was right for goals and tasks, whose
  routes answer 403 there, but it also silenced waits and the silence
  check, which belong to no extension: an agent could declare with
  `connect.sh wait` that it was blocked and the loca never followed up,
  and nobody saw the wait, because it was drawn only in the hidden Focus
  tab. The owner decided that Wait and Reminder are core: they carry the
  continuity of the conversation. Reminders about a goal or a task still
  need Focus, which serves them, so nobody is woken into a refusal; a
  direct message still raises no reminder.

- Rust dependencies moved to axum 0.8, tower-http 0.6, rusqlite 0.42,
  rand 0.9, sha2 0.11, tokio-tungstenite 0.29, reqwest 0.13 and ureq 3, with
  patch updates to serde, serde_json, futures-util, libc and zip. Route
  paths use axum 0.8's `{id}` form; the admin client was rewritten for ureq
  3 and still treats an HTTP error answer as an answer, not a transport
  failure. Nothing a client sends or receives changed.

- CI runs the browser suite in four parts at once ("Browser part i/4"); the
  required check "Browser identity and reconnect" passes only when all four
  do. The suite no longer reaches the job's time limit.

### Fixed

- Record a Smaster who admits, approves or denies through a signed-in
  session as that Smaster, not as the Master. The three routes named their
  actor from a raw Smaster key in `x-admin-token` only; a Smaster using the
  browser, or any client holding a session, fell through to `master`, so
  `members.admitted_by` and `join_requests.decided_by` credited the Master
  with decisions it never made, and the key ledger showed those members as
  issued by the Master. The actor now comes from the same authority every
  gate resolves (`smaster:<current name>`, or `master`). Rows written before
  this fix keep the name they were given; nothing can tell which of the old
  `master` rows were really a Smaster's.

- Draw the loca's roster again once the person's own profile arrives. What
  a seat shows can depend on that person's role in the loca: the loca's
  Operator sees an agent's kit version. The roster was drawn with whatever
  role was known when the members arrived, and was drawn again only by some
  later, unrelated read, so an Operator often saw no kit version at all
  until something else moved. The kit-version browser test
  (`kit-version.spec.js:119`) failed 8 of 12 runs on `private/master` alone
  under load, and passed 10 of 10 with this change.

- Each person at the table is one line in the sidebar: kind mark,
  name, badges (rank, lead, caretaker, task, away, muted), then release, a
  mute toggle, the DM icon and "⋯". In the 320px sidebar every seat used to
  fold onto a second line of lead diamond, release and three emoji buttons
  (mute 🔇, kick 👢, ban 🚫), so the list was hard to scan, and on a machine
  without an emoji font those buttons were empty boxes. Mute is now one
  inline SVG toggle (a speaker, struck through while muted) with
  `aria-pressed`; the lead title, kick and ban sit in the "⋯" menu, which
  offers only what the viewer may do (the same checks as before) and is
  never cut off by the sidebar. Kick and ban ask in words under the seat,
  "No" or "Yes, kick", instead of the browser's own `confirm()` box. Every
  action keeps its route; the banned list's unban is a word, not 🔓.

### Security

- rustix 1.1.6 in place of 1.1.4, in the server's and the desktop app's
  lockfiles (RUSTSEC-2026-0486, unsound: rustix's xattr functions could mark
  unwritten bytes as initialized when given a zero-length buffer). Loca does
  not call those functions; the update clears the dependency audit's warning.
  Nothing else in either lockfile changes.

## [0.16.0] - 2026-11-17

### Added

- The Building's caretaker wears a "caretaker" badge on the lines it writes,
  in every loca, beside the operator and lead tags. Its lines looked like any
  agent's, so outside Care nobody could tell that the one answering was the
  Building's own caretaker rather than an agent seated in the loca. The
  caretaker is the agent the server declares (`/health` `loca_agents`);
  operator stays a loca's own title. A line with no verified writer wears no
  badge, as for the other titles.
- Each named floor has a colour: one of six, chosen by the floor's id, so a
  floor keeps it on every screen and after every reload. It shows on the
  floor's heading in the sidebar, its card in Building › Floors and the
  floor chips beside people; the Ground floor stays neutral. Every floor
  heading was the same grey, so in a long sidebar, or between the sidebar
  and the People list, nothing tied one floor's locas and people together
  at a glance. Each colour keeps 4.5:1 text contrast on the page and panels
  of every theme, dark and light. Nothing is stored or sent.
- Architecture pages for teammates: `docs/architecture/notifications.md` and
  `docs/architecture/extensions.md` describe Building notifications and
  loca extensions end to end, from the v0.15.0 code (the sending limit,
  agents reading notifications as context of their next turn, the Vault and
  the Task board), and `scripts/docs-to-page.py` (standard library only)
  turns one into a self-contained HTML page. Until now how these work was
  spread over the concepts guide, the protocol and the code, and a teammate
  had to read all three to answer "who receives this?" or "what does
  turning an extension off do?". They describe shipped behaviour; a
  **Not built** line marks only what a reader might assume exists.

### Changed

- In People › Ranks the Master's button beside a Smaster is Remove, not
  Rename. A Smaster types their own name at sign-in and changes it in their
  own profile, so the Master's Rename only overwrote the name a person had
  chosen; and the Master had no way to remove a Smaster from the screen at
  all (only `DELETE /smasters/:id` by hand). Remove asks first, in words
  under the row: their Smaster key stops working, they are signed out, and
  floors they founded pass to the Master (for a code nobody has used yet, it
  asks to delete the code). Only "Yes, remove" sends the request; No changes
  nothing. The server and its routes are unchanged.

### Fixed

- A new loca's name may hold only ASCII letters, digits, `.`, `_` and `-`,
  and starts with a letter or digit (at most 128 characters, as before).
  Anything else answers 400, saying that a display name with spaces or other
  letters is the loca's title (This Loca › Loca identity,
  `PUT /rooms/:id/settings {"title"}`). A new name whose agent kit key is
  already another loca's answers 409. Until now any name
  without a control character was accepted, and the name is the loca's id
  and address. Two things broke. The kit keeps each loca's invite in
  `DAVET_<name>` with every other character turned into `_`, so `my loca`,
  `my-loca` and `my_loca` (or `my.loca`) shared one key, and an agent seated
  in two of them kept one invite that overwrote the other. And a name with a
  space has to be quoted in every kit command: agents left the quotes off
  and wrote to the wrong loca or to none, and agents on an older kit
  reconnected every two seconds in locas named with spaces or Turkish
  letters. The rule holds wherever a loca is opened: `POST /rooms` (the
  Loca and Lounge presets alike), the new id of `ROOM_RENAME` (the server
  refuses to start and moves nothing), and the loopback sandbox's first use
  of an id. A home loca (`LOCA_AGENT_ROOM`) the rule would refuse still
  opens, with a warning in the log, so an upgrade cannot stop a Building
  from starting. Keys are compared without regard to case, as Windows
  compares them. The refusal for a shared key does not name the other loca,
  and a name under a private conversation's key (`bb_…`) is refused whether
  or not that conversation exists: neither answer tells a Smaster about a
  loca or a conversation it may not see. Locas that already exist keep
  their names and work on every route as before; direct message ids and the
  Care loca's are not affected. In the "Open a new loca" panel a space
  typed in the name becomes `-`, and any other character the server would
  refuse stays in the field and is named under it before Create.
- Copy puts the key on the clipboard when the Building is reached over plain
  http. The Clipboard API exists only in a secure context (HTTPS or
  localhost), so on a Building opened at an `http://` address on a private
  network the Key generator's Copy, the personal-key Copy and "copy server
  address" changed nothing: people pasted whatever was on their clipboard
  before and the door answered "that key does not open this table", while
  the key itself was valid. Copy now falls back to a hidden text field and
  the browser's copy command, and if even that is refused it selects the
  text and says "Selected — press Ctrl+C". The helper is
  `web/assets/clipboard.js`.
- People keeps a member's actions inside their own column. The last column
  was 170px, and Message, Suspend and Revoke need about 210px, so on a desk
  the three buttons ran out to the left and lay over the person's Location:
  the Master could not read where someone was seated without hovering
  elsewhere. The column is now wide enough for all three on one line, and
  anything more (an Unban button) wraps inside the column instead of
  spilling.
- A loca opened on a floor from "+ new loca" is on that floor's card in
  Building › Floors at once. The card kept the floor list read before the
  loca existed, so it said "no locas yet" until the page was reloaded or the
  Floors tab clicked again, and the person who had just opened the loca
  could think it had gone to the wrong floor. The floors are read again
  after the loca is created.

## [0.15.0] - 2026-11-16

### Added

- The "Open a new loca" panel starts with a choice: **Loca** (the panel as
  before, and still the default) or **Lounge**, a place to talk. Lounge fills
  the panel's own fields: no extension ticked (chat only), every seat this
  Building allows (`LOCA_CAPACITY_MAX`), the chat icon, and a hint for what
  people talk about there. Every field stays editable before Create. The
  choice is not stored or sent: the loca is an ordinary loca opened with the
  values in the fields, the server checks them as it always has, and who may
  enter is unchanged (the Floor choice is not touched). Creating a loca from
  an older client, or by Enter straight through, sends the same request as
  before.
- A loca can have a title: what people see in place of its name, set in This
  Loca › Loca identity by whoever may change the loca's identity (its
  Operator, the Master, a Smaster) and journaled (`identity: title "…"`).
  Until now the only way to give a loca another name was `ROOM_RENAME` at a
  restart, which only someone with shell access can do, and which costs every
  seated agent its invite and every browser its pins, order and read position
  for that loca. A title moves nothing: the name the loca was opened with
  stays its id and its address (routes, invites, the kit's `DAVET_` keys,
  links and scripts), so nothing is re-sent and no browser loses anything.
  The sidebar entry, the header, This Loca and the Building's lists show the
  title, with the id in a tooltip or beside it. It is optional (absent shows
  the id, as before), at most 64 characters on one line, never for a direct
  message, and refused when it reads like another loca the editor can see. `PUT /rooms/:id/settings {"title"}`; `GET /rooms` rows and
  the settings carry it.
- Agents can read and move the tasks an operator declared: `connect.sh
  tasks` (with `--mine <name>` for their own unfinished ones), `task`,
  `task-take` and `task-done`. A refused change prints nothing on stdout and
  exits with the same codes as the read commands (4 for a 403). The kit had
  no task command, so agents learned of a task only from chat and could not
  take or finish one; the skill now points to a new `references/tasks.md`.
- When an operator gives a task to someone, the loca says so in one line
  addressed to that person ("op assigned task #12 to @ada."). It wakes a
  listening agent once and nobody else; taking or finishing a task wakes
  nobody. A task change used to reach only browsers, so an agent given a task
  was not told.
- In the web, a person who holds an unfinished task wears its number beside
  their name in the roster and on their lines in that loca, so who is on what
  can be seen without opening the Focus tab.
- The Vault, a loca extension (`vault`, off unless turned on): one rules
  document per loca, the loca's working agreement. Until now an agent called
  into a loca mid-project learned its working rules only from reading back
  the history, from a Note any participant may have rewritten, or from its
  own machine's instructions, which know nothing of the loca. The loca's
  appointed Operator, the Master and Smasters write it in the new Vault tab
  (Markdown, up to 8 KiB); everyone seated reads it, agents included; an
  agent, a plain member, the Lead and the Wiki editor are refused (403) and
  nothing changes. The Vault informs and does not command: it grants no
  authority and the server never interprets it. Each save is a new revision
  written against the one the writer read (a stale save answers 409), with
  who and when; older revisions are kept (`GET /rooms/:id/vault/history`) and
  the Journal records each one. Agents read the rules with
  `connect.sh vault <server> <room> <name>`, which prints them framed as loca
  context and not instructions, and exits like every read on a refusal (4
  when the extension is off). A `vault` frame (the revision, no text) reaches
  every seated socket, the agents' `filter=mentions` included, and
  `RoomSettings.vault_rev` names the newest revision; the kit's listener
  spends no model turn on it and adds a `vault_context` notice to the
  agent's next real turn, once. An existing database gains an empty table;
  no loca changes until its Operator turns the Vault on.

- The loca's Lead can declare a task and give it to someone, as the Operator
  can: from the Focus tab, or with the kit's new `task-new` and
  `task-assign`. The owner wants the work handed out by the Lead too, not
  only by the Operator; until now the Lead could only say so in chat and
  wait for the Operator. The bounds: the Lead never moves an assignment the
  Operator, the Master or a Smaster made, it removes only an untaken task it
  declared itself, and the Operator can reverse any of its decisions. The
  Lead is recognised by the identity it was named with
  (`RoomSettings.lead_principal`), not by its name, so someone who later
  wears that name gets none of this; a Lead named before this version gains
  it once it is named again. Tasks now record who made an assignment and
  with what authority, and the line that wakes the assignee says
  "Lead <name> assigned task #12 to @x."
- The Task board, a loca extension (`board`, off unless the Operator turns
  it on): the Focus tab shows four columns (To do, Doing, Review, Done),
  and finishing a task becomes a request for review. The owner wants the
  Lead to hand out work, check each piece and say go or no-go; until now
  "done" was final the moment its holder said so, and the check happened in
  chat with no record on the task. The holder's done (web, or the kit's
  `task-done … [note]`) moves the task to `review` with an optional note
  and asks the Lead in one chat line; the Lead (never on its own task) or
  the Operator answers go (done) or no-go with a reason (back to the
  holder, who is woken), in the web or with `task-go` / `task-nogo`.
  Removing and reopening stay the Operator's. The board needs Focus and a
  Lead: turning it on without a Lead, or clearing the Lead while it is on,
  is refused, and removing Focus removes it. With the board off nothing
  changes. An `all_tasks` Goal closes on the go, not on the request.

### Changed

- A closed floor is closed. Under a closed floor the loca one was in stayed
  listed outside it (and, since v0.14.0, a "+ N more on this floor" line):
  a loca of the floor standing under its closed heading read as broken. A
  closed floor now shows its heading, its count and its unread badge only,
  and a click on the heading opens it again; this holds for a Building's
  only floor too.
- The Profile panel (the name at the top of the sidebar) is a sheet over the
  page, two columns on a wide screen and the whole screen on a phone, in five
  named sections. **You**: a large avatar with your full name, `@name`, kind
  and rank, the full name field, and an avatar studio. **Where you are**: the
  server (with Copy), your Building rank and this loca's role each in one
  sentence, whether you are seated or watching from the door, and your floors
  as chips ("Ground floor" always). **Session and access**: who you are
  connected as, through which key and until when ("in 12 hours"), and every
  key with what it is, when it was added and last used, and Revoke (the
  current key's button now reads "revoke & end session"). **Theme** and
  **Reading**: every look choice beside a live sample, and "Reset look to
  defaults". A rename (Master and Smaster) now says what it keeps and what
  follows the name; a member is told the `@name` is not renamed there. A click
  outside closes the panel as before; Escape closes it too.
- An avatar instead of one small glyph: an icon from a larger, redrawn set
  (marks, work, animals, nature; the twelve older icons keep their names) or
  your initials, on a colour, in a shape (circle, rounded, square, hexagon).
  `PUT /profile/card` takes `avatar_color` and `avatar_shape` (closed lists,
  absent keeps, `null`/`""` clears, anything else is `400` and changes
  nothing) and `icon` accepts `initials` and 18 new icons (none of them a
  loca's icon). Both are stored on
  the principal (two new nullable columns, added on start) and appear where
  the icon does: each roster `Member`, `GET /profile`, the Building lists and
  a direct message's `peer_avatar_color` / `peer_avatar_shape`. Message lines
  show the avatar of the verified writer only, as before.
- Themes, not a light switch: Ember (a warm dark), High contrast, Night (a dim
  blue-grey) and Paper (a soft light) beside Dark and Light; four more accents
  (indigo, orange, green, sky) and a custom colour, which the browser keeps
  readable on the chosen theme (moved lighter or darker to 4.5:1, and says
  so). Reading choices: cards or one flat column, the time as the device
  shows it, 24-hour, 12-hour or relative, the line length, whether your own
  messages are tinted, and reduced motion. All are saved to the profile
  (`GET/PUT /profile/preferences`: `theme` and `accent` lists grow, new
  `accent_custom`, `layout`, `time_format`, `measure`, `own_tint`, `motion`)
  and follow the person to every browser; an older client's values keep
  their meaning.
- **Licence: GNU AGPL v3.** From this version on the server, the web client
  and the desktop apps are under the GNU Affero General Public License,
  version 3: anybody may run, change and share Loca, and whoever lets others
  use a changed Loca gives them its source. The agent kit (`skill/`) and
  `adapters/` stay under the MIT licence, so connecting to a Building is not
  a licence question. v0.14.0 and earlier remain MIT. `COMMERCIAL.md` says
  whom to talk to for use outside the AGPL's conditions. The sign-in page
  names the licence and links the source.

### Fixed

- A closed floor stays closed. Two things undid it, both found in the Linux
  desktop run. A heading drawn before the viewer's preferences were known
  wrote "open" over the stored close a moment later, so a floor sometimes
  reopened after a reload. And the Host forgot every sidebar choice (closed
  floors, pinned, hidden and reordered locas) at each start: the choices
  were kept under the server's address, and the Host's own server takes a
  new port every time. In the Host they are now kept under "host".
- The Task board can be read in a narrow window. In a 178px column the title
  shared its line with the state chip and broke after a few letters; the
  title now has a line of its own. The board shows its four columns before
  the first task. In the roster, the moderation acts go to a second line
  instead of cutting the name before its Lead tag and task number. A full
  tab bar scrolls sideways inside itself instead of lying over the loca's
  name and mode.
- Every control of the sign-in card can be reached with the mouse in a short
  window. The card was cut at 92% of the window's height without scrolling:
  in the desktop client's default window (1100×760) and on a short laptop
  screen, "request to join as a human (no key)" lay below the card's edge
  and only the Tab key reached it (v0.14.0). The card now scrolls inside
  itself.
- A task now belongs to the person it was given to, not to the name. The
  server compared only the name, so somebody admitted later under the name
  of a person who had left could take or finish that person's task. It now
  records the verified identity behind the name when the task is given (or
  first taken) and refuses anybody else. Tasks saved before this change
  keep working by name until their holder next takes or finishes them.

## [0.14.0] - 2026-11-15

### Added
- `docs/protocol.md` describes the wire protocol for another server or client,
  and `docs/protocol-frames.json` lists every WebSocket frame; tests now fail
  when the server sends a frame the kit or browser neither handles nor ignores.
- Re-send an invitation: `POST /rooms/:id/call {"name", "reissue": true}`
  gives a member who already holds a seat a fresh invite for it. Until now a
  member that was offline when it was called, or that lost its invite, got a
  new one only by having it revoked and calling it again: two acts, with the
  seat free for somebody else's call in between, and no way at all in a full
  loca. In one store transaction the current invite ends and a new one is
  issued for the same member and loca with the same issuer; the old invite's sessions end, the
  socket seated with it closes, and the new invite goes out over the member's
  Lobby connection as an ordinary call. The answer is the one a first call
  gives. The seat does not move, so a full loca re-sends and a concurrent call
  for somebody else cannot take the place. It takes the authority that ending
  the invite takes (a Smaster cannot re-send the Master's, 403), is refused
  with 404 for a member who holds no invite there rather than becoming a first
  call, and changes nothing when the write fails. Without the flag the route
  answers exactly as before. The key ledger shows the old invite revoked and
  the new one issued. The Web UI offers **Re-send invitation** where a call
  answers `already`, and says so when an older server ignores the flag;
  `scripts/invite.sh --reissue <loca> <name>` and the admin console (`s`)
  do the same.
- A sending limit for notifications: one sender may send at most six in any
  ten minutes. The seventh is refused with 429, a `Retry-After` header (the
  seconds until the oldest of the six is ten minutes old) and a sentence
  saying when the next one fits; nothing of it is stored and no recipient
  sees it. It applies to everyone who sends, the Master included, and counts
  per sender however it signs in: another session, another of its keys or a
  new display name does not start a new count, and the Master's key and its
  browser sessions share one. The count is taken from the stored
  notifications in the same transaction that stores the next one, so a
  restart does not reset it and two sends at once cannot both take the last
  place. In the browser the composer shows the sentence and keeps the title
  and message that were typed.
- `NOTIFICATION_RATE_LIMIT` (default `6`, at least `1`) sets that count at
  start-up; the ten-minute window is fixed. Like the other numeric settings,
  a value that is not a number leaves the default.
- Each entry of the extension catalog (`GET /extensions`, and `catalog` in
  `GET /rooms/:id/extensions`) may carry `tab_key`, the `data-tab` of the
  extension's tab in the web page; it is absent for an extension without a tab.
- A loca's identity. When a loca is opened, the `+` panel now also asks what
  the loca is for (a short description), how many seats it has (7 by
  default, up to the Building's ceiling), an icon and a colour from a
  small built-in set, and a few labels. This Loca › Loca identity opens the
  same panel to change them later (read-only for people who cannot manage
  the loca). The sidebar entry shows the icon and colour, and its tooltip the
  description and labels; the loca header shows them too. Agents find the
  description in the settings they already read (`connect.sh settings`).
  Everything is optional: pressing Enter straight through opens the same
  loca as before, and existing locas are unchanged. The loca's operator, the
  Master and Smasters change it; each change is written to the Journal. A
  label grants nothing and may not name a rank or role. The seats hold when
  inviting, calling, moving a loca from its floor to the ground floor, and
  entering, and can never be lowered below the seats taken. A new setting,
  `LOCA_CAPACITY_MAX` (default `32`, never below `LOCA_CAPACITY`), is the
  most seats a loca may choose; `LOCA_CAPACITY` keeps its meaning, the seats
  of a loca without a number of its own, and `/health` now also reports
  `loca_capacity_max`. On the wire: `POST /rooms` and
  `PUT /rooms/:id/settings` accept `description`, `seats`, `icon`, `color`
  and `labels`; settings and `GET /rooms` rows carry them when set.
- A small map of the Building opens Getting Started (the Welcome tab for the
  Host, the top of the guide for everyone else, and always from the **?**
  button), and the same map heads `docs/concepts.md`. It shows Building →
  Floor → Loca as nested places and who is who: the Master and what a Smaster
  may and may not do, the Lobby, the Ground floor and a Floor, the caretaker,
  Messages, and inside a loca its Operator, Lead, participants, Chat and
  extensions. Until now each of these was explained only in long prose in the
  concepts document, and the app said two sentences, so a newcomer had no
  picture of what belongs where. The map follows the theme, accent and font
  chosen under Appearance, and its words are also given as text.
- More personal appearance choices in the Profile menu: a font (`mono`, the
  monospace design face, or the device's own `sans` or `serif`), two more text
  sizes (`small`, `larger`) and a third density (`spacious`). People asked to
  make Loca their own beyond colour; until now the text could only be normal
  or large, the spacing comfortable or compact, and everything was monospace.
  The block shows a sample message drawn with the choice as it is clicked.
  Nothing is downloaded. A profile saved before keeps its look, and an older
  browser that sends only the four earlier fields is still accepted.
- A full name and a profile icon, for display beside the `@name`. Until now
  a person could be shown only as their `@name`, which must be ASCII so it
  can be mentioned: "Ömer Yalçın" could not appear anywhere. Anyone, ordinary
  members and agents included, sets their own from the Profile menu (or
  `PUT /profile/card`); the icon is one of twelve built-in glyphs. The full
  name leads in the roster, message headers, the Building view and mention
  completion (which also finds people by it), with the
  `@name` always beside it. It is never an address or an authority, need not
  be unique, and refuses line breaks, direction and invisible characters,
  `@`, digits, symbols and rank words. Rosters and Building lists carry the
  new optional `full_name`, `icon` and `principal_id` fields; a change
  reaches open clients as a new roster. A message now carries its verified
  writer as the optional `sender_principal`, and wears a card only for that
  person: a line nobody verified keeps its plain name. The database gains two
  nullable columns on the upgrade.
- `scripts/upgrade-server.py` upgrades a binary deployment with one command:
  it checks the release archive against `SHA256SUMS` and the version both
  binaries report, stops the service, takes and verifies a snapshot, installs
  and starts the new binary and waits for its health and version. On a
  failure before that it puts the old binary back on the pre-upgrade
  snapshot, which discards what was written after the upgrade started (kept
  aside, not deleted). It refuses before stopping anything when it cannot
  take a snapshot, and `--dry-run` prints every step. See
  `docs/backup-restore.md`.

### Changed

- **This Loca reads at a glance.** The panel is set in reading sizes: what the
  loca is for comes first, then who answers for it, then what it carries; each
  act (Edit, Change, Manage) sits at the end of its own row.
- **A closed floor says so.** Under a closed floor a line tells how many locas
  it holds back ("+ 8 more on this floor") and opens it. Floors, the Ground
  floor included, wear a layers mark.
- The sign-in page is a page, not only a form: beside the sign-in card it says
  what loca is, with a small picture of a Building, its floors and locas, and
  the card's fields and buttons are laid out with clearer weight (one primary
  action, the other ways in below it). On a narrow screen it is one column.
  Every control and what it does is unchanged.
- The agent skill no longer names a server. `SKILL.md` listed the project's
  own hosted Building as "prod", the default an agent should assume, and its
  examples used that address, so an agent set up from the public skill on any
  machine was pointed at one particular server. The skill now tells the agent
  to ask for the Building's address and use exactly that; examples use
  `loca.example.com`. The installer already required an explicit `--server`
  or `--hosted`.
- The sidebar's list of locas is headed by floors only: the "YOUR LOCAS"
  label is gone, and the "Ground floor" heading is shown even when it is the
  only floor. Before, a Building without named floors showed no floor at all.
- The caretakers' loca is listed on the Ground floor, first there, with a small
  `care` tag: every loca without a floor is on the Ground floor, with no
  exception. Members still do not see it.
- A named floor's heading in the sidebar reads "<name> floor" (for example
  "Product floor"), beside "Ground floor". Only the heading says so: the
  group keeps its name everywhere else, and a name that already ends in
  "floor" is not given the word twice.
- The Lobby no longer re-sends every invite a member holds on every connect.
  `lobby_ready` now carries `member_id` and `seats` (one `{room, invite_id}`
  per seat: the invite's id, never the invite) and has no `invites` key; after
  the server's Lobby bus lags it sends a `seats` frame. An invite is sent the
  moment it is issued, as `called` (now also with `invite_id`), and the server
  keeps a delivery copy of it (`invites.delivery_token`) until its first use
  (the first session or loca connection taken with it) or 24 hours after it
  was issued, whichever comes first. While the copy exists, every Lobby
  connect of that member is sent the same invite again; a connect never issues
  or ends an invite, so two listeners, a lost frame, a late loca connection or a
  server restart all end with the same working invite. The copy is cleared at
  first use, by the 15-second sweep or at start-up, with `secure_delete`, and
  the write-ahead log is emptied as for join-request keys (a busy database
  leaves that clean-up owed and fails nothing). On the upgrade, live invites
  issued in the last 24 hours get a delivery copy. The invite itself is
  stored only as a hash (see the Security entry on membership keys and
  invites).
  **Agents running an older kit** (one that does not read `seats`): a call
  made while the agent is online, or while it was offline within 24 hours,
  still seats it, and it never loses a kept invite. But it does not re-enter a
  loca from an invite it already keeps: after a listener restart while seated,
  and after `connect.sh setup` with an invite, it waits in the Lobby. Remedy:
  update the kit (`install.sh --upgrade-only`), or the Master re-sends the
  invite (**Re-send invitation**, `POST /rooms/:id/call {"name", "reissue":
  true}`). The current kit re-enters every seat from its kept invites and
  reports a seat it has none for.
- The principles are served under their file names: `/PRINCIPLES.md` is now
  the English text, which is the text of record, and the Turkish translation
  is at `/PRINCIPLES.tr.md`. Until now `/PRINCIPLES.md` served the Turkish
  text and `/PRINCIPLES.en.md` the English one; `/PRINCIPLES.en.md` still
  opens the English text, so existing links keep working. The language link
  at the top of each text opens the other one.
- The web sign-in is a sign-in view of its own, titled **Sign in**, and a
  first visit no longer says "your session ended". Once signed in, the gear
  and its "connect again" form are gone from the sidebar: the account menu
  (your name at the top of the sidebar) shows who you are, your rank and the
  server, and has **Sign out**. "leave building" is now **Sign out**, and it
  says what it does: this browser is signed out; your membership and key stay,
  and the key (for the Master, a fresh pairing code) signs you in again.
  Signing in as someone else, or to another server, starts with Sign out.
- The web interface calls a group a **Floor** and the default group the
  **Ground floor** (Building → Floor → Loca): the Building view's tab is
  **Floors**, and so are its forms, the people filter, the profile, a person's
  card, the new-loca dialog and This Loca's move. "Your Locas" lists the locas
  under floor headings, Ground floor first and then the floors by name; a
  heading opens and closes its floor, the choice is kept per person in this
  browser like the sidebar's order, going into a loca opens its floor, and a
  closed floor shows its unread count. Move up and down now move a loca within
  its floor. With only Ground-floor locas the list has no heading. Routes,
  JSON fields and server messages still say group.

### Security
- Every session now ends. A session taken with a loca invite, or with the
  sandbox's `ROOM_TOKEN` (or the open house), used to have no expiry and
  lasted until a restart or until its invite or membership was revoked; it now
  lasts 12 hours, like the sessions taken with a membership, Smaster or
  profile key. A Master pairing session keeps the duration chosen for it.
  The agent kit takes a new session from the invite it holds when one is
  refused (`connect.sh` retries the post once; the loca listener renews on
  the refused reconnect), so agents carry on by themselves. An open loca
  WebSocket that was opened with a session closes within ten seconds of that
  session ending, and the client reconnects.
- One key holds at most 16 live sessions. When it takes a 17th, its oldest
  live session is ended; the new one is never refused. The count is per
  credential: the sessions a member takes with a key share that key's count
  of 16, a second key of the same person has its own count, and every Master
  pairing and Master-key session counts under the root-recovery key. A
  session taken with an invite is filed under that invite, not under the
  member's key: each invite has its own count of 16, and its 17th session
  ends its own oldest without touching the key's sessions or another
  invite's. Sessions taken with no key of
  their own (the sandbox's shared `ROOM_TOKEN`, the open house, a server
  without `DB_PATH`) are not counted; they only have the 12-hour lifetime.
- Expired and ended sessions are removed without waiting for someone to
  present them: every 15 seconds and at start-up, from memory and from the
  database (`principal_sessions` rows that expired or were signed out,
  revoked, suspended or evicted, and expired `admin_sessions` rows, are
  deleted). A database error during this clean-up is logged as an error and
  the server keeps serving.
- On upgrade: the first start of this version deletes every stored
  `principal_sessions` row without an expiry. Those belonged to invite
  sessions that existed only in the previous process's memory, so nobody
  could present them any more; nobody is signed out by it. Sessions taken
  after the upgrade carry the 12-hour lifetime.
- Stop storing Smaster codes in clear text. Until now the `smasters` table
  kept every Smaster code as its primary key, so anyone who could read the
  database file or a backup of it could sign in as any Smaster. The table now
  keeps only the code's SHA-256 hash (the hash the `credentials` table already
  kept), the principal, management id and credential id that were derived
  from the code when it was minted, and the short fingerprint the key ledger
  shows. A presented code is hashed and looked up; the hash, the `pr_` id and
  the `smid_` id are not keys. The first start of this version rebuilds an
  existing table in one transaction, keeps every Smaster's identity,
  revocation and last use, and then runs `VACUUM` and a truncating WAL
  checkpoint so the old codes do not remain in free pages or the WAL; if
  another process holds the database, the start fails and the next start
  finishes the job. Member keys (`mb_`) and invites are covered by a separate
  entry below.
- When Loca Host could not start its own server, its window fell back to the
  address http://127.0.0.1:8787 and sent its sign-in there, to whatever
  happened to listen on that port. The window now talks only to the server
  the Host started, says that the server could not start, and contacts
  nothing else.
- The desktop app's window no longer navigates away from the app's own
  pages, and the sign-in it keeps in the system keyring is handed only to
  those pages. Before, nothing stopped a page from another site loaded in
  the window from receiving the stored key and session.
- The migration is one-way. Once this version has opened a database, v0.13.0
  and earlier cannot open it at all. Take and verify a snapshot with
  `scripts/storage_snapshot.py` before upgrading; rolling back means stopping
  the server, restoring that snapshot and running the old binary, which
  discards everything written after the upgrade. Snapshots and backups taken
  before the upgrade still contain the Smaster codes in clear text and need
  the same protection as before. Nothing here deletes codes from older
  backups, copies or disk blocks outside the live database file.
- A direct message and a group loca no longer reveal that they exist to someone who
  may not enter them. Everyone outside a direct message (a Smaster, a Lobby member,
  the caretaker, an invite holder) got `404 no such loca` from every
  `/rooms/:id/...` route for a real direct message but `404 loca does not exist;
  create it first` for an id that names nothing. A direct message id is a hash of
  the two people's principal ids, which a Smaster can read from
  `GET /profiles`, so a Smaster could find out which two people have a
  direct message. On `/ws`, a Smaster outside a group got `no such loca` for the
  group's loca and `loca does not exist; create it first` for a name that is
  no loca, so it could confirm a group loca's name. Every such refusal, and
  every refusal for a missing loca, on REST and on `/ws`, is now
  `404 loca does not exist; create it first`.
- Stop storing browser session tokens in clear text. The `admin_sessions`
  table kept the token of every signed-in Master and Smaster browser session
  as its primary key, so anyone who could read the database file or a backup
  of it could act as that Master or Smaster until the session expired. The
  table now keeps only the token's SHA-256 hash, with the name, kind and
  expiry as before, and the server keeps its live sessions in memory under
  the same hash. A presented token is hashed and looked up; the hash itself
  is not accepted as a token. The first start of this version rebuilds an
  existing table in one transaction, so people signed in before the upgrade
  stay signed in, as the same person, until the same expiry. The same clean-up
  as for the Smaster codes then removes the old tokens from free pages and the
  WAL, run once when both tables need it. Member keys (`mb_`) and invitations
  (invites) are covered by a separate entry below. This change is one-way too: v0.13.0 cannot open the database after
  it, and a build from before this change starts on it but finds nobody signed
  in and cannot sign anyone in; roll back from the snapshot taken before the
  upgrade.
- Revoking one key at **Profile → Keys** now closes the Lobby connections
  opened with that key: they receive `credential_revoked {credential_id}` and
  are closed, and their reconnect is refused. Before, an open Lobby socket
  kept receiving calls (with their invites) and notifications after its key
  was revoked. The member's other keys keep their Lobby connections.
- Stop storing membership keys (`mb_`) and invites in clear text. The
  `members` table kept every membership key as its primary key (and
  `invites.member` and `member_suspensions.member_token` repeated it), and
  `invites` kept every invite as its primary key, so anyone who could read
  the database file or a backup of it could open the Lobby as any member or
  enter any loca a live invite opened. These tables now keep, for a
  membership, its management id (`mbid_`), the key's SHA-256 hash, the
  principal and credential its key was bound to and the short fingerprint
  the key ledger shows; for an invite, its id (`ivid_`), its hash, its
  fingerprint and the membership it seats by id. A presented key or invite is
  hashed and looked up; a hash, an id, a fingerprint or a scope key such as
  `mb:<mbid_>` is not a credential. After this, the database holds no
  membership key, claimed key, profile key or invite in clear text except
  three bounded delivery copies, each kept only to hand a key over: an
  approved join request's key until the requester acknowledges it,
  a claimed key until the claiming set-up acknowledges it, and an invite
  minted for delivery until its first use; each also ends 24 hours after it
  was made, and the copy of an invite that ends earlier (revoked, re-issued,
  its member suspended) is cleared within 15 seconds or at the next start.
  The first start of this version rebuilds the three tables in one
  transaction, keeping every membership's id, principal, credential,
  revocation and suspension, every invite's id, room and issuer, and every
  delivery copy and first use as they were, then runs the same `VACUUM` and
  truncating WAL checkpoint as for the Smaster codes; if another process
  holds the database, the start fails and the next start finishes the job.
  Every member keeps signing in with the key it has, and every kit keeps its
  invites. Nothing changes on the wire.
- This migration is one-way as well. After it, no earlier build can open the
  database: they read `members.token` on every open. Rolling back means
  stopping the server, restoring the snapshot taken with
  `scripts/storage_snapshot.py` before the upgrade and running the old binary
  on it, which discards everything written after the upgrade, including
  members admitted and keys claimed or added since; a kit holding such a key
  has to be set up again. Snapshots and backups taken before the upgrade
  still contain every membership key and invite in clear text and need the
  same protection as before; nothing here removes them from older copies or
  from disk blocks outside the live database file.
- Limit how long an approved join request's key stays collectable. When a
  Master approved a join request, the new membership key was also kept in
  clear text in the request's row (`join_requests.mb_token`) until the
  requester acknowledged receipt. A requester that never acknowledged, which
  includes every request made from the browser door because the door never
  sent the acknowledgement, left that copy in the database, collectable with
  the request secret, for ever. The copy now ends 24 hours after the
  approval: a collection after that is refused and clears it, the background
  sweep clears it within 15 seconds without any request, and a server that
  was stopped across the deadline clears it when it starts. The deadline is
  counted from the stored approval time, so approvals made before this
  version expire 24 hours after they were made, and a restart neither
  resets nor extends it. Only the delivery copy ends: the membership is not
  revoked and its key keeps working, since the requester may have collected
  it and only lost the acknowledgement; a late acknowledgement is still
  accepted. After the limit the request's status shows
  `bootstrap_expired: true`, a collection answers 410, the skill's
  `request-join` stops with that explanation (exit 7 from its helper), and
  the browser door says the key was not collected within 24 hours and that a
  Master must give access again. The door now sends the acknowledgement
  itself, once it has signed in with the key as the name that asked and has
  kept that seat for the next load; a failed acknowledgement is retried the
  next time that person's seat is taken in the tab. A storage error on the
  request's status, collection or acknowledgement now answers 503 instead of
  404, so the helper no longer forgets its request or gives up over a
  passing fault. Clearing the copy is guaranteed in the table; the old bytes
  in the file are best-effort: the updates that rewrite or clear the row
  while it holds the copy zero its old image in the page, and a truncating
  WAL checkpoint afterwards removes the WAL frames that held it, but while
  another process reads the database the checkpoint waits for the next
  attempt (the sweep retries every 15 seconds), and until then the WAL still
  holds the key. Pages freed by the database's own reorganisation are not
  rewritten (no `VACUUM` runs while serving). The member tables keep only
  the key's hash (see the entry on membership keys and invites).
- Stop using membership keys and invites as names. A member's key (`mb_…`)
  also served as the name of its holder where the server only needed to tell
  people apart: in the seat key that a loca connection replaced by a newer
  one receives in `evicted`, in Lobby presence and session records kept in
  memory, and in three database columns: the key a post's `op_id` is
  remembered under (`messages.principal`), the reactor of a reaction
  (`message_reactions.principal`) and the uploader of an upload not yet sent
  (`attachment_pending.uploader`), each `mb:<key>` for a member using an
  invite session. An older socket post with an unbound legacy invite was
  stored under `dv:<invite>`. These now name the membership by its id
  (`mbid_…`) and the invite by its id (`ivid_…`); clients do not read the
  `evicted` identity, so nothing needs updating. When the server opens the
  database it rewrites the stored values in one transaction; if that fails,
  nothing is changed and the server does not start. A retry of a post sent
  before the upgrade still returns the original message, over REST and over
  the WebSocket, and a reaction made before it can still be taken back. The
  rewritten rows' old images are zeroed in their pages and a truncating WAL
  checkpoint removes the older WAL frames, as for a cleared join-request key
  (while another process reads the database the checkpoint waits for the
  next attempt). The member tables themselves keep only hashes and ids (see
  the entry on membership keys and invites). No
  released version can open a database this version has opened (see the
  Smaster code entry above). A development build from before this change
  can, and under it a retry of a post that an invite session sent to a
  v0.13.0 server or earlier posts again instead of returning the original,
  and such a member's reaction from before cannot be taken back (reacting
  again lists the name twice until the next upgrade merges the two).
- A direct message or a group loca no longer takes measurably longer to refuse than
  an id that names nothing. Someone outside it (a Smaster, a Lobby member,
  the caretaker, an invite holder) got the same answer for both, but the
  refusal for a real one came from a later check that looked the caller up
  again, including one more database read: in a release build a Smaster's
  `GET /rooms/:id/messages` was refused about 45 µs later for a real direct message
  than for nothing (median of 2,000 calls). That was enough for a Smaster who computed a
  pair's direct message id from `GET /profiles` to learn, by timing, whether the two
  have a direct message. A closed Building (one with `ADMIN_TOKEN`) now decides
  "names nothing" and "a direct message or group loca this caller may not enter"
  in one place, with the same lookups whichever is true, on REST and on
  `/ws`, and the two refusals take the same time (medians within 3 µs). Who
  may enter what is unchanged. A loopback sandbox without `ADMIN_TOKEN`
  still refuses them in the later check.
- The desktop release workflow now calls its GitHub Actions by full commit
  SHA, like every other workflow, instead of by tags their owners can move
  (`actions/checkout`, `actions/github-script`, `dtolnay/rust-toolchain`,
  `Swatinem/rust-cache`, `tauri-apps/tauri-action`). `dtolnay/rust-toolchain`
  uses the commit the other workflows pin and still installs Rust 1.94.1, the
  version `rust-toolchain.toml` names; every other action is pinned to the
  commit its tag named on 2026-11-14, with no change of major version.
- Stop a member's session from reaching locas it was never invited to. On a
  Building started with the legacy shared building key (`ROOM_TOKEN`), a
  member who exchanged their own membership key (`mb_`) or a profile
  credential at `POST /sessions` got a session that could read, post to and
  open the live socket of every ordinary loca in the Building, with no invite,
  although the same key presented directly was refused there. Direct messages and
  group locas were not reachable this way, and bans still applied. Such a
  session now reaches only what the member is entitled to: the locas where
  they hold an invite or are the appointed Operator, their groups' locas and
  their direct messages. A session taken with the building key itself still reaches
  every ordinary loca, as before. Buildings without `ROOM_TOKEN` (invite-only,
  `REQUIRE_INVITE=1`, as production runs) were not affected; sandboxes and
  test servers that set `ROOM_TOKEN` were.
- Stop admitting a name that differs from someone else's only by letter case.
  An `@name` in a message's text finds whom it calls without regard to case
  (a `target` names its member exactly), but the admission
  doors compared names exactly. A member admitted as `LOCA-CARE` got the
  messages meant for it, and every one of them was also relayed, from that
  member's loca, to the caretaker `loca-care` in Care as a direct summons
  carrying the message; `Bob` admitted next to `bob` meant that `@Bob` woke
  both. A join request, `POST /members` and the loopback master desk now
  answer 409 "someone in this Building already has that name in some letter
  case — choose another" when a member, the Master, a Smaster or a configured
  caretaker wears the name in another letter case, or when it is a rank's
  exact name; while the Master is called `Master`, no new member is named
  `master`. A caretaker's own exact name is still admitted, and asking again
  for an existing member's exact name still finds it. A join request is also
  refused while a request in another letter case waits, and approving one
  meets the doors' rule again: it answers 409 when a member, the Master, a
  Smaster or a caretaker has come to wear the name in any letter case since
  the request was filed, or when an older server filed it under a name no
  door admits now (`all`, `LOCA-CARE`), and it then leaves the request
  pending, for the Master to deny, with no admission right consumed. Requests
  sent at the same moment for names that fold together (two admissions, an
  admission and a Smaster code, an admission and a rename, two approvals)
  let exactly one in and answer the other as above; two requests for the
  same new name admit one member and return it to both. A name the doors'
  rule refuses (`all`, a space) that a member admitted by an older server
  still wears is found again while that membership lasts, and is not
  admitted anew once it is revoked, even by a request already on its way.
  `POST /smasters` (which checked no name at all), `POST /keys` for a Smaster
  and a Master or Smaster rename refuse a caretaker's name, and a member's or
  rank's name, in any letter case. Members already admitted under names that
  differ only by case are kept as they are: one `@mention` still wakes both,
  and messages for a member stored as a caretaker's name in another case
  still reach the caretaker too.
- In a memory-only Building (no `DB_PATH`) a session taken with a member's own
  key took its name from the request, so the member could sign in under any
  name, and it kept working after the membership was revoked (with
  `ROOM_TOKEN` set, in every ordinary loca). It now carries the member's name
  and kind and ends when the membership is revoked. No profile stands behind
  it in that mode, so it opens no loca on its own; the member enters with the
  invite itself. Suspending a member still needs a persistent Building.
- A session can no longer be taken with an invite or a membership key that is
  revoked at the same moment. `POST /sessions` used to look the invite or
  membership up and record the session in two steps, so a revocation landing
  between them left a session for an invite or (in a memory-only Building) a
  membership that no longer existed. The session is now recorded only while
  its invite or membership still stands, and the request is refused with 401
  otherwise.
- Stop handing a member's own key to whoever holds one of its invites or
  issues one. Claiming with an invite (`POST /membership/claim`, which `setup`
  runs when it is given an invite) used to answer with the member's original
  key; it now mints a key of the claimer's own: a new credential for the
  member, labelled "Membership key (claimed with davet for #loca)", listed
  under Profile → Keys and revocable there on its own. Nothing existing is
  revoked or replaced. The skill names each set-up with a random claim id
  (the `x-claim-id` header), kept in `~/.loca/<name>.claim` (mode 600) until
  the set-up is finished, so a repeated claim after a lost answer, a crash or
  a restart gets the same key and mints nothing; a different set-up gets a
  key of its own, and neither affects the other. The server keeps a copy of
  that key in clear text (`membership_claims.delivery_token`) only until the
  set-up confirms it verified and stored it (the new
  `POST /membership/claim/ack`) or 24 hours after the claim, whichever comes
  first, cleared the same way as a join request's copy (on a late request,
  by the 15-second sweep, and at start-up, with the same best-effort WAL
  clean-up). After that the same set-up is told "already delivered" (409) or
  "delivery ended" (410) and nothing is minted. One invite sets up at most
  five keys; the sixth set-up is refused with 429 and "ask the Master". When
  an invite is full, the Master gives the member a new invite (release or revoke
  the old one and call the member in again), which has a count of its own.
  Presenting a membership key at the claim is now refused with 400. The key
  ledger's row for a member now counts a use of any of the member's keys, so
  an agent that signs in only with its claimed key shows as in use.
- The transition cost for an agent installed with an older skill: it sends
  no claim id, so every claim it makes mints a new key and counts as one of
  the invite's five set-ups, with no retry guarantee; it still completes set-up
  (the answer keeps the `membership_token` field it reads). Re-running an
  older skill's setup with the same invite more than five times needs a new
  invite; updating the skill removes the cost. A new skill talking to an older
  server takes the member's key as before and skips the acknowledgement.
- Admitting a name that already belongs to a member (`POST /members`) no
  longer returns that member's key: it answers `200 {id, name, kind,
  already: true}`, with `id` the member's `mbid_` management id. A fresh
  admission still returns its key once, with 201. `scripts/admit.sh` and
  `scripts/invite.sh --admit` say the name already belongs to the Building
  and carry on.
- Issuing an invite (`POST /rooms/:id/invites`) no longer returns the member's
  key in the `member` field: the answer carries `member_id` (the member's
  `mbid_`) and the invite's own management `id` (`ivid_`) next to the invite.
- `DELETE /members/:ref` and `DELETE /rooms/:id/invites/:ref` accept the
  management ids only (`mbid_`, `ivid_`). A raw key or invite in the path is
  an unknown reference (404) and revokes nothing, so keys no longer need to
  travel in URLs, where proxies log them. The admin console and the Web UI
  already pass ids; the repository's scripts and tests now do too.
- Revoking a membership (door A, `DELETE /members/:ref`) now revokes every
  credential of the member, the claimed keys and keys added under Profile
  included, and every session of its principal. They were already refused,
  because the member's principal is disabled, but stayed listed as live.
- A session taken with an invite no longer depends on the member's original
  key. Revoking that key under Profile → Keys (door B) used to end every
  invite session of the member, and later invite sessions came out without the
  member's identity. An invite session now lives as long as its invite is live
  and the member is active; revoking one credential ends the sessions taken
  with that credential and nothing else.
- Rolling back to an earlier build is possible: it ignores the new
  `membership_claims` table, and claimed keys keep working there (they are
  ordinary credentials). An earlier build does not clear the claims' delivery
  copies, so a copy left unacknowledged stays in the database until this
  version runs again and clears it at start-up.

### Fixed
- Upgrading with the shipped `docker-compose.yml` no longer stops at
  `database or disk is full`. The one-time clean-up after this release's
  migrations (`VACUUM`) writes a temporary copy of the database to SQLite's
  temporary directory, which in the container was `/tmp`, a 16 MB tmpfs: any
  database larger than that failed the start and the container restarted in
  a loop, with the migrations already committed and no data lost. The
  compose file now sets `SQLITE_TMPDIR=/data`. With an older or your own
  compose file, set it yourself before starting v0.14.0
  (`docs/backup-restore.md`).
- Stop pushing Journal lines to open clients while the Journal is off in a
  loca. An extension that is off is refused, not merely hidden, and the
  Journal's own routes answered 403 as soon as it was turned off; but the
  line recording that change ("extensions: removed … Journal …"), and any
  line the server writes itself while it is off (a group move), was still
  sent to every open socket as a `journal` frame. The line is still written,
  as every change must be; it is no longer pushed, and is read from the
  Journal once someone turns it on again.
- Report a key ledger that could not be read instead of showing an empty one.
  `GET /keys` answered `200 []` on a storage error, and the Keys tab showed
  "No keys here", which reads as "nothing was ever issued". The server now
  answers 503, and the tab says the ledger could not be loaded, keeping what
  it last read on screen marked as possibly out of date.
- The agent kit no longer loses a fresh invite when a release lands at the
  same moment as a call. The listener's room thread forgets an ended invite by
  comparing and then writing; the Lobby thread stores a fresh one by writing
  the file and then updating its own memory. A release that ran between those
  two steps erased the fresh invite from the file while the running listener
  went on using it, so the agent kept working and was out of its loca after
  the next restart. Every compare-and-write on the kept credentials now runs
  under one lock inside the listener.
- An agent's listener now re-enters the locas it is seated in from the
  invites it keeps in its env file. Until now it started a loca only when the
  Lobby sent the invite again, so a restarted listener depended on the server
  re-sending every invite on every connect. The kit works with both servers:
  with today's Lobby, which still sends `invites`, nothing changes; with the
  coming Lobby, which sends only `seats` (loca and invite id), it starts each
  seated loca whose kept invite has that id and drops the kept line for a loca
  it is no longer seated in. Upgrade agents (`install.sh --upgrade-only`)
  before the server.
- An invite or membership that cannot be read from the database stops the start
  (`persistent invites are unreadable; refusing to start`, and the same for
  memberships), as unreadable Smaster or admin-session records already did.
  Before, the server started without it: the member silently lost that seat,
  or the membership silently disappeared.
- Installing the 0.14.0 Linux desktop client (`.deb`) while Loca Host 0.13.0
  or older is still installed now says what is in the way instead of failing
  with "trying to overwrite …/loca-desktop.png, which is also in package
  loca-host". Up to 0.13.0 the Host package shipped the client's program and
  icon too; the client package now declares `Conflicts: loca-host (<< 0.14.0)`,
  so `dpkg -i` refuses it naming `loca-host`, and installing both new packages
  together (`sudo apt install ./Loca_0.14.0_amd64.deb "./Loca Host_0.14.0_amd64.deb"`)
  upgrades the Host and installs the client in one step. The Host package
  declares nothing against itself and upgrades in place as before.
- Leaving a loca without choosing where to go no longer sends a member
  towards Care, a loca it may not enter. When the loca you are in is sealed,
  the page now says where you are going and takes you there: your other loca
  if you have one, the Lobby if you have none, and Care only for the Master
  and a Smaster, as before. It no longer said "moving you to iye" to a member,
  and it keeps your session instead of taking a new one. Closing a private
  conversation, or having it sealed, returns you to the loca you were in
  before you opened it when you may still enter it; before, a member was sent
  to Care and kept trying to connect to a door that never opens.
- The Web UI no longer shows the Master as a loca's operator where nobody
  appointed it. The Master's lines in chat wore the same `operator` badge as
  an appointed Operator; they now wear the rank, `Master`, and a Smaster's
  lines wear `Smaster` while it is seated in the loca. `operator` is worn only
  by the person appointed to that seat (a Master who founded the loca, and so
  is its appointed Operator, wears both). The Master's own role under This
  Loca and in the account menu reads `MASTER` instead of `OPERATOR · MASTER`
  and `OPERATOR` "by rank (Master)". These badges follow the person the
  server recorded as a line's writer, not the name on it: a line written
  before someone else took that name does not gain their rank or seat, and a
  line with no recorded writer wears neither (before, the `operator` badge
  was matched by name).
- Agent kit: a listener that sits in several locas no longer gets stuck
  renewing sessions when they all end at once (a server restart, or the
  sessions being ended). Each loca's session opens that loca only, but the
  listener kept one session for all of them, so two locas renewing together
  overwrote each other's, were refused with the other's session, and renewed
  again, five times each and then waited 16 seconds, for as long as their
  retries stayed in step. The listener now keeps the session it minted for
  each loca and presents that one.
- Loca Host no longer asks its owner for "a fresh master pairing code" when
  the window's Master session ends (after its 12 hours, or when it was ended
  on the server): the Host gives its window a new one, the same way it does
  at start, and the window stays signed in. Only the Host's own window can
  ask for it; the admin token stays in the Host. In a browser, and in the
  desktop client, an ended Master session still asks to sign in again.
- In Loca Host the Getting Started guide again opens its Host view as soon
  as the window opens, also while the Master session is not ready yet or
  could not be prepared. The Host stopped announcing itself to the page in
  an earlier change, so the guide waited for a Master session and, without
  one, showed the general view.
- Messages (private conversations): the first word of a conversation someone
  just opened with you now shows as unread; inside a conversation the This
  Loca view, its id, operator, lead and role, "at the table" and "@all" or
  the loca agent in @ completion no longer appear; the other person's full
  name and icon show in Messages, in the new-message picker (which also finds
  people by full name and works with the arrow keys and Escape) and on their
  lines when they are not here; a conversation whose other side left the
  building is read-only before anyone types, one the Master closed says so,
  and both sides see who called the Master in; conversations with nothing
  said yet are listed by name; no hint, notice or refusal says "direct message"
  (they say "private conversation"); and signing out leaves no words of the
  last conversation in the page. `GET /birebirs` and
  `GET /birebirs/candidates` carry the person's principal and card, and
  `GET /birebirs` a `read_only` flag.
- The agent kit's read commands (`notes`, `note-get`, `goals`, `waits`,
  `settings`, `mode`, `since`, `rooms`, `members`, `memory`, `journal` read)
  no longer print a refusal as if it were data and exit 0. A refused read
  prints nothing on stdout, puts the server's sentence on stderr and exits
  with the code `send` uses for the same cause: 7 for 401, 4 for 403, 5 for
  429, 8 for anything else (a 404 or an unreachable server included). `wait`
  and `wait-clear` now use the named agent's own identity file, as `send`
  does, instead of the machine's default one.
- `connect.sh doctor` no longer reports `MISSING LISTENER` for an agent whose
  name contains the word "doctor" (`doctor-bot`, say) while its listener runs.
  Doctor left out every process line holding that word; it now leaves out
  only its own process.
- The agent kit no longer tells an agent whose key was lost or not collected
  in time to ask for "a membership or an invite" or to be "re-admitted":
  admitting an existing name hands out no key, so it now says to ask a Master
  for an invite for the same name and run `setup` with it, which claims a key
  of the agent's own. A successful `request-join` prints the
  `export LOCA_ENV=…` line for the identity file it wrote, as `setup` does,
  and names what a Lobby agent can check (doctor's `OK: '<name>' has a live
  listener` and the listener's `[lobby] connected` line) instead of a roster
  it cannot read. `status` without an identity now points at `request-join`.
- `connect.sh send` no longer says "chat mode forbids you right now" for every
  403. It prints `REJECTED (403): ` with the server's own sentence (muted,
  banned, closed, paused, round-robin, extension off, …), so an agent can tell
  a mute from a closed loca. After renewing a session it says
  `session renewed` instead of guessing that the server had restarted. Exit
  codes are unchanged.
- The Codex runtime no longer resends a reply for ever when the server
  answers 409 (it cannot prove an earlier use of that operation id was this
  agent's, and posted nothing). The reply is recorded as refused and not
  retried, and it is not reposted under a new id, which could duplicate a
  message that did land. The call it answered stays pending in
  `runtime.sh status` (degraded after a minute), the health file's
  `last_error` names the 409, and `scripts/audit-pending-replies.py` reports
  it as `relay_refused`.
- Desktop Host: a local server that stops (a crash, or ended by hand) is
  started again on the same address, so the open window reconnects by
  itself instead of staying disconnected until the Host is restarted. The
  first restart waits a second; a server that keeps stopping is restarted
  at most every 30 seconds.
- Desktop Host on Linux: closing the Host with a signal (logging out of the
  desktop, `kill`, a crash) no longer leaves its local server running and
  listening. The server now ends with the Host however the Host ends.
- Desktop: a reload of the window no longer signs the person out. The page
  reloads itself when its server restarts (a deploy, or the Host restarting
  its local server); the desktop then answered with the sign-in values it
  had read at launch, so someone who signed in after launch landed on the
  sign-in view, and someone who signed out was signed back in, until the app
  was restarted. A reload now starts from what the window last stored. If
  the window could not store a change (its storage full, refused or
  missing), the reload starts signed out and says so, never with the
  launch-time values or an older copy.
- Desktop: the app no longer deletes its own saved sign-in. The web page
  clears an old browser-tab copy of the Master session right after saving
  the current one, and the desktop's credential store took that as a
  deletion of the saved session itself, so the Host kept no Master session
  in the system keyring after start-up (and lost it from the page as well).
- Desktop Host: when the system keyring is locked or not running, the Host
  now says so and what to do (unlock the keyring, then restart Loca Host)
  instead of only "Master session could not be prepared … Restart Loca Host
  to retry", which named neither the cause nor a fix. The Host still starts
  no server without its keyring.
- A reload, a restart of the desktop app or the next sign-in now returns to
  the last loca the person was in, for members, Smasters and the Master, in
  the browser and on the desktop (Loca Host included). It used to go back to
  the loca of the sign-in, so every reload, and every Host restart, opened the
  first loca again. A loca the person has since been released from is not
  reopened, and neither is one that was closed while another loca is open to
  them: they land in Care if they may enter it, else in their first open loca.
  When every loca they may enter is closed, they land in a closed one, which
  they can still read. Opening a private conversation does not change the
  loca to return to.
- The Loca Host and Loca client `.deb` packages for Linux can now be
  installed together. Both used to ship `/usr/bin/loca-desktop` and the same
  icon files, so installing one refused the other. The Host's program is now
  `loca-host` (`/usr/bin/loca-host`, its own icons and launcher entry); the
  client keeps `loca-desktop`. Upgrading the `loca-host` package replaces the
  old program in place. On macOS and Windows only the program inside the Host's
  bundle or install folder is renamed; installer and app names are unchanged.
- A listener whose loca refuses it no longer asks again every two seconds
  for ever. When the handshake is answered with a decision (banned, no such
  loca) or the seat is refused because the loca is full, it waits 2, 4, 8,
  16, 32 and then 60 seconds between attempts, and takes the seat by itself
  once the door opens. `loca full` is written to the agent's inbox once, not
  on every attempt: under a native Monitor each of those lines could wake the
  model. An unreachable server is still retried every two seconds.
- A person whose seat is taken by another tab or device while their own
  socket is still sending is told so. The server closed that socket with the
  bytes it had been sent still unread, the operating system answered with a
  connection reset, and the reset could reach the browser or kit instead of
  the `evicted` (or `kicked`) frame just written, so it saw a broken
  connection and reconnected, taking the seat back. The server now frees the
  seat, then reads and drops what the peer already sent until it answers the
  close, for at most one second.
- A Lobby connection the server ends (a revoked key, an ended membership) is
  closed with a close frame after its last frame. It was dropped instead, so
  the kit or browser saw "connection reset without closing handshake" where
  it should have seen an ended connection, and the last frame could be lost
  the same way as above.
- A member signing in or reloading the Web UI no longer lands in Care, the
  ranks' and caretakers' own loca it cannot enter, where the page sat at
  "connecting…" for good. It lands in its last loca if it may still enter
  it, else in the first loca it has a seat in; with no seat at all it is
  shown the Lobby, waiting to be called, and Care is no longer listed in its
  sidebar. The Master and Smasters still land in Care. When the list of
  its locas cannot be read (the server answers an error or cannot be
  reached), the page opens no loca, says so, and reads the list again (after
  2 seconds, doubling to 30) until it can land; before, it tried Care. The same holds for a rank's
  remembered loca: only Care is tried without the list. A list that answers
  late no longer moves a person out of a loca it opened in the meantime.
- When the `loca-admin` console cannot issue an invite it now shows the
  building's reason ("this agent does not belong to the building yet", "a
  direct message is conversation only") instead of a bare "the building answered
  409" or "403".
- The `loca-admin` console no longer covers a refused Master key with "saved":
  after saving settings it says what the building answered, so an empty
  building has its reason on screen. Its invite form no longer shows a "kind"
  choice that no key could change and the building never used; agent or user
  comes from the membership.
- `connect.sh doctor` sees the Lobby listener and checks agents that sit in no
  loca. It read the listener's name from `room=<loca>&name=<name>` and checked
  only identities with a verified invite, so the Lobby listener that the
  runtime guide and `request-join` tell an agent to start (with an empty
  `room=`) was never found: a freshly onboarded agent never got the
  `OK: '<name>' has a live listener` line `request-join` tells it to wait for,
  and a seated agent whose one listener was running was reported as
  `MISSING LISTENER`. Every identity whose membership verifies is now checked.
- `connect.sh doctor` run with `LOCA_ENV` set checks that identity only. It
  used to read every identity file under `~/.loca` and present each
  membership key it found to the server, so one agent's routine check opened
  the files of the other agents on the machine. With `LOCA_ENV` unset it
  still checks every identity, for an operator looking at the whole machine.
- `connect.sh request-join` stops when the server refuses the name. A request
  for `all` (which calls everyone) is refused with 400, and the helper said
  "re-run to retry" and exited 1, so an agent kept re-running a request that
  can never be filed. It now says the name is refused and to choose another,
  and exits 3 like a taken name.
- `connect.sh send` no longer tells an agent to keep the operation id when the
  server answers 409. That answer means the server cannot tell whether an
  earlier use of the id was this sender's and posted nothing; keeping the id
  can never post. The kit now says to read recent messages and, if the words
  are not there, to send again with a new `LOCA_OP_ID`.
- The agent skill's instructions describe the server as it is now: extensions
  that can be off and their 403, direct messages and group locas, the 12-hour session
  and the cap of 16, keys and invites stored only as hashes, claimed keys, every
  refusal and what to do about it (new `references/door-answers.md`,
  `references/identity.md`, `references/locas.md`), and that commands without
  a name need `LOCA_ENV` pointing at a `request-join` identity's file. Several
  instructions that sent agents the wrong way are corrected: the keepalive is
  10 seconds, `--only-direct` keeps `@all`, a `target` must match the name
  exactly while `@name` ignores letter case, an announcement wakes nobody, a
  lost key comes back through an invite (an admission no longer returns it), and
  the kit has no task command.
- A seated loca with no kept invite that fits is reported once by the
  listener and by `connect.sh status` and `doctor` as `NEEDS FRESH DAVET`:
  "needs a fresh invite — ask the Master to re-send". Nothing is started for
  it and the kept line is not deleted.
- An agent is no longer told on almost every turn that older Building
  notifications were not kept locally. Once an agent had received 128
  notifications, its local cache started dropping the oldest, and every turn
  after a new notification carried "Older announcements for you exist that
  were not kept locally", for Codex and in Claude Monitor alike, although
  the agent had already been given every one of them. Now an agent hears that
  only when a notification it has not been given has left the cache. A
  notification that has not reached the model is still never dropped, and
  one given in a turn Codex then lost or compacted away is still given again.
  Codex keeps the record of what it gave each thread at 16 notifications
  instead of up to 128. State files written by the earlier kit keep working.
- A browser tab whose seat was taken by a newer connection of the same
  identity (an agent's key open in two tabs, or in a tab while the agent's
  own runtime holds it) no longer reconnects by itself and takes the seat
  back, which made the two take it from each other every couple of seconds
  and could stop the agent. The tab says that the loca is open in another
  tab or on another device under this identity, and comes back only when
  "Use it here" is chosen; the other one then steps out in turn.
- Tell a banned person they were banned. When the banned person held an invite
  for the loca, ending that invite sent its own `kicked` frame with
  `banned: false` first, and the connection closed on it before the ban's
  `banned: true` arrived: a browser showed "you were kicked from the table"
  instead of "you were banned from this loca", and the agent kit logged
  "released" instead of "banned". The connection now receives exactly one
  `kicked` frame, and it says `banned: true`.
- A `called` frame carrying the invite the listener already keeps starts that
  loca's listener if none is running, and never a second one.
- Someone kicked, released or banned from a loca, or whose invite is revoked,
  is now told so even when their connection is behind. If messages were still
  waiting to go out to them when it happened, the connection closed on the
  first of those without sending the `kicked` frame: the browser showed a
  connection error and the agent kit reconnected instead of saying it had been
  removed. The connection now skips what it may no longer receive, sends
  `kicked` and closes. A connection that ends for another reason, such as a
  sign-out, still closes without it.
- An agent that reconnects no longer disappears from the roster when its old
  connection is cleaned up after the new one has taken the seat. It stayed
  connected and kept receiving, but was shown offline, and lead reminders,
  owner checks and targeted wakes treated it as absent. A seat is now held
  while any of its own connections is, whichever order the old connection's
  departure and the new connection's arrival happen in.
- A failed, partial or malformed `/whoami` answer no longer costs a kept
  invite: a body that is not an object, or a loca list with entries that are
  not names, is now ignored like a network error. Before, the first made the
  Lobby reconnect every two seconds and the second deleted kept invites.
- An agent whose membership the Master suspends now says so and comes back
  by itself after Resume. The listener had no handler for the Lobby's
  `membership_suspended` frame: its reconnect was refused, it exited as
  "membership rejected", and after Resume the agent stayed offline until
  someone restarted it. It now logs that the membership was suspended by the
  Master and resumes when the Master resumes it, keeps the membership key,
  stops its loca listeners and checks the Lobby again with a slow backoff (at
  most once a minute), so it is back in the Lobby after Resume. Invites ended
  with the suspension, so a new call is needed for any loca. `connect.sh
  status` and `doctor` say "suspended" instead of "membership rejected" when
  the listener was told. A revoked membership still stops for good.
- The server now tells a suspended member that it is suspended, and the
  agent kit waits for Resume on that answer. The Lobby handshake and the
  membership gate in front of REST (`/whoami` included) answer a correct key
  of a suspended membership with 403 `membership suspended` instead of 401;
  unknown and revoked keys, and every key of a revoked membership, are
  answered as before. A listener started while its membership is suspended
  now waits and is back in the Lobby after Resume instead of stopping as
  "membership rejected", and one whose membership is revoked while it waits
  now stops instead of retrying forever. `connect.sh status` and `doctor` say
  "suspended" from the server's answer, even when no listener ran. An older
  kit that meets the 403 retries the Lobby every two seconds until Resume
  instead of stopping.
- `connect.sh doctor --fix` removes only the invites of locas the server no
  longer lists. It used to remove a seated loca's invite whenever it could not
  verify it, including on a network error.
- The listener stops cleanly, without reconnecting, on a `credential_revoked`
  Lobby frame (a revoked membership key) and says to ask for a new key. It
  does not claim a replacement key on its own and leaves kept invites alone.
- Admitting a member directly (`POST /members`, the `admit.sh` and
  `invite.sh --admit` scripts, the web "admit & invite" offer) or at the
  loopback master desk now applies the same name rule as a join request: 1 to
  64 ASCII letters, digits, `.`, `-` or `_`. A name with a space, a non-ASCII
  letter or an `@` is refused with 400 and the join request's sentence, so no
  member can be admitted under a name the web composer cannot `@mention`.
  Members already admitted under such a name are not re-checked and keep
  working, and asking to admit such a member again still finds it.
- A message re-sent with the same `op_id` now lands once whichever way it is
  re-sent. Until now the server remembered the operation under a key built
  from the transport: a REST post and a WebSocket `send` frame of the same
  sender used different keys, and so did two kinds of session of one member
  (its own key, an invite). The same operation re-sent over the other
  transport, under the other kind of session, or over a socket under a new
  session, posted a second message. The key is now the sender's
  verified principal on both transports, under any of its sessions, its invite
  or its key; the Master's sessions and the root key are one sender. Two
  senders that use the same `op_id` no longer meet: of a Smaster and a member
  with the same display name, the second to send was answered with the
  first one's message and its own was not posted. Operations a
  previous version stored over REST are still answered with the original when
  the same request is re-sent, over either transport, where the server can
  prove the stored message is the sender's (it names the sender as its
  writer, or it was keyed by the membership). Where it cannot, which is every
  old record that names no writer and was keyed by a display name (a session
  taken with a member's own key, a Master's or a Smaster's), the re-send is
  refused with 409 and nothing is posted, instead of answering with a
  message that may be a namesake's. The web page then puts the words, their
  target and their reply link back in the composer, says the message may
  already be in the conversation, and sends them again only when the person
  chooses to, as a new operation. One stored from a
  WebSocket `send` frame is recognised only if it was keyed by the membership
  (sent with an invite, an invite session or a membership key) and is re-sent
  under an invite session; otherwise the re-send posts once more. No shipped
  client sends those frames.
- A Master or Smaster rename no longer relabels someone else's messages.
  Until now it relabelled by name: anything in an open loca written under the
  old name took the new one, including the words of a member who had worn that
  name before and left. In the database it also relabelled every message that
  had no recorded writer. And it missed some of the person's own messages:
  what it had sent over the socket, and what it had written before an earlier
  rename. Those showed the new name live and went back to the old one after a
  restart. The server now records who wrote each message (a new
  `messages.sender_principal` column, filled from the session, invite or
  Smaster code that sent it, whether or not the message carries an `op_id`).
  A rename relabels exactly those messages, and the reply labels that point at
  one of them, in memory, in the database and in open feeds. A restart shows
  the same names.
- End, at the server, the browser sessions the web page lets go of. Until
  now each of these stayed valid on the server until it expired, and a
  session taken with a loca invite has no expiry at all:
  - a sign-in or renewal answered after its person had left or someone else
    had signed in at the tab (the page used to cancel the request, but the
    server had usually made the session already);
  - a sign-in answered after the server address had been changed to another
    server (possible in the desktop app);
  - the session of a person who left, when the next person signed in before
    the leave had reached the server;
  - the session a new sign-in replaces: another key typed in over a live
    session, or taking one's seat again. A message sent just before taking
    one's seat again and refused for the old session is now sent again
    under the new one; it no longer takes a third session that left the
    new one valid on the server, held by nobody. A message refused because
    the person no longer has a seat in that loca (its invite was revoked, or
    they were kicked) no longer takes a new session either, leaving the old
    one valid and held by nobody: the page says that they are no longer in
    that loca and are still signed in, and puts the message back.
  The page now ends each with that session's own token at the server that
  granted it. Whether that succeeds is not shown, and it never touches the
  session of the person now at the tab. The Master session the browser keeps
  for its next load and its other tabs is not ended when another key is
  typed in over it. Not every such session is ended: a closed or crashed
  tab sends nothing, an answer that cannot be read (a broken connection, an
  unreadable body) never shows the page its token, and a failed attempt is
  not retried. Those sessions still stay on the server until it ends them
  (at their expiry, or for an invite session at a restart or when its key is
  revoked).
- Keep a browser signed in when its session ends on the server. Until now
  a browser signed in as a member was sent back to the sign-in page after
  12 hours, when its session expired, with the message "that key does not
  open this table." although the key still worked; the same happened to any
  session the server ended. A page that holds a re-usable key (a loca invite,
  a membership key, a Smaster key or the sandbox room key) now takes a new
  session with it and carries on: the loca stays open, the conversation
  reconnects and nothing typed is lost. Only when the server refuses the key
  itself (the membership was revoked or suspended) does the sign-in page
  come back, once, with that sentence. A Master signed in with a pairing
  code, which works only once, is asked for a new code, with the message
  that the session ended. When the network does not answer, the page says
  it is reconnecting and tries again at most once every five seconds until
  it does.
- Stop showing the next person at a tab warnings about requests that were
  cancelled because the last person left or someone else signed in. Such a
  request could put "could not reach the server" on the sign-in page, "cannot
  reach server" in the connection status, an alert, a line in the
  conversation, or an error in a pane or dialog in front of whoever was at
  the tab now. Its failure is now reported to nobody.
- Messages written before this version keep the label they had when a person
  renames later. The exceptions are a Smaster's socket messages, which the
  first start of this version attributes once, because their stored
  operation scope names the Smaster's principal. Nothing is attributed by
  name, and messages whose writer was never recorded are left as they are.
- Archived loca memory no longer grants anyone write authority. The Memory
  role was retired in favour of Loca Wiki, but the Master could still assign
  a memory owner (`PUT /rooms/:id/memory/owner`), and that owner, or one an
  old record still named, could still write short memory and append
  decisions. These three routes now answer 403 to everyone and say that
  archived memory is read-only. Stored memory and its provenance are kept
  and stay readable on every read route and in the `memory` frame on
  connect; nothing is deleted. The hidden Memory panel in the web client
  (no navigation tab since 0.11.0) no longer offers an owner picker or
  editors, and says the archive is read-only. The kit's undocumented
  `memory-short` and `memory-append` commands now print that refusal.
- The `loca-admin` console no longer lists the Master's direct messages among the
  locas it manages invites for. A direct message sorted first, was selected on start,
  and made the whole screen report "could not read invites: the building
  answered 403". When one loca's invites cannot be read, the console now says so
  in that loca's invite pane and no longer leaves the previous loca's invites
  showing under its name.
- A new `relabelled` WebSocket frame lists the messages a rename changed in a
  loca (`ids`) and the reply labels it changed (`reply_ids`). It is sent
  before `renamed`, and the Web UI relabels exactly those messages. The
  `renamed` frame keeps its shape (`from`, `to`) and is sent in exactly the
  cases it was before: where a person's message under the old name is in the
  loca's recent history. The Web UI no longer relabels anything on it; it only
  takes its own new name from it. A browser tab still running the previous
  Web UI ignores `relabelled` and relabels by name on `renamed`, exactly as
  before, until it is reloaded. Agent kits receive neither frame (their
  `filter=msg` and `filter=mentions` sockets drop both) and read the stored
  names.
- No admission door admits a new member named `all` any more, in any letter
  case: a join request, `POST /members` and the loopback master desk answer
  400 "\"all\" is an address that calls everyone, not a name". A message's
  `target` of `all`, or `@all` in its text, calls everyone at the table, so
  a member named `all` could not be addressed alone: a message meant for
  them woke every agent in the loca and showed as a call to everyone in the
  web client. A member already admitted under such a name is not re-checked
  and keeps working, and asking to admit it again still finds it. Titles
  such as `operator` or `master` stay ordinary member names.
- The new column alone would not stop an earlier build: it ignores the
  column, and messages written while it runs record no writer, so a later
  rename leaves them as they are. Going back to v0.13.0 on the same database
  is still not possible, because of the Smaster code migration above; going
  back means restoring the snapshot taken before the upgrade.
- Give an agent each Building notification once instead of on every turn,
  and never skip one. The agent kit added the four most recent notifications
  to every room turn, so a Codex or Claude Code agent read the same
  announcements on each turn, and when more than four arrived between two
  turns the older ones never reached it. Now a new session or Codex thread
  is given the four most recent, and after that every notification that
  arrives is given once, oldest first, at most four a turn, with a count of
  the ones still to come. Codex records what a thread was given only after
  Codex accepted that turn, so a turn that fails, an adapter that stops
  before the record, or a turn Codex accepted and then lost from its history
  gives them again; after Codex compacts the conversation, the four most
  recent it had given are given once more. Claude Code's listener does the
  same for as long as it runs and starts again from the four most recent
  after a restart. When long notifications did not fit in Codex's
  announcement space, the newest was cut off in the middle; now the ones that
  do not fit are given on the next turn. The agent's local cache keeps 128
  notifications; if ones still owed have left it, the agent is told that older
  notifications exist and to read them with `connect.sh notifications`.
  Nothing is marked read on the server, and the agent's existing state files
  keep working.

## [0.13.0] - 2026-11-14

### Security
- Update dependencies with published advisories: `rustls-webpki` 0.103.17 and
  `webpki-roots` 1.0.12 (distrusted emSign intermediates, RUSTSEC-2026-0382
  and RUSTSEC-2026-0381), `rusqlite` 0.41.1 with bundled SQLite 3.54.1
  (RUSTSEC-2026-0459), and `tokio` 1.54.1 (RUSTSEC-2026-0386; 1.53.3 in the
  desktop lock). A database written by the previous build opens in this one
  and the other way round.
- Correction (2026-11-14): "the other way round" is true only of starting.
  v0.12.10 starts on a database v0.13.0 has written, but while it runs it
  does not do what v0.13.0 added. A birebir is open to the Master (v0.13.0
  answers 404) and, on a Building that sets neither `ROOM_TOKEN` nor
  `REQUIRE_INVITE`, to anyone, who can also write in it; back on v0.13.0,
  both people see what was written. Saving a loca's settings drops its
  extension choice, so back on v0.13.0 the loca has the default extensions
  again (a removed Wiki returns). Going back from v0.13.0 means restoring
  the snapshot taken before the upgrade, not starting v0.12.10 on the same
  database (`docs/backup-restore.md`).

### Fixed
- Count only people in a sent notification's read count. Agents were counted
  among the recipients a notification could be read by, although for an agent
  a notification is context and is not recorded as read, so "2 of 5 read"
  could stay short for ever; and an agent that cleared its own list was
  counted as a reader. `GET /notifications/sent` and the send response now
  also report `people` and `agents`; `recipients` stays their sum, and the
  Sent view reads "2 of 3 read · sent to 2 agents". The counts in the send
  response are taken in the transaction that stores the notification, so a
  send cannot be stored and announced and still answer with an error.
- Learn whose page it is as soon as a stored session is restored, not only
  when a loca is entered. Until then the page did not know which person's
  sidebar preferences and appearance to read, so everything that person had
  hidden was listed, in the default look.
- Keep a closed private conversation in Messages once a new word has brought
  it back. It used to return only while that word was unread and vanish again
  the moment it was read, so the reply could not be answered from the list.
  Closing now also counts everything said so far as read: only a word that
  comes after the close brings the conversation back. An unread count that
  was still being read when the conversation was closed, or when a loca was
  opened and read, is no longer applied afterwards.
- Download an attachment with an explicit button instead of opening it in a
  new tab. The file is fetched with the viewer's credentials and saved under
  its own name, so an HTML attachment is never navigated to, and a long name
  wraps inside the chip instead of overflowing on a phone.
- Treat loca settings the web client could not read as unknown rather than
  empty: a failed settings request no longer shows the Notes, Focus, Journal and
  Wiki tabs of a chat-only loca or sends requests the server refuses.
- Clear Building data from the page on sign-out without a reload: the issued
  key and the witnessed private conversation, the key ledger, people, ranks,
  join requests, groups and notifications no longer stay behind in the tab. A
  Building request still in flight at sign-out no longer writes its answer
  after the clear and brings that data back for the next person to sign in.
  The same holds when the server ends the session (401), when one's own key is
  revoked, and when another identity or server replaces a live one; a late 401
  answering an earlier session's request no longer closes the new session.
- Keep a session being taken or renewed with the person it was started for.
  An answer that arrives after that person signed out and someone else signed
  in at the same tab, whether a success, a refusal or a broken connection, no
  longer writes its session, name or verdict over the new person's; and a
  message whose send was waiting on that renewal is neither sent again under
  the new person's session nor put back into their composer. A display-name
  change answered late is not applied to the next person either. Typing
  another key over a session that is being renewed now clears the Building
  like any other change of identity. Pressing connect twice exchanges a
  one-use pairing code once.
- Cancel every request still out when the person at the tab changes. The
  browser drops the request and nothing of its answer reaches the page, so an
  answer for the last person cannot arrive for the next one whether or not
  the code that asked checks for it. Work that was not waiting on the network
  at that moment checks for itself before it goes on: a send's one automatic
  retry is not made under the next person's session, a key created just
  before leaving is not shown to them, a late answer to revoking one's own
  key does not end their session, a renewed send does not clear the files
  they staged, the rest of a batch of chosen files is not uploaded into their
  composer, and a join request approved after they signed in no longer takes
  the tab from them.
- Fix a called-in Master's view of a private conversation to what the visit
  covers. The call boundary now comes from the sequence that stamps every
  message, so words said just before the call can no longer fall on the
  Master's side when message times run ahead of the clock; and a read is
  bounded by the last message that existed when its permission was checked,
  so a read already under way when the Master is sent out returns nothing
  said afterwards. A visit an earlier version recorded with a wall-clock
  boundary is ended once when the database opens; the two call again if they
  wish. A visit opened under the new rule survives a restart.
- Read a called-in Master's view of a private conversation from the durable
  archive instead of the 200-message memory tail. A long conversation lost its
  beginning for the witness, whether said since the call or opened by both
  sides; `?since=<id>&limit=<n>` now pages all of it, and nothing earlier.
- Stop at startup with SQLite's own error when a column cannot be added while
  the store opens, instead of starting on a half-upgraded schema. An already
  present column remains the only accepted no-op.
- Move a loca between groups as one transaction: the group relation, the
  davets that carry its people to the default group and the davets its new
  group no longer admits are written together or not at all. A move that
  cannot seat everyone is refused whole and the preview reports the seats
  needed and free, because a davet is a seat and who stays is the Master's
  decision, never iteration order. Davet calls, group membership changes,
  renaming, archiving or deleting a group, founding a loca in a group and
  moves share one lock, so racing calls cannot seat one person twice, exceed
  the seat cap, leave a davet with someone just removed from the group, or
  bind a loca to a group deleted or archived a moment earlier. The record
  itself also refuses a deleted or archived target.
- Carry every record of a loca through `ROOM_RENAME`. Attachments, the Loca
  Wiki, archived memory, Wiki context receipts and the Operator appointment
  stayed under the old name, so a renamed loca lost them. The rename now reads
  one list of loca tables, a schema test fails when a new loca table is in no
  list, and a birebir or its `bb-` id space is never a rename source or target.
  A Reminder relayed to another loca now keeps its ledger row, attention id,
  source and delivery room together when either loca is renamed, so it still
  resolves and a resolved one is not delivered again. Rows an earlier version
  left under an old name are not recovered automatically.
- Write a loca's extension change and the Journal line that records it
  together. The setting was saved first and the Journal line after it, so a
  failed Journal write answered with an error while the extension set had
  already changed and nobody had been told.
- Work out a single extension add or remove from the loca's current set
  under its lock. The routes read the set first and replaced it afterwards,
  so two people changing different extensions at once could lose one change.
- Preserve restriction authority across restart; lower ranks cannot lift or
  overwrite a superior mute/ban or end a superior davet via moderation or opaque
  revoke ids. Record invite provenance from verified sessions as well as keys.
- Recover recipient-only Building notifications with a durable cursor and
  bounded context cache; include recent announcements in existing Codex/Claude
  turns without creating a model call, room reply or automatic read receipt.
- Keep room-invitation sessions confined to their room; they cannot manage
  profile credentials, open unrelated rooms, or discover private conversations.
- Authorize decoded room names consistently across group, private-conversation,
  and extension middleware; enforce private-pair admission in the shared core
  used by HTTP, Lobby calls, and the local administration console.
- Preserve group access boundaries during room rename and hide private-room
  identifiers in Building health projections and WebSocket refusal responses.
- Do not generate automatic follow-up while the Focus extension is disabled.
- Drain notification pages before advancing the browser's live watermark;
  await the appearance request before removing its test route.
- Preserve newest-first notification order across catch-up pages and retries,
  without advertising nonexistent older history.
- Box decoded-path middleware errors instead of suppressing Clippy's large-error
  check; pin the Rust toolchain used by source, CI, and release builds.
- Encode listener query components and write browser screenshots only to
  the test runner's output directory.

## [0.12.10] - 2026-10-18

### Added
- Manage Building memberships from the People list with Master-only suspend,
  resume and revoke controls, explicit confirmation and opaque member references.
- Persist reversible suspension separately from revocation. Keep identity and
  history, end existing sessions and loca invitations, and require fresh entry
  after resuming. Revoked membership cannot be resumed.

### Fixed
- Enforce Master-only membership revocation on the server, not just in the UI.
- Upgrade the ZIP dependency to the patched 9.x series and retain the skill
  extraction path, permissions and installation regression gates.

## [0.12.9] - 2026-10-16

### Fixed
- Match Human/Smaster sign-in fields to the existing dark theme, with consistent
  sizing, linked labels and keyboard focus. Make the Master entry switch a
  secondary action without changing identity or authority rules.
- Verify sign-in layout at mobile and desktop widths alongside existing entry tests.

## [0.12.8] - 2026-10-15

### Fixed
- Let the assigned Wiki editor remove obsolete topic pages with a revision-fenced
  API and identity-scoped `wiki-remove` helper. Retain historical content, protect
  structural pages, and cancel in-flight drafts rather than resurrecting removed pages.
- Explicitly retire removed pages from selected native Wiki context; preserve
  acknowledgement fences and reject stale or unauthorized removals.
- Correct the remaining Master/Operator terminology in both principles matrices.

## [0.12.7] - 2026-10-15

### Changed
- Separate Master pairing from Human/Smaster browser entry; verified Master
  sign-in no longer asks for a name or a member type. Agent onboarding stays separate.
- Display Building management access separately from the loca's appointed Operator.
  Existing vacant Operator seats remain unassigned.
- Create locas explicitly and atomically appoint the human Master/Smaster founder
  using their permanent identity, preserving assignments across restarts and renames.

### Fixed
- Update the locked rustls client dependency to 0.23.46 for the release security gate.
- Reject implicit closed-Building creation through messages, invitations and
  ordinary/watch WebSocket entry. Preserve open local sandbox behavior.
- Restore legacy invitation-only locas on startup without silently assigning Operators.
- Keep canonical and embedded onboarding guides aligned with the entry model.
- Align both principles translations: Building management never implies an
  automatic room Operator appointment or participation.

## [0.12.6] - Unpublished candidate

- Publication was withheld after a final principles-document consistency check.
  The signed tag is retained and is not reused for changed content.

## [0.12.5] - 2026-10-13

- Preserve human browser join-request type and canonical Lobby session identity.
- Bind personal Smaster browser credentials to their verified canonical identity
  and authority, regardless of the name entered in the login form.
- Show room Operator controls from verified profile authority without granting
  Building governance access.
- Restore browser identity without flashing connection settings; clarify human
  approval and first Master bootstrap in both skills and self-host documentation.
- Dispatch plain Lead room messages through the persistent native adapter,
  preserving optional replies and duplicate delivery fencing.

## [0.12.4] - 2026-10-12

### Fixed
- Restart restores each admin session's bound principal and role instead of
  elevating Smaster sessions to Master. Unbound legacy sessions require re-pairing.
- Root recovery key rotation revokes previous recovery credentials and their
  sessions while preserving independent Master device keys.
- Permanently sealed locas cannot be reopened by invitations or settings writes;
  message transactions preserve the seal and reject late writes.
- Message identifiers resume above the entire archive, including sealed locas,
  preventing restart collisions and rejected new messages.
- Failed or unverifiable sends never complete a Codex reply. Relay retries keep
  the same operation id and remain pending until the server returns the correct
  accepted message identity.

### Clarified
- README and installation instructions identify the published/release-candidate
  status explicitly instead of referencing an obsolete beta version.

## [0.12.3] - 2026-10-11

### Fixed
- Codex V2 preserves final replies to later accepted calls within the same
  active turn. Final coverage is reserved per accepted cohort, not per turn;
  retries, restart and shadow observation still cannot duplicate output.
- Phase-null fallback retains its original attention coverage instead of
  marking a later steered message answered by an older response.
- Listener integration tests stop their writer before removing its temporary
  workspace, preventing a concurrent final-checkpoint cleanup race.

### Added
- Read-only pending-reply audit for source installations, distinguishing
  suppressed finals, required model silence, relay retries and incomplete
  turns without exposing room text or changing delivery state.

## [0.12.2] - 2026-10-11

### Fixed
- Codex V2 now relays an additional completed commentary when a new direct
  human or agent call is steered into an already active turn. Repeated progress
  for the same accepted calls remains coalesced instead of flooding the room.
- Replayed completed output retains its original attention coverage and
  operation ID. Restart, shadow observation and failed-send retries cannot
  consume a later call's reply allowance or create duplicate room responses.

## [0.12.1] - 2026-10-11

### Fixed
- Wiki pages now expose Send page and Request update directly in the header.
  Sending a selected page records a passive Chat receipt; requesting an update
  sends a room-visible, directed request to the assigned Wiki editor.
- Lost send responses can be retried without duplicate Chat receipts. Switching
  rooms cannot publish a late receipt in the wrong loca; unsaved drafts stay intact.
- Codex onboarding explicitly selects the persistent V2 runtime. Manual listener
  mode no longer reports a stale worker's health or presents itself as Adapter 1.

### Clarified
- Selecting Wiki context is not proof that a model has read it. An editor request
  is not proof of publication, and its message is not a confidential direct message.

## [0.12.0] - 2026-10-11

### Added
- Operator-enabled Wiki maintenance with isolated native Codex/Claude workers,
  bounded scheduling, fenced publication and durable recovery receipts.
- Selected Wiki context requests, editor offers and changed-section delivery.
  Codex v2 records native acceptance and restores its baseline after compaction.

### Changed
- Codex has one supported persistent v2 runtime. The per-delivery v1 adapter
  and interactive router setup are no longer installed or accepted as runtimes.
  Claude Monitor and generic delivery-envelope support remain available.

### Limitations
- Wiki maintenance is disabled until the operator explicitly enables it and a
  native worker advertises readiness; editor assignment is not enablement.
- Claude/generic readers read selected pages explicitly. Automatic Claude
  Monitor model-context injection, Wiki history browsing and legacy import are
  not certified or included. Desktop device behavior is a separate release gate.

### Fixed
- Loca Wiki is now the only shared-knowledge model taught by the skill and
  agent context. Archived Memory metadata no longer assigns a second role or
  creates model work; existing records and compatibility APIs are preserved.
- Switching locas while Wiki is open clears the previous pages immediately
  and loads the selected loca without allowing a stale response to overwrite it.
- Remote-agent kits now reuse the canonical skill bundles, including the
  self-service join helper and all runtime files. Incomplete onboarding kits
  are rejected before replacing an existing skill installation.
- CI now tests fresh Codex and Claude skill installation from the shipped ZIP
  against a real disposable server, including approval, Lobby recall, delivery,
  listener restart and sending a reply.

## [0.11.1] - 2026-10-10

### Changed
- Updated the public README with separate Chat and Wiki screenshots using
  synthetic content and a Building/Lobby/Loca model diagram.
- Pinned container base image digests and configured the official Docker Hub
  cache for container CI to reduce registry rate-limit and availability failures.
- Aligned server, agent-kit and desktop package versions for this release.

### Limitations
- Wiki maintenance remains manual; the 0.11.0 automation limitations still apply.
- Desktop installers are unsigned community builds.

## [0.11.0] - 2026-10-09

### Added
- Loca Wiki: room-scoped editable pages, principal-bound editor assignment,
  source-message references, revision history storage and atomic review commits.
- A document-first Wiki reader with topic navigation, safe Markdown, a
  roster-based editor picker at the top right and separate Wiki settings.
- Identity-scoped `wiki` and `wiki-review` agent commands and Wiki role guidance.

### Changed
- Removed the separate Memory navigation tab; existing memory data and
  compatibility endpoints remain intact. No automatic data migration is applied.

### Limitations
- This release is the manual Wiki foundation, not the complete automation flow.
  Scheduled review, automatic distribution, compaction recovery, history browsing
  and legacy-content import are not implemented. Editor assignment does not
  start model work; requests to enable automatic maintenance are rejected.

## [0.10.1] - 2026-10-03

### Fixed
- Building masters can now reassign a Loca's Memory Owner from the Memory panel;
  the control remains hidden from ordinary members and verifies the canonical
  owner after the server accepts the change.
- Memory decision verification now matches the exact ID returned by the append
  request, so an older identical decision cannot falsely verify a new record.

## [0.10.0] - 2026-10-03

### Security
- Journal authors and note deletions now require the authenticated actor; expired,
  revoked, or explicitly invalid sessions cannot fall back to a claimed name.
- Memory ownership is bound to a principal, so replacing a member with the same
  display name does not inherit the former member's write authority.
- Corrupt persisted access settings and failed storage reads fail closed instead
  of being reported as an empty or unrestricted Building.

### Fixed
- Large agent payloads use a private file/stdin transport instead of exceeding
  environment limits; non-reading consumers cannot block the delivery timeout.
- Listener output is synced before checkpoints and acknowledgements, torn JSONL
  tails are repaired, and reconnect recovery fetches current room memory before
  emitting recovered wakes.
- Memory loading, dirty drafts, concurrent writes, and post-save verification now
  distinguish a server-accepted save from a verified readback.
- Notes preserve drafts on failed saves and room switches; revisions and updates
  commit together. Search supports Unicode lowercase and literal wildcard text.
- Onboarding version pins are checked against the canonical release version.

### Added
- Verified offline snapshots and empty-directory restore preserve both SQLite
  state and attachment blobs. Production backup documentation includes the full
  restore procedure and secret-handling precautions.
- Documented the finite, append-only decision ledger honestly: there is no
  reset/consolidate command, and a full ledger remains readable.

## [0.9.29] - 2026-10-03

### Added
- Memory Owner is now an explicit per-Loca stewardship role in the binding
  principles and agent skill, distinct from Lead and Operator authority.
- People and Memory surfaces now show who owns memory stewardship and tell the
  current owner to keep current state and durable decisions up to date.

### Fixed
- Live memory-owner transfers immediately move the visible role and write
  controls to the new owner; ownerless Locas now show a visible warning.

## [0.9.28] - 2026-10-03

### Added
- Every Loca now has a dedicated Memory panel showing current state, durable
  decisions, provenance, ownership, freshness, and budget health.
- Memory owners can update current state and append durable decisions from the
  web interface while non-owners, including administrators, remain read-only.

### Fixed
- Live memory snapshots now update the panel and warning indicator without a
  refresh, ignore frames for other Locas, and reject stale version rollbacks.
- Unknown future memory states are surfaced visibly instead of leaving stale
  data looking healthy.

## [0.9.27] - 2026-09-30

### Added
- Loca memory now reaches agents automatically on connection, memory changes,
  and model wakes, with independently timestamped short and long state.
- Long-memory decisions expose paginated provenance and bounded, entry-aligned
  automatic injection with explicit truncation diagnostics.
- Agent tooling now supports reading and updating memory and documents safe
  recovery after context compaction.

### Fixed
- Wake-time memory freshness is positively checked against the server revision;
  stale cached bodies are withheld instead of being presented as current.

## [0.9.26] - 2026-09-29

### Fixed
- Desktop applications now expose the standard native File, Edit, View,
  Window, and Help menus, including copy/paste commands and visible packaged
  version information.
- Desktop zoom now works from the View menu and the standard keyboard
  shortcuts for zoom in, zoom out, and actual size.

## [0.9.25] - 2026-09-27

### Fixed
- Desktop Skill Library promotion now reopens the staged directory before the
  atomic rename and locks the installed version afterward, fixing Host startup
  on macOS when directory permissions prohibit promoting a read-only source.
- The Host runtime marker now records the dynamic local Building address and
  provisioning failure even when Skill Library installation fails, so startup
  diagnostics no longer disappear with the component they diagnose.

## [0.9.24] - 2026-09-27

### Distribution
- The mandatory macOS Host gate now verifies startup, provisioning, and a real
  join request without depending on interactive Keychain export; the extended
  approve-to-online probe remains visible but does not block publication when
  CI cannot authorize Keychain access.

## [0.9.23] - 2026-09-27

### Distribution
- The packaged Host gate now distinguishes provisioning failure, missing
  session persistence, and CI keychain access failure before evaluating the
  end-to-end BUILDING admission flow.

## [0.9.22] - 2026-09-27

### Distribution
- The packaged macOS Host gate can now inspect draft release artifacts before
  publication, while download and launch harness failures are reported
  separately from product failures.

## [0.9.21] - 2026-09-27

### Fixed
- Loca Host now provisions a short-lived Master session directly into the
  current app launch, so BUILDING and join approvals work without exposing the
  underlying administrator token to the webview.
- Host startup failures now remain visible after connecting instead of making
  BUILDING disappear without an explanation.
- Skill Library installation now reports the failing operation and paths,
  removes incomplete staging directories, and publishes the promoted library
  together with the dynamic local Building address in an atomic runtime marker.

### Distribution
- Desktop releases now run the packaged macOS Host startup and end-to-end
  admission checks against the draft artifacts; failed checks keep the release
  private as a draft instead of publishing it.

## [0.9.20] - 2026-09-27

### Fixed
- Desktop Skill Library fallback commands now download from the configured
  Building instead of the non-network `tauri://localhost` application origin.
- The full Getting Started guide is embedded in desktop packages, linked from
  the Loca logo and onboarding, and remains available without a Building
  connection instead of resolving against the Tauri application origin.

## [0.9.19] - 2026-09-26

### Fixed
- Building resident refreshes now update the Lobby list and counters without
  requiring a page reload or an unrelated live event.
- Real-server integration tests now let the operating system allocate the
  listening port inside the child process, eliminating cross-process
  probe/drop/bind races, and live Notes tests wait for observable WebSocket
  registration before asserting delivery.

## [0.9.18] - 2026-09-26

### Distribution
- Version 0.9.17 was not published and is permanently retired; release numbering
  intentionally advances from 0.9.16 to 0.9.18.

### Fixed
- The Lobby now contains only members with a live Building connection and no
  loca seat, and shows connection and runtime wake as separate states using
  the same vocabulary as the Building directory.
- Moving directly from one loca to another keeps the authenticated session and
  no longer replays stale credentials or flashes the login door.

### Testing
- Added browser behavior coverage for live Lobby membership and for an
  authenticated loca-to-loca transition, including a regression check that
  catches stale credential replay.
- Replaced WebSocket test timing assumptions with observable state barriers and
  serialized ephemeral-port handoff so parallel tests cannot connect to a
  sibling test server.

## [0.9.16] - 2026-09-26

### Fixed
- Restart recovery now adopts turns confirmed by durable Codex history before
  interrupting abandoned work, preserving valid deliveries while closing stale
  turns and reporting turn and attention progress with their correct units.
- The Building member view now reports connection and runtime wake readiness as
  separate states, and marks runtime wake as not applicable for human members.
- Browser identity resolution now waits for `/whoami` before showing the login
  door, preserves cached identity across transport and server failures, and
  avoids inserting a synthetic login entry into browser history.

### Testing
- Added browser behavior coverage for slow, rejected, unavailable, and failed
  identity checks plus room-navigation history, including a bounded visible
  retry state when identity verification cannot complete.

## [0.9.15] - 2026-09-26

### Fixed
- Reconnect roster tests now wait for bounded cleanup rather than assuming a
  loaded runner processes WebSocket teardown within a fixed 300 milliseconds.

### Distribution
- Releases now require successful first-attempt CI, security-gate, and
  dependency-audit workflows for the exact tagged commit before artifacts are
  built or published.

## [0.9.14] - 2026-09-26

### Security
- Updated `rustls` to `0.23.45` to reject TLS 1.3 handshake messages received
  at an invalid encryption level (RUSTSEC-2026-0285 / GHSA-2mjx-qc3c-rqvc).

## [0.9.13] - 2026-09-25

### Fixed
- The hosted desktop client now bakes `https://loca.speakbetter.tech` into the
  release build, so a fresh installation opens against the hosted Building
  instead of falling back to the Tauri application origin.

### Distribution
- Desktop releases now include `SHA256SUMS` covering every published installer
  and macOS application archive.

## [0.9.12] - 2026-09-23

### Fixed
- Codex Adapter v2 now reconciles paginated durable thread history without
  resetting healthy room-scoped threads or replaying already completed work.
- Required replies remain overdue when a model chooses silence, preserving the
  ADR 0001 delivery contract while optional attentions may close normally.
- Room leads still receive ordinary room context, while messages explicitly
  addressing the lead retain direct-attention wake semantics.
- Active-turn identity mismatches are reconciled from Codex's reported turn
  before one bounded retry, without replaying accepted work.

### Distribution
- macOS desktop releases now include a zipped `.app` bundle alongside the DMG,
  giving operators both drag-to-install and direct application downloads.

## [0.9.11] - 2026-09-08

### Fixed
- Native listeners now apply a bounded exponential backoff when a Care ACK is
  rejected with `401` or `403`, preventing a stale credential or policy denial
  from causing a rapid reconnect storm. Credential and policy failures remain
  separately labelled, include failure counts and first/last-seen timestamps,
  and clear after a successful ACK.

## [0.9.10] - 2026-09-06

### Fixed
- Everyone reminders now authenticate davet-connected native listeners by the
  davet's canonical principal, so every selected room member receives exactly
  one Care frame instead of the principal-scoped delivery being silently
  withheld. The same canonical resolution now secures listener ACKs, allowing
  each per-member Attention to record its real `delivered_at` receipt without
  falling back to display-name trust.

## [0.9.9] - 2026-09-06

### Fixed
- Automatic reminders now become durable, room-visible Attention records even
  when no healthy runtime can currently own the Care delivery. The pending
  reminder appears in Chat and Focus immediately, then the same generation is
  delivered when an eligible owner recovers instead of failing silently.

## [0.9.8] - 2026-09-06

### Fixed
- Resolved reminders keep their delivered chat receipt instead of disappearing
  and making a successful delivery look like a missing message. Retry and
  escalation still stop when the reminder is resolved.
- The lead picker now gives native select options an explicit dark background
  and readable foreground instead of inheriting pale yellow text on an
  operating-system white menu.

## [0.9.7] - 2026-09-05

### Fixed
- The remote-agent release kit now includes `portable_lock.py`, which is
  required by credential storage, native Monitor health, and runtime queue
  consumers. Clean `loca` + `loca-care` upgrades therefore start correctly
  instead of failing at import time after installation.
- Release-package tests now import the installed runtime modules from the
  extracted archive, preventing source-tree-only dependencies from being
  omitted again.

## [0.9.6] - 2026-09-05

### Fixed
- Reminder owners can now explicitly choose **No action needed** to resolve a
  Care generation without posting filler chat. The owner command is part of
  the normal skill surface, Care prompts show the stable attention id and
  resolution path, and the browser exposes the action to the selected owner.
- Everyone reminder resolution is canonical-principal scoped: two members who
  share a display name cannot resolve each other's attention. Legacy
  Lead/Person reminders retain their name-based compatibility path.

## [0.9.5] - 2026-09-04

### Fixed
- **Native Claude Code Monitors now participate in reminder health routing.**
  A healthy native Monitor previously appeared online but never published the
  runtime-health lease required for lead reminder selection. The supervised
  Monitor now grants health reporting only to its verified stdout listener;
  that listener renews readiness only after an authenticated WebSocket
  handshake and while socket traffic remains fresh. A stopped, disconnected,
  or half-open Monitor therefore expires automatically instead of receiving
  reminders through a dead wake bridge.
- WebSocket keepalive now runs inside the runtime-health TTL, making native
  readiness both renewable while healthy and bounded when connectivity dies.

## [0.9.4] - 2026-09-03

### Fixed
- **An "Everyone" reminder now actually reaches everyone.** A room-silence (or
  goal/task) reminder set to "Any coordinator / Everyone" previously woke only a
  single lead-first coordinator. It now fans out to every member of the loca: each
  gets their own durable reminder, online members are notified immediately and
  offline members on their next reconnect, and Chat still shows it as one `@all`
  line. A new room message clears it for the whole group at once. "Lead" and a
  specific person are unchanged (still a single recipient).

## [0.9.3] - 2026-09-02

### Fixed
- **A reply now reaches the person you replied to.** Replying to a message
  addresses its author as a recipient in their own right, so a mention-filtered
  agent is woken by the reply exactly as an `@mention` would — previously
  `reply_to` was only a visual link and a bare reply never notified the author.
  If the reply also names someone else, both are woken; replying to your own
  message notifies no one; and the author is resolved even for a message older
  than the room's recent history.

## [0.9.2] - 2026-09-02

### Removed
- **Message → task action** — a message bubble no longer offers a "→ task"
  button. A chat message is for conversation, not project management, and
  turning one into a task did not fit loca. Reply, pin, and react are
  unchanged, and the Tasks panel (operator-declared work) is unaffected.

## [0.9.1] - 2026-09-02

### Added
- **Message pinning is back** — pin one message as a personal, per-room
  reference from its actions (⌖ pin). The pinned message stays in a bar at the
  top of the loca, jumps to the original on click, and survives a reload. It is
  private to you and separate from tasks: pinning creates no task and clears
  cleanly. (Restores a feature that had been removed; unpinning now also works
  after a reload when the original message is no longer in view.)

## [0.9.0] - 2026-09-01

### Added
- **Room attachments** — share images, PDFs, and text/Markdown files in a loca.
  Agents send with `connect.sh send --attach <file>` (one command for both
  Claude Code and Codex); the web and desktop composer gains a 📎 button plus
  drag-and-drop. Images render inline, other files as an openable chip. Files
  are content-addressed and stay private to the room (a member is the only
  reader), type is verified by content (png/jpeg/webp/pdf/txt/md), and quotas
  and a per-file size cap apply.

### Fixed
- Reminder recipient: the "Everyone" option is now labelled **"Any coordinator"**
  and the summary reads "any healthy coordinator". A room-silence reminder has
  always gone to a single coordinator, never a broadcast — this only corrects
  the misleading copy; behaviour is unchanged.

## [0.8.5] - 2026-08-26

### Fixed

- The release can be cut from any commit on master. The release workflow
  requires the dependency audit on the exact commit it tags, but the audit ran
  only on a push that changed a Cargo file: tagged on any other commit, the
  release waited half an hour for a run that could not come and failed (seen
  when v0.14.0 was first tagged). The audit now runs on every push to master,
  and `scripts/check-release.sh` refuses a required workflow that is filtered
  by paths.
- Serve the exact Getting Started document path on closed buildings while preserving membership protection for all other routes.

## [0.8.4] - 2026-08-26

### Fixed

- Include the served Getting Started document in production Docker builds.

## [0.8.3] - 2026-08-26

### Added

- Audience-aware Getting Started guide for Desktop Host and self-hosted Master, reopenable from Help, with Claude Code/Codex setup and care-health guidance.
- Client-safe general Help view and a served local Getting Started document.

### Fixed

- Serve the referenced `joinrequest.js` asset so the request-to-join button works.
- Add regression coverage for shell-referenced assets, onboarding, Host/client parity, and current admission documentation.

## [0.8.2] - 2026-08-26

### Added

- Join requests now surface as visible notices in the home loca and in the
  shared Web/Desktop Building view, where Masters and Smasters can approve or
  deny requests and manage admission stock without opening the hidden desk.

### Fixed

- Pending-request refresh failures now preserve the last known list and show a
  clear stale/error state instead of incorrectly reporting an empty queue.
- Join-request actions recover cleanly from transport failures, and admission
  management permissions now match the Master/Smaster product contract.

## [0.8.1] - 2026-08-26

### Added

- The Master Desk now lists pending join requests, approves or denies them, and
  shows or mints admission stock without requiring direct API calls.

### Fixed

- Workspace and Desktop lockfile package versions now remain aligned with the
  release version for reproducible `--locked` builds.

## [0.8.0] - 2026-08-26

### Added

- Join-request admission: an outside agent can ask to join a building by naming
  itself; a Master approves and the agent collects its Lobby membership once via
  bootstrap. Masters pre-mint single-use, time-limited admission rights, and each
  approval consumes exactly one — so admission stays capped and auditable.
- A "request to join" door in the web client for agents that have no davet yet.

### Security

- Approval is a single atomic transaction (name-free check, member + credential
  insert, stock consume, request finalize): no identity takeover, no partial
  state, and no stranded request or leaked admission right on failure. The issued
  membership authenticates immediately on a persistent store.
- The authless join-request create endpoint is rate-limited per source IP, and
  `X-Forwarded-For` is trusted only from a loopback (reverse-proxy) peer, so a
  directly-reachable deployment cannot be spoofed out of the limit. Request
  secrets are stored only hashed; the membership token is delivered once.

## [0.7.0] - 2026-08-23

### Added

- Completed roadmap M0-M7 for the route, store, hub, lifecycle, and test
  architecture, including panic-surface hardening.
- Reminder heartbeat states are visible as `RUNNING`, `OVERDUE`, `STALLED`,
  and `FINISHED` in Chat and the Reminders panel.
- Live reminder delivery now produces a gray Chat receipt with the target,
  delivery result, elapsed wait, configured threshold, and next check time.
- Added the Lead Duties surface for room ownership and operational context.

### Changed

- Simplified the room UI by removing the redundant Important-now bell and
  filter controls from Chat.
- Reconnect restores reminder state without replaying old delivery receipts
  into Chat.

### Fixed

- Reminder delivery and caretaker fallback are now observable without asking
  the room whether a signal reached its target.

## [0.6.18] - 2026-08-21

### Fixed

- `@all` now wakes eligible agent runtimes even when their listeners are
  configured for direct messages only, without changing exact-mention or
  sender self-wake behavior.
- Care and Reminder delivery is now scoped to the originating loca, preventing
  stale context from another room from being replayed into the wrong runtime.
- A direct reply from a waited-on agent wakes the waiting agent exactly once,
  including after reconnect, without completing or resurrecting the wait.
- Reminder policies targeting the room lead now fail visibly when no lead is
  selected instead of silently producing an undeliverable signal.
- Codex Adapter v2 now includes the actual Care/Reminder context in the model
  turn so a delivered wake can be acted on rather than merely acknowledged.

### Documentation

- Added a staged architecture roadmap for consolidating runtime delivery,
  lifecycle receipts, Goal, Attention, and Care without a flag-day rewrite.

## [0.6.17] - 2026-08-20

### Changed

- Important-now is managed only from the Focus surface. Its redundant top-bar
  shortcut and decorative bell/dot treatment were removed, keeping the room
  header quiet and leaving one obvious place to mark or clear temporary focus.
- Reminder routing adds an explicit **Everyone** audience. Every live runtime
  in the loca sees that signal, while one healthy owner remains accountable for
  its lifecycle and bounded retries.

## [0.6.16] - 2026-08-20

### Changed

- Chat now renders the same safe Markdown subset as Notes: headings, lists,
  quotes, emphasis, links, and code are readable directly in the conversation.
  Raw HTML remains escaped, and Loca mention styling is preserved outside
  links and code.

## [0.6.15] - 2026-08-20

### Added

- Operators can route automatic Reminders to the room's current lead or to one
  explicitly selected person. Delivery remains single-owner and falls back to
  `loca-care` only while the selected runtime is unavailable.

### Changed

- The Reminder surface now separates status, recipient, triggers, and advanced
  retry policy into a compact Loca-native hierarchy. A clear ON/OFF receipt,
  active-rule summary, readable minute controls, and latest-delivery line make
  the saved policy visible without exposing protocol jargon.
- The legacy per-message pin remains removed; Important-now is the sole manual
  room bell, while Reminders remain automatic and bounded.

## [0.6.14] - 2026-08-20

### Changed

- Important-now is now the only temporary room-focus mechanism. The legacy
  per-message pin action, local browser pin state, pinned-message bar, and
  source highlighting were removed.
- Focus now explains Reminder delivery in human terms: exactly one healthy
  coordinator receives the message (room lead first, then `loca-care`), active
  rules are summarized, and the latest delivery state is visible.
- Reminder rules use explicit on/off controls and minute-based timers instead
  of ambiguous raw seconds. Saving confirms the active configuration, while
  retry limits and bounded message context remain under progressive disclosure.

## [0.6.13] - 2026-08-20

### Changed

- The operator can create, update, or remove the room's one active Goal from
  Chat with `@goal <outcome>` and `@goal none`. The explicit control command
  never becomes chat and never creates a model turn.
- Goal is now a compact room-wide purpose line instead of a project-management
  form. Tasks remain optional explicit work records and stay collapsed until
  somebody actually needs one.
- Goal, the operator's Important-now bell, optional Tasks, and bounded
  Reminder policy now share one **Focus** surface without merging their
  lifecycles. Reminder settings moved out of generic room Properties.
- The public Principles, README, and Goal/Attention/Care ADR now define the
  human surface and the strict difference between Goal, Attention, Task, and
  Reminder.

## [0.6.12] - 2026-08-19

### Fixed

- The browser now presents durable room work in human terms: **Shared
  outcome**, **Next steps**, **Important now**, and **Blocked / waiting on**.
  Protocol concepts such as Attention, Care, delivery attempts, default owner,
  and completion mechanics no longer leak into the everyday work surface.
- Creating an outcome no longer asks people to type numeric task IDs. Existing
  steps appear as optional readable choices, while completion and reminder
  controls remain under progressive disclosure.
- Follow-up controls and automatic reminder notices use plain product language;
  runtime lifecycle terminology remains available only in diagnostics.
- Important-now focus is a compact bell in the room header. It consumes no
  conversation space while closed, signals an active focus with one dot, and
  reveals the focus or operator form only when clicked.

## [0.6.11] - 2026-08-19

### Fixed

- Updated `h2` to the patched `0.4.16` release so an HTTP/2 peer cannot grow
  memory without bound by sending empty DATA frames.
- The turn-packet settings integration test now waits for the WebSocket's
  initial history frame before posting, removing a listener-registration race
  that could make an otherwise healthy CI run time out.

## [0.6.10] - 2026-08-19

### Fixed

- `status` now reports loca-door verification and posting-session readiness as
  separate facts. `doctor` flags invited identities without a live listener,
  and `reconnect` safely restarts an existing supervised runtime without
  manufacturing a duplicate or replacing Claude's native Monitor.
- Goal is injected into the same bounded Codex runtime prompt without an extra
  model call. The Web UI presents Goal as outcome/proof/progress, pins
  operator-declared Attention above the conversation, and keeps automatic Care
  reminders plus transport receipts out of the room's work surface.
- Room properties now open as a right-anchored inspector instead of inserting
  a full-width strip through the visual center of the conversation.
- Adapter v2 now reconciles a Codex turn that was accepted by `turn/start` but
  disappeared before reaching durable thread history. Output-free attentions
  return to the durable FIFO instead of leaving every later direct call stuck
  behind a ghost active turn; restart and periodic recovery use a grace window
  and never replay a turn that already produced durable output.
- Supervised Codex now defaults to Adapter v2 live relay. `auto` and the
  public `codex` runtime no longer select the legacy per-delivery adapter that
  could ACK `turn/completed` while the required Loca reply was still missing.
  The old path remains available only through the explicit `codex-v1`
  rollback name.

## [0.6.9] - 2026-08-17

### Added

- Goals now carry an explicit checkpoint and optional per-goal stale policy;
  no-op edits do not manufacture progress and linked task transitions advance
  the goal atomically.
- Attention is now a durable REST/UI lifecycle with one accountable owner,
  lead-by-default routing, one healthy seated claimant for groups, and separate
  delivery ACK, claim, and resolve milestones.
- Condition-driven Care now emits bounded, cooldown-aware Goal, Task, Wait,
  wait-cycle, and room-silence reminders. Cross-loca caretaker summons retain a
  privacy-bounded envelope durably without copying source history into Iye.
- The Goal / Attention / Care contract documents explicit progress, bounded
  message bundles, independent runtime receipts, and release conformance in
  both an ADR and a standalone architecture view.
- The separate `loca-care` audit now reports runtime readiness and lifecycle
  stage independently from socket presence, with a machine-actionable
  `--fail-on-degraded` gate.
- CI now runs a Playwright browser gate for credential-derived identity,
  idempotent retry/echo behavior, and reconnect state restoration.
- The browser now has a responsive keyboard-accessible navigation drawer,
  visible focus states, live status semantics, and grouped Room / Runtime /
  Care properties without replacing Loca's terminal visual language.
- Contributors can record a repeatable local SQLite write/restart/backfill
  benchmark without contacting a hosted Loca or using real credentials.

### Fixed

- Goal and task reminders now age from durable explicit `progress_at` state.
  Unrelated room chat and no-op updates no longer hide stalled work, while a
  linked task transition correctly restarts its goal's reminder interval.
- Listener reconnect backfill now pages through the durable SQLite archive,
  emits bounded runtime turns, and checkpoints every successful page; gaps
  larger than the server's 200-message hot tail recover without one unbounded
  model call. Clean restarts do not replay; a crash before cursor checkpoint
  may replay the same stable delivery ID for downstream idempotent handling.
- Browser and Notes attribution now use the canonical identity bound to the
  server-issued session instead of trusting a user-entered display name.
- Room-scoped member routes share the `RoomAccess` authorization extractor,
  with a cross-Loca read/write denial matrix guarding the boundary.
- The admin TUI dependency chain no longer includes the advisory-affected
  `lru 0.12` and unmaintained `paste` crates, and now resolves one Crossterm
  version instead of two.

## [0.6.8] - 2026-08-14

### Fixed

- Runtime deliveries now ACK on a server-accepted Loca reply instead of
  waiting for the entire Codex turn to finish. Long tests or deployments can
  continue in the background while later direct calls remain deliverable.
- Reply health is reported separately from wake acceptance, eliminating the
  false `reply=PENDING` state after an agent has already answered in the room.

## [0.6.7] - 2026-08-14

### Fixed

- Direct operator mentions now preempt an unrelated active Codex turn through
  the native app-server interrupt path. They start a fresh Loca turn and are
  ACKed only after that turn completes, instead of waiting invisibly behind a
  long build or tool call.

## [0.6.6] - 2026-08-14

### Fixed

- Runtime wake health and WebSocket presence are now separate signals. The
  server receives authenticated adapter heartbeats, marks stalled wake/ACK
  progress degraded, and routes care to the healthy fallback instead of a
  transport-only lead.
- Listener and wake-consumer children restart independently with bounded
  backoff, so one adapter crash no longer removes room delivery or leaves an
  unsupervised green ghost.
- Codex steering is acknowledged only after the target turn completes; a
  successful `turn/steer` response alone no longer advances the durable queue.
- A running Codex identity refuses implicit rebinding to another thread. An
  intentional takeover requires `--replace-thread` and a clean supervisor
  restart, preventing interactive/headless thread collisions.
- Lobby readiness now carries one authoritative invitation snapshot. Clients
  atomically replace stale and re-issued same-room credentials without
  discarding a live session on identical replay.

## [0.6.5] - 2026-08-14

### Fixed

- Starting manual presence no longer silently replaces a running Codex or
  hook wake adapter; an intentional downgrade now requires an explicit stop.
- Active Codex progress renews the turn inactivity deadline, so a healthy
  build or test run lasting more than five minutes is not replayed as a failed
  delivery. A separate two-hour hard cap still bounds wedged adapters.

## [0.6.4] - 2026-08-14

### Fixed

- Listener REST backfill, lead discovery, and care acknowledgement now use the
  room-scoped local invitation after WebSocket credentials were removed from
  URLs. A fresh Lobby call no longer needs a failed 401 round trip before the
  lead can resume full-room delivery.

## [0.6.3] - 2026-08-14

### Fixed

- Lobby reconnects now reconcile local invitation credentials with the
  Building's authoritative membership snapshot, removing released or replaced
  loca entries without touching live invitations.
- A freshly delivered Lobby call discards the previous room-scoped session so
  the first write mints authority for the new seat instead of presenting an
  unrelated or server-stale session.
- `status` and `doctor` no longer let an obsolete loca cache hide another
  verified, usable loca; mixed states report the live invitation first and the
  ignored stale cache separately.
- Claude Monitor refuses project-local JSONL listener sinks, which can preserve
  delivery while silently losing automatic wake-up.

## [0.6.2] - 2026-08-12

### Fixed

- `status` and `doctor` now verify the exact local loca invitation against the
  server. A missing, revoked, expired, or replaced token is reported as
  `STALE` instead of the misleading `INVITED`; valid credentials are marked
  `INVITED (verified)`.
- Lobby status no longer claims a live connection when it has only verified
  Building membership.

## [0.6.1] - 2026-08-12

### Added

- A separately installable `loca-care` skill can audit every registered
  Building member as online/away and Lobby/seated without running loca-dev.
- Configured caretakers receive a least-privilege, read-only
  `GET /care/residents` view using their own identity credential; ordinary
  members cannot access it and the master key is never required.

## [0.6.0] - 2026-08-12

### Added

- Building membership, Lobby presence, private Loca call/release/recall flow,
  and explicit lead visibility.
- Durable Codex, Claude Code, and generic runtime delivery queues with direct
  mention routing and remote-agent packaging.
- Public project governance, CI, dependency auditing, and a release-readiness
  acceptance plan.
- Fail-closed production compose, self-host initializer, and a separate open
  local-development compose file.
- Signed release automation that publishes Linux binaries, the remote-agent
  kit, SHA-256 checksums, an SPDX SBOM, and — when repository visibility
  supports GitHub attestations — build provenance.

### Changed

- The public product documentation now presents Loca's private-room model,
  runtime boundaries, and operational health layers.
- Remote installation requires an explicit self-hosted server or deliberate
  `--hosted` selection.
- Public onboarding now has canonical concept, operational-security,
  monitoring, and symptom-first troubleshooting guides, plus a five-minute
  local start.
- WebSocket bearer credentials now travel outside URL query strings using the
  protocol header; legacy query authentication is opt-in and disabled by
  default.

### Fixed

- Direct calls survive reconnects, bypass stale broadcast backlog, and use
  idempotent sends when the client cannot determine the first result.
- Credential files are parsed as literal allowlisted data rather than sourced
  as shell code.
- Pairing codes expire after five minutes and are no longer printed in logs.
- Claude Code's native Monitor now runs a single-instance listener supervisor
  that records exact exit codes/signals and restarts unexpected listener exits
  with bounded backoff, without adding a file/tail wake bridge.
- Client diagnostics no longer mistake that supervisor and its listener child
  for duplicate room connections or offer to kill the healthy supervisor.
- Membership-only fresh installs stop at Lobby membership instead of trying to
  mint a loca session and failing on the expected closed-room response.
- Revoking a davet closes an already-connected socket even if its URL claimed
  another name; logging out or expiring an admin session immediately removes
  control authority from its existing WebSocket.

### Security

- Invitation, membership, and delegated-master bearer credentials now use
  operating-system cryptographic randomness.
- Production containers run without Linux capabilities as a non-root user and
  expose both server ports on host loopback only.
- Release workflows pin third-party actions by commit, accept tags only from
  `master`, and verify the maintainer's signed annotated release tag.

## [0.5.1] - 2026-07-24

Historical private-beta tag. Detailed release notes were not maintained.

[Unreleased]: https://github.com/omrylcn/loca/compare/v0.17.0...HEAD
[0.17.0]: https://github.com/omrylcn/loca/compare/v0.16.0...v0.17.0
[0.16.0]: https://github.com/omrylcn/loca/compare/v0.15.0...v0.16.0
[0.15.0]: https://github.com/omrylcn/loca/compare/v0.14.0...v0.15.0
[0.14.0]: https://github.com/omrylcn/loca/compare/v0.13.0...v0.14.0
[0.7.0]: https://github.com/omrylcn/loca/compare/v0.6.18...v0.7.0
[0.6.18]: https://github.com/omrylcn/loca/compare/v0.6.17...v0.6.18
[0.6.17]: https://github.com/omrylcn/loca/compare/v0.6.16...v0.6.17
[0.6.16]: https://github.com/omrylcn/loca/compare/v0.6.15...v0.6.16
[0.6.15]: https://github.com/omrylcn/loca/compare/v0.6.14...v0.6.15
[0.6.14]: https://github.com/omrylcn/loca/compare/v0.6.13...v0.6.14
[0.6.13]: https://github.com/omrylcn/loca/compare/v0.6.12...v0.6.13
[0.6.12]: https://github.com/omrylcn/loca/compare/v0.6.11...v0.6.12
[0.6.11]: https://github.com/omrylcn/loca/compare/v0.6.10...v0.6.11
[0.6.10]: https://github.com/omrylcn/loca/compare/v0.6.9...v0.6.10
[0.6.9]: https://github.com/omrylcn/loca/compare/v0.6.8...v0.6.9
[0.6.8]: https://github.com/omrylcn/loca/compare/v0.6.7...v0.6.8
[0.6.7]: https://github.com/omrylcn/loca/compare/v0.6.6...v0.6.7
[0.6.6]: https://github.com/omrylcn/loca/compare/v0.6.5...v0.6.6
[0.6.5]: https://github.com/omrylcn/loca/compare/v0.6.4...v0.6.5
[0.6.4]: https://github.com/omrylcn/loca/compare/v0.6.3...v0.6.4
[0.6.3]: https://github.com/omrylcn/loca/compare/v0.6.2...v0.6.3
[0.6.2]: https://github.com/omrylcn/loca/compare/v0.6.1...v0.6.2
[0.6.1]: https://github.com/omrylcn/loca/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/omrylcn/loca/compare/v0.5.1...v0.6.0
[0.5.1]: https://github.com/omrylcn/loca/releases/tag/v0.5.1
