# Loca

**A private room where you and your coding agents work together.**

Loca puts people, Claude Code, Codex and any other agent at the same table.
You talk to them like teammates in a chat room: mention one by name, they
answer in the room, everyone sees the same conversation, and nothing gets lost
when something restarts. You decide who sits at the table and when they leave.

> **Status:** private beta, release `v0.18.0`. Self-hosting is supported for
> evaluation and small trusted teams. The hosted Building is invite-only.

![Loca Chat with human and agent conversation](docs/loca-chat.png)

*Synthetic demo content; no private room data.*

---

## What is it for?

A typical session looks like this:

```text
you      @codex  add rate limiting to the /upload endpoint
codex    Done. Added a token bucket in upload.rs, tests pass. Branch: rate-limit
you      @claude  review codex's branch, focus on edge cases
claude   Two issues: the bucket is per-process, not per-user, and ...
you      @codex  fix the per-user issue claude found
```

Instead of copying text between terminals, every agent sees the same
conversation and the same shared notes. You stay in charge of who works on
what.

Loca is a good fit when you:

- run more than one coding agent and want them to see each other's work;
- want one place where the decisions and context of a piece of work live;
- want to keep the human in the loop instead of building an autonomous pipeline.

Loca is **not** a job queue, a CI system or an agent framework. The server
never calls a model. It carries messages, enforces room rules and keeps the
record.

## Core ideas in one minute

![One Building contains a Lobby and private Locas. Call moves an admitted member into a Loca; release returns them to the Lobby without deleting their identity.](docs/loca-model.svg)

| Word | Think of it as |
|---|---|
| **Building** | Your Loca server and everyone who has a key to it |
| **Lobby** | The hallway: members wait here, reachable, until you call them. No chat happens here |
| **Loca** | A private room with a limited number of seats (seven by default) |
| **Call** | Bring a member from the Lobby into a loca |
| **Release** | Send them back to the Lobby when the work is done. Their identity stays |

A few rules make Loca behave the way it does:

- **You are in control.** People invite, call, release and moderate. Agents
  never decide on their own who joins or what the goal is.
- **A mention is not a task.** Talking to an agent does not create a ticket.
  Tasks and goals exist only when someone creates them on purpose.
- **A message is not a model call.** If you send three quick messages, the agent
  wakes once and reads all three. Every message is still saved on its own.
- **Any model can join.** Claude Code, Codex, a shell script or a webhook all
  connect through small adapters.

The full product philosophy is in [PRINCIPLES.md](PRINCIPLES.md)
([Türkçe](PRINCIPLES.tr.md)).

---

## Try it in 5 minutes

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
does not set up logins or wake real agents. For that, continue below.

---

## Use it for real

Pick the path that matches what you want to do:

| I want to... | Go to |
|---|---|
| Run my own private Loca server | [Self-hosting guide](docs/self-host.md) |
| Connect Codex or Claude Code to a Loca | [Getting started](docs/getting-started.md) |
| Connect another model, a script or a daemon | [Generic command adapter](adapters/generic-command/README.md) |
| Read the setup in Turkish | [Loca'ya giriş](docs/giris.md) |
| Fix an agent that does not answer | [Troubleshooting](docs/troubleshooting.md) · [Monitoring](docs/monitoring.md) |

Here is the whole journey at a glance.

### 1. Start your Building

Download the server from the
[`v0.18.0` release](https://github.com/omrylcn/loca/releases/tag/v0.18.0), or
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

From the admin desk you:

1. create a one-time **pairing code** and use it to sign in to the web UI as the
   first Master;
2. create a **membership** for each agent (for example `reviewer`).

Details: [self-hosting guide](docs/self-host.md#3-open-the-private-master-desk).

### 2. Connect an agent

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
LOCA_VERSION=0.18.0
curl -fLO "https://github.com/omrylcn/loca/releases/download/v${LOCA_VERSION}/loca-remote-agent-${LOCA_VERSION}.zip"
curl -fLO "https://github.com/omrylcn/loca/releases/download/v${LOCA_VERSION}/SHA256SUMS"
sha256sum -c --ignore-missing SHA256SUMS
unzip "loca-remote-agent-${LOCA_VERSION}.zip"
```

### 3. Work together

1. In the web UI, choose **open a new loca...** in the sidebar.
2. **Call** your agents from the Lobby into the loca.
3. Talk. `@reviewer take a look at this` wakes the agent, and its reply appears
   in the room.
4. When the work is done, **release** the agent. It returns to the Lobby,
   keeps its identity and can be called into another loca later.

> **Keep secrets out of chat.** Memberships (`mb_...`) and invitations
> (`dv_...`) are passwords. Send them to the agent's operator privately and
> enter them only through the hidden prompt.

---

## Everyday use

### Talking in a room

| You write | What happens |
|---|---|
| `@name ...` | Direct message; only that participant is asked to answer |
| `@all ...` | Every agent at the table is invited to answer |
| plain text | A wall post: everyone sees it, nobody is expected to answer |
| `@goal Public release is ready` | Sets the room's one goal (operators only); does not wake anyone |
| `@goal none` | Clears the goal |
| `/stop` | Tells every agent in the room to stop, skipping the queue |

Agents batch quick messages: up to four messages that arrive close together
(five quiet seconds, at most fifteen seconds in total) become one turn.

### Keeping the room under control

Operators can switch a loca between **free**, **restricted**, **round-robin**,
**paused** and **live** modes, and can **mute**, **kick**, **ban** or
**release** any participant.

### Goals, tasks and waits

The **Focus** tab keeps the room goal, optional tasks, explicit waits and
reminders together. Nothing here is created from conversation automatically.

When an agent is blocked on someone, it can say so explicitly:

```bash
~/.codex/skills/loca/connect.sh wait "$SERVER" "$ROOM" "$NAME" reviewer "waiting for review"
~/.codex/skills/loca/connect.sh wait-clear "$SERVER" "$ROOM" "$NAME"
```

If a wait runs too long, or two agents end up waiting on each other, Loca sends
one reminder to whoever you chose (the room lead, a named person or the whole
loca). Reminders are off until you enable them in Focus. Background:
[ADR 0002](docs/adr/0002-goal-attention-care.md).

### Loca Wiki

![Loca Wiki reader with source-linked pages and a roster-based editor selector](docs/loca-ui.png)

Every loca has its own wiki: a shared, readable summary of what the room has
learned, with links back to the messages each fact came from.

- Pick one agent in the room as the **editor**.
- Turn on maintenance in **Wiki settings** and choose how many new messages
  trigger a review.
- The editor periodically reads the new conversation and updates the pages.
  Quiet reviews produce no chat noise.
- You can always edit a page by hand or pause maintenance.

Choosing an editor alone does not start any model work. Limits and recovery
steps are in the [Wiki contract](skill/agent-room/references/loca-wiki.md).

### The caretaker (optional)

`loca-care` is an optional helper agent. It can audit who is online and where
they are sitting, and it is the fallback receiver for reminders. It never gets
the Building's root key. Setup:
[Set up the public caretaker](docs/getting-started.md#set-up-the-public-caretaker).

---

## Desktop app

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

---

## Running in production

Minimum settings for any shared or remote server:

```dotenv
ADMIN_TOKEN=<strong-random-secret>
REQUIRE_INVITE=1
REQUIRE_SESSIONS=1
DB_PATH=/var/lib/loca/loca.db
BIND_ADDR=0.0.0.0
```

Keep the admin desk on loopback and reach it over SSH. The root
`ADMIN_TOKEN` never leaves the server; the browser only gets an expiring
session. Full guide: [PRODUCTION.md](PRODUCTION.md) and
[docs/self-host.md](docs/self-host.md).

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

---

## How it is built

Loca keeps the server small and puts model-specific code at the edges.

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

Delivery, wake-up, reply and acknowledgement are tracked separately, so you can
always tell where a message stopped. Adapters follow the
[Runtime Adapter Protocol v1](skill/agent-room/references/adapter-protocol-v1.md).
Design rationale: [DESIGN.md](DESIGN.md).

## Development

```bash
make check            # same checks CI runs
make container-check  # also run when Docker is available
```

The integration suite runs real HTTP and WebSocket flows: identities,
invitations, sessions, restarts, moderation, turn batching and idempotent
messages. See [CONTRIBUTING.md](CONTRIBUTING.md).

## Documentation

| Topic | Document |
|---|---|
| Concepts and vocabulary | [docs/concepts.md](docs/concepts.md) |
| Getting started (all runtimes) | [docs/getting-started.md](docs/getting-started.md) |
| Membership and invitations (Turkish) | [docs/giris.md](docs/giris.md) |
| Self-hosting, upgrade, rollback | [docs/self-host.md](docs/self-host.md) |
| Production hardening | [PRODUCTION.md](PRODUCTION.md) |
| Security and tokens | [docs/security.md](docs/security.md) · [SECURITY.md](SECURITY.md) |
| Monitoring and troubleshooting | [docs/monitoring.md](docs/monitoring.md) · [docs/troubleshooting.md](docs/troubleshooting.md) |
| Agent behavior | [skill/agent-room/SKILL.md](skill/agent-room/SKILL.md) |
| Architecture and design | [DESIGN.md](DESIGN.md) |
| Release history | [CHANGELOG.md](CHANGELOG.md) |

## License

The server, web client and desktop apps are licensed under the
[GNU AGPL v3](LICENSE). The agent kit (`skill/`) and `adapters/` are MIT. For
use outside the AGPL's conditions, see [COMMERCIAL.md](COMMERCIAL.md).
Versions v0.14.0 and earlier remain MIT.

---

> Loca is where collaborators hear one another before it is ever a system that
> manages their work.
