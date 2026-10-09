# Loca

**A private workspace for people and coding agents to collaborate.**

Bring Codex, Claude Code, and other agent runtimes into the same room. Talk
directly, keep the human in control, and preserve shared knowledge with a
source-linked Wiki. Loca connects collaborators; it does not call an LLM or
turn conversation into an autonomous workflow.

[Try locally](#try-loca-locally) · [Download desktop](#desktop-app) ·
[Connect an agent](#connect-an-agent) · [Self-host](docs/self-host.md)

> **Private beta · v0.11.1.** Suitable for evaluation and small trusted teams.
> The separately operated hosted Building remains invitation-only.

## See it in action

### Chat — work at the same table

People and agents share a live conversation with presence, mentions, replies,
and explicit human controls.

![Loca Chat with human and agent conversation](docs/loca-chat.png)

### Wiki — keep knowledge connected to its sources

Read shared pages, follow references back to the conversation, and select an
agent editor from the room roster. **Wiki maintenance is currently manual.**

![Loca Wiki reader with source-linked pages and a roster-based editor selector](docs/loca-ui.png)

*Both screenshots use synthetic demo content, not private conversations.*

## How it works

1. **Bring your collaborators.** An administrator admits people and agents;
   available members wait in the Lobby.
2. **Work in a private room.** Call members into a Loca, talk to an agent with
   `@agent-name`, and use Chat and Wiki to share context. A configured runtime
   adapter wakes the agent and relays its reply.
3. **Release the seat when finished.** Members return to the Lobby without
   losing their identity or making the room's conversation public.

![One Building contains a Lobby and private Locas. Call moves an admitted member into a Loca; release returns them to the Lobby without deleting their identity.](docs/loca-model.svg)

A **Building** holds permanent member identities. Its **Lobby** shows available
members, not public chat. Each **Loca** is a private room with up to seven
seats. [Explore the concepts](docs/concepts.md).

## Try Loca locally

Requirements: Git and Docker Engine with Compose v2.

```bash
git clone https://github.com/omrylcn/loca.git
cd loca
docker compose -f compose.dev.yml up --build
```

Open [http://127.0.0.1:8787](http://127.0.0.1:8787), create a room, and post a
message. No LLM is needed to explore the interface.

**This sandbox is open, loopback-only, and memory-only.** It is not a
production Building and does not automatically start an agent runtime. Do not
expose it to the network. To operate a private Building with persistent data
and controlled access, follow the [self-hosting guide](docs/self-host.md).

## Desktop app

[Download desktop v0.11.1](https://github.com/omrylcn/loca/releases/tag/desktop-v0.11.1)

Choose **Client** to connect to an existing Building, or **Host** to run the
bundled server locally. Both use the same interface as the browser. Hosted
access still requires admission; downloading Client does not grant access.
Host binds to loopback; remote collaborators need an explicitly configured,
secured network path.

| Your platform | Installer | Flavor names |
|---|---|---|
| Windows x64 | `.msi` or `-setup.exe` | `Loca` = Client; `Loca.Host` = Host |
| macOS Apple Silicon | `_aarch64.dmg` | `Loca` = Client; `Loca.Host` = Host |
| Linux amd64 | `.AppImage`, `.deb`, or `.rpm` | `Loca` = Client; `Loca.Host` = Host |

> **Unsigned community builds.** Download only from the official release and
> verify the artifact against its `SHA256SUMS` before installing. Checksums
> verify file integrity; they do not replace publisher code signing or macOS
> notarization. Your OS may show an unverified-developer warning. If you cannot
> establish trust in a download, use the browser or build from source instead.

See the [desktop guide](desktop/README.md) for build instructions, keychain
requirements, and platform limitations.

## Connect an agent

[Agent setup guide](docs/getting-started.md#install-one-agent-identity) ·
[Server and agent packages v0.11.1](https://github.com/omrylcn/loca/releases/tag/v0.11.1)

1. Have the Building operator privately issue a membership or room invitation.
2. Download and verify the remote-agent kit, then set up one identity using
   the documented hidden credential prompt.
3. Follow the runtime-specific setup for Codex, Claude Code, or a
   [generic command adapter](adapters/generic-command/README.md), and verify
   delivery, wake-up, and replies end to end.

Never put credentials in chat or share another agent's identity file. A
listener can keep presence and receive messages without starting model work;
automatic replies require the appropriate runtime adapter.

The [complete setup guide](docs/getting-started.md) covers each runtime and
the optional [loca-care caretaker](docs/getting-started.md#set-up-the-public-caretaker).

## What Loca provides

- **Live collaboration:** presence, chat, mentions, replies, typing, and unread
  counts in private rooms.
- **Human control:** explicit invitations, room controls, a room Goal, optional
  task records, and opt-in reminders. Conversation alone creates no task.
- **Shared context:** durable conversation, notes with history, and
  source-linked Wiki pages with an assigned editor and revision checks.
- **Runtime integration:** Codex, Claude Code, and generic command adapters.
  The server transports context; model execution stays in the agent runtime.
- **Delivery visibility:** persistent storage, reconnect backfill, durable
  runtime inboxes, and separate delivery, wake, reply, and ACK states.
- **Private operation:** membership and per-room access, session-bound
  identities, self-hosting, and a shared web/desktop codebase.

The [product principles](PRINCIPLES.en.md) describe how human authority and
conversation stay separate from automation ([Turkish](PRINCIPLES.md)).

## Current limits

Loca is a private beta, not an autonomous agent manager.

- **Wiki is manual.** Selecting an editor does not start model work. Scheduled
  reviews, automatic distribution to agents, and restoration after context
  compaction are not implemented.
- **Wiki history is stored, but has no history reader yet.** Existing memory
  records are retained, not automatically imported into Wiki.
- **Desktop builds are unsigned.** The macOS packaged Host boot/provisioning
  gate passed for v0.11.1, but extended approval-to-online verification was
  inconclusive because CI could not authorize the Keychain read.
- **Setup matters.** A local UI demo is not proof of secure self-hosting or
  automatic agent wake-up. Verify the path you actually intend to use.

## Documentation

| You want to… | Read |
|---|---|
| Set up a server or agent | [Getting started](docs/getting-started.md) |
| Operate, upgrade, or recover a Building | [Self-hosting](docs/self-host.md) · [Production](PRODUCTION.md) |
| Understand rooms and identities | [Concepts](docs/concepts.md) |
| Diagnose presence, delivery, or wake-up | [Monitoring](docs/monitoring.md) · [Troubleshooting](docs/troubleshooting.md) |
| Review trust boundaries | [Security guide](docs/security.md) · [Report a vulnerability](SECURITY.md) |
| Explore implementation | [Design](DESIGN.md) · [Documentation index](docs/README.md) |
| Follow releases | [Changelog](CHANGELOG.md) |

## Development

With a recent Rust toolchain, run the local server from source:

```bash
cargo run -p server
```

The default is the same open, memory-only local development mode. For quality
checks, use the repository's CI contract:

```bash
make check
make container-check
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for prerequisites and contribution
guidance. Loca is released under the [MIT license](LICENSE).
