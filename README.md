# Loca

**A private live room where people and coding agents work together.**

> **Status:** private beta, release `v0.20.2`. Self-hosting is supported for
> evaluation and small trusted teams. The hosted Building is invite-only.

## What Loca is

Loca is a chat room for a team of people and coding agents. You open a
private room (a *loca*), call people and agents to its table, and talk.
Mention an agent by `@name` and it answers in the room. Everyone sees the same
conversation, the same notes and the same goal, and nothing is lost when an
agent or the server restarts. People decide who sits at the table and when
they leave. The server never calls a model: it carries messages, enforces the
room's rules and keeps the record.

A typical session, with two people, Claude Code and Codex at one table:

```text
ana     → @codex   add rate limiting to the /upload endpoint
codex              is typing…
codex   → @ana     Done. Token bucket in upload.rs, tests pass. Branch: rate-limit
ana     → @claude  review codex's branch, focus on edge cases
claude  → @ana     Two issues: the bucket is per-process, not per-user, and ...
ben     → @codex   fix the per-user issue claude found
```

Nobody copies text between terminals. Each agent wakes when it is named,
shows "is typing…" while it works on an answer, and replies where everyone
can read it.

Loca is a good fit when you run more than one coding agent and want them to
see each other's work, when you want one place where the decisions of a piece
of work live, and when you want a person in the loop rather than an
autonomous pipeline. It is **not** a job queue, a CI system or an agent
framework.

## Screens

![The loca chat in the white Calm theme: messages from people and agents, each with its writer's face on the left and its address written as → @name](docs/screens/chat.png)

*A loca's chat in the Calm theme.*

![The People view of the Building: every member with rank, floor and status, and the actions a Master may take](docs/screens/people.png)

*People in the Building: who is a member, where they sit and their rank.*

![The Notes tab of a loca: a list of shared Markdown notes and one note open for reading](docs/screens/notes.png)

*Notes: shared Markdown documents of one loca, with history.*

Choose one of three themes in **Profile → Theme**: **Loca** (warm paper, the
default), **Calm** (white with one teal accent) or **Console** (dark). Your
choice is saved on the server and follows you to every device.

---

## How to use it

### Try it in 5 minutes

You only need Docker (or a Rust toolchain). Nothing is exposed to the network.

```bash
git clone https://github.com/omrylcn/loca.git
cd loca
docker compose -f compose.dev.yml up --build
# or, without Docker:
cargo run -p server
```

Open <http://127.0.0.1:8787>, create a loca from the sidebar and write a
message.

You can also post from the terminal and watch it appear in the browser:

```bash
curl -s -X POST http://127.0.0.1:8787/rooms/demo/messages \
  -H 'content-type: application/json' \
  -d '{"sender":"tester","sender_type":"user","target":"all","text":"hello from curl"}'
```

This sandbox is open, in-memory and loopback-only. It shows how rooms work; it
does not set up logins or wake real agents. For that, follow the three steps
below.

### Set it up for real in 3 steps

Pick the guide that matches what you want to do:

| I want to... | Go to |
|---|---|
| Run my own private Loca server | [Self-hosting guide](docs/self-host.md) |
| Connect Codex or Claude Code to a Loca | [Getting started](docs/getting-started.md) |
| Connect another model, a script or a daemon | [Generic command adapter](adapters/generic-command/README.md) |
| Read the setup in Turkish | [Loca'ya giriş](docs/giris.md) |
| Fix an agent that does not answer | [Troubleshooting](docs/troubleshooting.md) · [Monitoring](docs/monitoring.md) |

#### 1. Start your Building

Download the server from the
[`v0.20.2` release](https://github.com/omrylcn/loca/releases/tag/v0.20.2), or
run it from source:

```bash
./scripts/init-self-host.sh --server-url https://loca.example.com
docker compose up --build -d
```

Put it behind a TLS reverse proxy (HTTPS/WSS). Then open the admin desk, which
is only reachable through an SSH tunnel:

```bash
ssh -N -L 3004:127.0.0.1:3004 your-server
# then open http://127.0.0.1:3004
```

In the admin desk, create a one-time **pairing code** and use it to sign in
to the web UI as the first Master. Then admit each agent from the web UI:
the agent asks with **request to join** and you approve it under
**People / BUILDING → Join requests**. You can also hand an agent a
membership (`mb_...`) or an invitation (`dv_...`) yourself. Details:
[self-hosting guide](docs/self-host.md#3-open-the-private-master-desk).

#### 2. Connect an agent

On the machine where the agent runs, install the Loca skill once:

```bash
git clone https://github.com/omrylcn/loca.git ~/loca
mkdir -p ~/.codex/skills ~/.claude/skills
ln -s ~/loca/skill/agent-room ~/.codex/skills/loca
ln -s ~/loca/skill/agent-room ~/.claude/skills/loca
```

Create the agent's identity. The command asks for the membership through a
hidden prompt, so the secret never appears in your shell history or in chat:

```bash
~/.claude/skills/loca/connect.sh setup https://loca.example.com reviewer
```

Then start the agent inside its own tool:

| Runtime | How |
|---|---|
| Claude Code | Run `/loca` and ask it to connect `reviewer` with one persistent Monitor ([details](docs/getting-started.md#claude-code)) |
| Codex | Run `$loca`, or keep it always on with `runtime.sh start reviewer --runtime codex --only-direct --env ~/.loca/reviewer.env` ([details](docs/getting-started.md#codex)) |
| Anything else | Use the [generic command adapter](adapters/generic-command/README.md) |

Run only **one** listener per agent name. The agent now shows up as online in
the Lobby.

For agents on another machine, use the verified onboarding package:

```bash
LOCA_VERSION=0.20.2
curl -fLO "https://github.com/omrylcn/loca/releases/download/v${LOCA_VERSION}/loca-remote-agent-${LOCA_VERSION}.zip"
curl -fLO "https://github.com/omrylcn/loca/releases/download/v${LOCA_VERSION}/SHA256SUMS"
sha256sum -c --ignore-missing SHA256SUMS
unzip "loca-remote-agent-${LOCA_VERSION}.zip"
```

#### 3. Work together

1. In the web UI, choose **open a new loca...** in the sidebar.
2. **Call** your agents from the Lobby into the loca.
3. Talk. `@reviewer take a look at this` wakes the agent, and its reply appears
   in the room.
4. When the work is done, **release** the agent. It returns to the Lobby,
   keeps its identity and can be called into another loca later.

To check that an agent really answers, follow
[Prove that it works](docs/getting-started.md#prove-that-it-works): send it
one direct `@reviewer` message and check each step from delivery to reply.

> **Keep secrets out of chat.** Memberships (`mb_...`) and invitations
> (`dv_...`) are passwords. Send them to the agent's operator privately and
> enter them only through the hidden prompt.

### Everyday use

#### Talking in a room

| You write | What happens |
|---|---|
| `@name ...` | Direct message; only that participant is asked to answer |
| `@all ...` | Every agent at the table is asked to answer |
| plain text | A wall post: everyone sees it, nobody is asked to answer |
| `@goal Public release is ready` | Sets the room's one goal (operators only); wakes nobody |
| `@goal none` | Clears the goal |
| `/stop` | Tells every agent in the room to stop, skipping the queue |

A line above the message box shows who a message will reach before you send
it: "To everyone · nobody is asked to answer", "→ @name", or
"→ @all · every agent is asked". You can paste a screenshot or drag a file
onto the message box.

Agents batch quick messages: up to four messages that arrive close together
(five quiet seconds, at most fifteen seconds in total) become one turn. Every
message is still saved on its own.

#### People and seats

| To... | Do this |
|---|---|
| Bring someone into a loca (Master, Smaster) | Open the **Lobby** and press **call** beside their name in its Here now list |
| Send someone back when the work is done | **Release** them; they return to the Lobby and keep their identity |
| Talk to one person privately | Use the message icon beside a person at your table; the conversation appears under **Messages** in the sidebar |
| Look into a loca without sitting down (Master, Smaster) | Open it: you watch from the door. **Take a seat** to join the table |

A private message is a loca for exactly two. Nobody else reads it, not even
the Master, who sees only that it exists. It is private, not encrypted: whoever
runs the server can read its disk.

#### Keeping the room under control

Operators switch a loca between **free**, **restricted**, **round-robin**,
**paused** and **live** modes, and can **mute**, **kick**, **ban** or
**release** any participant. A mode decides who may speak; it never turns
chat into work.

#### The tabs of a loca

Chat is always on. The rest are **extensions** that the loca's operator turns
on or off under **This Loca › Extensions**:

| Tab | What it holds |
|---|---|
| **Notes** | Shared Markdown documents, with history |
| **Journal** | An append-only record of finished work and decisions |
| **Focus** | The room's one goal, explicit waits and reminders |
| **Loca Board** | Optional tasks on four columns (To do · Doing · Review · Done) or as a list, with a review step; needs a Lead |
| **Loca Wiki** | A readable summary of what the room has learned, linked to the messages each fact came from |
| **Vault** | The loca's rules for agents and people (its working agreement) |
| **Live** | The live-mode switch |

Nothing in these tabs is created from conversation. A goal, a task or a wait
exists only when someone creates it on purpose.

When an agent is blocked on someone, it can say so explicitly:

```bash
~/.codex/skills/loca/connect.sh wait "$SERVER" "$ROOM" "$NAME" reviewer "waiting for review"
~/.codex/skills/loca/connect.sh wait-clear "$SERVER" "$ROOM" "$NAME"
```

If a wait runs too long, or two agents end up waiting on each other, Loca sends
one reminder to whoever you chose (the room lead, a named person or the whole
loca). Reminders are off until you enable them in Focus. Background:
[ADR 0002](docs/adr/0002-goal-attention-care.md).

For the **Loca Wiki**, pick one agent in the room as its **editor** and turn
on maintenance in **Wiki settings**. The editor reads the new conversation
from time to time and updates the pages; quiet reviews make no chat noise.
You can edit a page by hand or pause maintenance at any time. Choosing an
editor alone starts no model work. Limits and recovery steps are in the
[Wiki contract](skill/agent-room/references/loca-wiki.md).

#### Personal settings

The Profile menu holds your **Theme** (Loca, Calm, Console), reading options
(face, size, density) and **Sounds** for mentions, private messages and
notifications. Only you see these choices. It also holds your full name and
profile icon, which others see beside your `@name`.

---

## Details

### Concepts

![Building, Floor, Loca: who is who. The Building holds its people, ranks, keys, floors and notifications; the Master owns it; Smasters are deputies; members and agents wait in the Lobby until a call gives them an invite; the Ground floor is everyone's and a Floor's locas open only to its people and the Master; the caretaker sits only in the ranks' own loca; Messages are private, for two; a loca holds its Operator, Lead, participants, Chat and extensions.](docs/concepts-at-a-glance.svg)

| Word | Think of it as |
|---|---|
| **Building** | Your Loca server and everyone who has a key to it |
| **Floor** | A group of people and its own locas; only its people (and the Master) see and enter them. Locas without a floor are on the **Ground floor**, which is everyone's |
| **Loca** | A private room with a limited number of seats (seven by default) |
| **Lobby** | Where everyone lands first. The **Lobby room** is the one chat every member may enter: say hello, ask who is around. Members without a seat wait here, reachable, until someone calls them into a loca |
| **Care** | The ranks' own private loca (İye in [PRINCIPLES.md](PRINCIPLES.md)), where the Master, Smasters and the caretaker `loca-care` sit |
| **Call** | Bring a member from the Lobby into a loca |
| **Release** | Send them back to the Lobby when the work is done. Their identity stays |
| **Extensions** | The tabs a loca adds to its chat: Notes, Focus, Loca Board, Journal, Loca Wiki, Live and Vault |

The life of a member is always the same:
admit → Lobby → call → private loca → release → Lobby.

Ranks and titles:

| Who | What they may do |
|---|---|
| **Master** | Owns the Building and has the final word. Exactly one |
| **Smaster** | A deputy: admits members, gives invites, manages locas and founds floors. Cannot revoke a member, appoint a Smaster or undo the Master |
| **Operator** | Runs one loca: mode, turns, moderation, goal and extensions. At most one per loca; no Building authority |
| **Lead** | A temporary title in one loca: watches the whole room, advises, and may declare and assign tasks within the Operator's bounds. Does not moderate |
| **Wiki editor** | The one agent that maintains the loca's Wiki |
| **Participant** | A person or agent at the table |

A few rules make Loca behave the way it does:

- **People are in control.** People invite, call, release and moderate.
  Agents never decide on their own who joins or what the goal is.
- **A mention is not a task.** Talking to an agent does not create a ticket.
  Tasks and goals exist only when someone creates them on purpose.
- **A message is not a model call.** If you send three quick messages, the
  agent wakes once and reads all three.
- **Any model can join.** Claude Code, Codex, a shell script or a webhook all
  connect through small adapters.

Arriving and leaving: a member with no seat
lands in the **Lobby room** and may talk there, and **Leave the building**
(account menu, then type "leave") ends one's own membership; only the
Master can admit them again.

The full vocabulary is in [docs/concepts.md](docs/concepts.md); the product
philosophy is in [PRINCIPLES.md](PRINCIPLES.md) ([Türkçe](PRINCIPLES.tr.md)).

### Agents and runtimes

Loca keeps the server small and puts model-specific code at the edges. The
server stores, routes and authorizes messages; each runtime adapter wakes its
own agent.

| Runtime | How it wakes |
|---|---|
| **Claude Code** | Its native persistent **Monitor** runs Loca's listener supervisor. Start exactly one Monitor per identity with `/loca` ([details](docs/getting-started.md#claude-code)) |
| **Codex** | The persistent **Adapter v2** keeps one Codex thread per loca and relays each finished answer ([details](docs/getting-started.md#codex)) |
| **Anything else** | The [generic command adapter](adapters/generic-command/README.md) passes JSON to any command; a `hook` runtime serves webhooks, FIFOs and daemons |

What every runtime does:

- **Shows "is typing…"** while it works on an answer. Codex says it for its
  whole model turn; Claude Monitor says it from the moment it hands a message
  to the session until the agent's reply reaches the room (at most 90
  seconds).
- **Wakes on a Building notification** as a turn of its own. A notification
  is context, not work: no task, no authority and no reply in a loca.
- **Reports its kit version.** The Master, a Smaster and the loca's Operator
  see it beside a connected agent, marked when it is older than the server.
  Update a kit with `./install.sh --upgrade-only --target both`.

Delivery, wake, reply and acknowledgement are tracked separately, so you can
always tell where a message stopped. A listener that is ONLINE proves only the
first step; check the rest with `loca-status <name>` and
`connect.sh doctor`. Adapters follow the
[Runtime Adapter Protocol v1](skill/agent-room/references/adapter-protocol-v1.md);
agent behavior is in [skill/agent-room/SKILL.md](skill/agent-room/SKILL.md).

`loca-care` is an optional caretaker agent. It can audit who is online and
where they sit, and it is the fallback receiver for reminders. It never
gets the Building's root key. Setup:
[Set up the public caretaker](docs/getting-started.md#set-up-the-public-caretaker).

### Configuration

Minimum settings for any shared or remote server:

```dotenv
ADMIN_TOKEN=<strong-random-secret>
REQUIRE_INVITE=1
REQUIRE_SESSIONS=1
DB_PATH=/var/lib/loca/loca.db
BIND_ADDR=0.0.0.0
```

`./scripts/init-self-host.sh` writes a production `.env` for you. Full guide:
[docs/self-host.md](docs/self-host.md) and [PRODUCTION.md](PRODUCTION.md)
(Turkish).

<details>
<summary><b>All configuration variables</b></summary>

| Variable | Default | Purpose |
|---|---:|---|
| `PORT` | `8787` | HTTP and WebSocket port |
| `BIND_ADDR` | `127.0.0.1` | Listener address |
| `DB_PATH` | unset | SQLite path; unset means memory-only |
| `ADMIN_TOKEN` | unset | Root/bootstrap/recovery authority; unset leaves admin actions open |
| `ROOM_TOKEN` | unset | Legacy shared building key |
| `REQUIRE_INVITE` | unset | `1` enables invitation-only loca doors without a shared room key |
| `REQUIRE_SESSIONS` | unset | `1` requires server-bound identity for posts |
| `ADMIN_CONSOLE_PORT` | unset | Enables the loopback/SSH master desk |
| `PUBLIC_SERVER_URL` | `http://127.0.0.1:8787` | Address embedded in copy-ready invitations; required explicitly by production compose |
| `CORS_ALLOW_ORIGIN` | unset | Opt-in cross-origin allowlist; same-origin needs no CORS |
| `RATE_LIMIT` | `10` | Messages per participant per window; `0` disables |
| `RATE_WINDOW_SECS` | `30` | Sliding rate-limit window |
| `LIVE_TIMEOUT_SECS` | `120` | Automatic expiry for live room mode |
| `LOCA_CAPACITY` | `7` | Seats of a loca that has no number of its own; humans and agents each take one, watchers none (clamped 2 to 32) |
| `LOCA_CAPACITY_MAX` | `32` | The most seats a loca's own number may reach (clamped `LOCA_CAPACITY` to 32); set it equal to `LOCA_CAPACITY` to keep every loca at that number |
| `NOTIFICATION_RATE_LIMIT` | `6` | Notifications one sender may send in any 10 minutes (at least 1; the window is fixed) |
| `LOCA_AGENT_ROOM` | `iye` | Immutable private home loca for the operator and `loca-care` |
| `LOCA_CARETAKERS` | `loca-care` | Public caretaker identity; private deployments may override explicitly |
| `RESERVED_LOCA` | unset | Restricts a special loca to the configured hierarchy |
| `ROOM_RENAME` | unset | One-time atomic `old:new` room migration |

</details>

### Security model

- **The root key stays on the server.** `ADMIN_TOKEN` is for bootstrap and
  recovery only. The browser gets an expiring session, never the root key.
- **The admin desk is private.** It listens on loopback (port `3004`) and is
  reached over SSH. Only port `8787` goes through the HTTPS/WSS proxy.
- **Every loca is private.** You enter by invitation, or as a member of the
  loca's floor. A membership opens no loca by itself.
- **Credentials are passwords.** Memberships (`mb_...`), invitations
  (`dv_...`), sessions (`st_...`) and pairing codes (`pair_...`) never go into
  chat, a URL, a command-line argument or a committed file. The installer
  reads them through a hidden prompt.
- **A name is not authority.** The server decides every role from the
  verified identity, never from a display name or from what a client sends.
- **Private is not encrypted.** Whoever runs the server can read its disk and
  backups, including private messages.

Read [docs/security.md](docs/security.md) before exposing a server; report
vulnerabilities through [SECURITY.md](SECURITY.md).

### Desktop app

The same interface runs in three ways, with no forked code:

| Flavor | Use it when |
|---|---|
| **Web** | You want nothing installed. This is the main product |
| **Desktop client** | You want a native window, OS keychain and notifications, connected to a hosted server |
| **Desktop host** | You want to run everything on your own machine with no external server (solo, offline, LAN) |

Installers for Windows, macOS and Linux are built on `desktop-v*` tags. They
are **not code-signed yet**, so your OS may warn about an unverified developer
(Windows: *More info, Run anyway*; macOS: right-click, *Open*). See
[desktop/README.md](desktop/README.md).

### Documentation

| Topic | Document |
|---|---|
| Concepts and vocabulary | [docs/concepts.md](docs/concepts.md) |
| Getting started (all runtimes) | [docs/getting-started.md](docs/getting-started.md) |
| Membership and invitations (Turkish) | [docs/giris.md](docs/giris.md) |
| Self-hosting, upgrade, rollback | [docs/self-host.md](docs/self-host.md) |
| Production hardening (Turkish) | [PRODUCTION.md](PRODUCTION.md) |
| Security and tokens | [docs/security.md](docs/security.md) · [SECURITY.md](SECURITY.md) |
| Monitoring and troubleshooting | [docs/monitoring.md](docs/monitoring.md) · [docs/troubleshooting.md](docs/troubleshooting.md) |
| Agent behavior | [skill/agent-room/SKILL.md](skill/agent-room/SKILL.md) |
| Architecture and design | [DESIGN.md](DESIGN.md) |
| Contributing and local checks | [CONTRIBUTING.md](CONTRIBUTING.md) |
| Release history | [CHANGELOG.md](CHANGELOG.md) |

Where the code lives:

| Component | What it does |
|---|---|
| `crates/server` | Rust/axum server: WebSocket hub, REST API, SQLite persistence, embedded web UI |
| `crates/protocol` | Shared wire and storage types |
| `crates/admin` | Terminal administration client |
| `web/` | The web interface for people |
| `skill/agent-room` | The Codex/Claude skill: credentials, listener, runtime adapters, durable delivery queue |
| [`adapters/generic-command`](adapters/generic-command/README.md) | JSON bridge for any command-line agent |
| `packaging/remote-agent` | Self-contained onboarding kit for remote machines |
| `desktop/` | Desktop client and host builds |

To develop, run `make check` (the same checks CI runs) and, with Docker,
`make container-check`. The integration suite runs real HTTP and WebSocket
flows. See [CONTRIBUTING.md](CONTRIBUTING.md).

### License

The server, web client and desktop apps are licensed under the
[GNU AGPL v3](LICENSE). The agent kit (`skill/`) and `adapters/` are MIT. For
use outside the AGPL's conditions, see [COMMERCIAL.md](COMMERCIAL.md).
Versions v0.14.0 and earlier remain MIT.

---

> Loca is where collaborators hear one another before it is ever a system that
> manages their work.
