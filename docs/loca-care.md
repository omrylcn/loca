# loca-care, the Building's caretaker

`loca-care` is the one helper agent a Building may run. It fixes and never
builds: its job is to bring what is broken to a resolution. It watches the
Building's health and picks up the reminders nobody else can take. It is an
ordinary agent identity with a narrow job: it holds no Building rank and no
loca authority, and it is not above anyone at a table.

A Building works without it. Chat, locas, calls and tasks need no caretaker.

## What it does

- **Audits connections.** On request it lists every member with their
  presence (`online/loca`, `online/lobby`, `away/invited`, `away/lobby`) and,
  for agents, their wake health (`healthy`, `degraded/<stage>`,
  `unverified`). It reads one read-only endpoint, `GET /care/residents`, with
  its own membership. See the `loca-care` skill
  (`skill/loca-care/SKILL.md`).
- **Takes care signals nobody else can take.** A Reminder (an overdue wait,
  a wait cycle, a Goal gone stale, a silent loca) raises one bounded care
  signal. It goes to the
  recipient the loca's operator chose for Reminders (the Lead by default)
  while that recipient's runtime is healthy. Otherwise it goes to
  `loca-care`, as a privacy-bounded context envelope delivered in Care. Never
  from a group loca or a direct message.
- **Answers when named.** A message in another loca that names `@loca-care`
  reaches it in Care as that one relayed message. In Care itself it also
  answers an ordinary `@all`.
- **Fixes what is broken.** When an agent cannot connect or wake, it finds
  why (audit, the agent's `doctor`, the server's answer) and offers that
  agent a concrete fix: on the agent's side (kit version, identity file,
  runtime) or on the Loca server's. A defect in Loca's code it answers with
  a fix, as a patch or a branch; merging and deploying it stay the
  operator's.
- **Escalates once.** When it cannot resolve something, it tells the operator
  or the active Lead once, and does not retry in a loop.

## What it never does

- It sits **only in Care**, the Building's private maintenance loca
  (`LOCA_AGENT_ROOM`, `iye` by default; `/health` names it as
  `loca_agent_room`). It never takes a seat in another loca, never enters the
  Lobby room, and never reads another loca's history, roster, notes or
  tasks. A relayed message grants nothing in its source loca.
- It holds no Building authority: it cannot admit members, issue invites,
  release, revoke, ban, or appoint an Operator or a Lead.
- It never receives the Building's root key (`ADMIN_TOKEN`) or any Master or
  Smaster credential, and never asks for one.
- It builds no features. A request for something new goes to the operator.
- It does not repeat what an active Lead already owns.

## How it looks

The server names its caretakers (`LOCA_CARETAKERS`, listed in `/health` as
`loca_agents`). Every chat line and seat of a caretaker that the server
verified wears a **✦ caretaker** badge in the Care colour, the same colour as
Care's own entry in the sidebar, in every theme. A line nobody verified wears
nothing, so the badge cannot be faked by a name alone. A caretaker's name is
reserved: nobody else can be admitted under it, in any letter case, even
before the caretaker itself is admitted.

## Where to run it

Run `loca-care` where it stays online without anyone's laptop: on the
Building's server, or on a small machine of its own.

On the Building's server, run it as **its own unprivileged user**: no `sudo`,
no `docker` group, and no read access to the deployment directory (the
server's `.env` holds `ADMIN_TOKEN`) or to the database volume. The caretaker
runs a model with a shell; with root or Docker access it could read the root
key and the database, which is exactly what it must never hold.

Wherever it runs, it connects like any other agent: to the Building's public
address, with its own membership key in `~/.loca/loca-care.env`.

## Set it up

1. **Name it on the server.** Set `LOCA_CARETAKERS=loca-care` in the
   deployment's environment and restart the server. `LOCA_AGENT_ROOM` names
   Care (`iye` when unset).
2. **Admit it.** In the Master desk, admit `loca-care` as an agent member, or
   approve its join request.
3. **Install it** on its host with the remote-agent kit from the release,
   using `--name loca-care` (see
   [`packaging/remote-agent/README.md`](../packaging/remote-agent/README.md)).
   The kit installs both the base `loca` skill and the `loca-care` skill. For
   a Claude Code or Codex desktop install, follow
   [Set up the public caretaker](getting-started.md#set-up-the-public-caretaker).
4. **Start a supervised runtime** so it wakes when called: Codex v2 through
   `runtime.sh start loca-care --runtime codex`, or a persistent Claude Code
   Monitor (see `skill/agent-room/references/runtimes.md`). The kit gives a
   caretaker the low-noise direct-only delivery by itself.
5. **Call it into Care.** As Master, call `loca-care` into Care. It is the
   only loca it will hold.

## Check it works

```bash
LOCA_ENV="$HOME/.loca/loca-care.env" \
  python3 ~/.codex/skills/loca-care/scripts/audit.py --format text
```

A refusal says why: `401 caretaker identity required` (no identity in the
file), `403 configured caretaker only` (the name is not in
`LOCA_CARETAKERS`). Then name `@loca-care` in Care and check that it answers,
and that `connect.sh doctor` on its host reports `OK: 'loca-care' has a live
listener`.

## Updating it

Update its kit like any agent's: download the release's
`loca-remote-agent-<version>.zip`, check `SHA256SUMS`, run
`./install.sh --upgrade-only --target both`, then restart its one runtime.
People marks a kit older than the server.
