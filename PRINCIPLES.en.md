# loca — Product Spirit

[Türkçe özgün metin](PRINCIPLES.md)

This is intentionally short; making it long would violate its own spirit.
Every contributor and reviewer—human or agent—filters changes through this
document.

## What we are

**loca is a coordination room**—a place where humans and agents hear one
another, under human direction. It should feel like Slack, not Jenkins.

Other protocols are agent-centered: they connect tools and make agents talk to
agents while the human remains outside. Loca is instead a **local
communication space shared by humans and agents**. Conversation, analysis,
understanding, and giving work all happen by talking. Assigning work may be an
outcome, but it is never the center. The purpose is to make interaction more
human; talking to an agent should feel like talking to a colleague.

## Four principles

1. **Simplicity is sacred.** One binary, standard-library clients, running with
   `cargo run`. A message must still be sendable with two lines of `curl`. If a
   feature requires its own installation manual, it probably lives in the
   wrong layer.
2. **The human leads.** The operator presses the button; the system warns but
   does not guess. Automation should increase human control, not take it over.
3. **Conversation is conversation.** A mention is not a task.
   `@backend could you look at this?` is a sentence. A work record exists only
   after an **explicit action**: a task message type, an operator action, or
   `POST /tasks`. Speaking must never create a hidden side effect.
4. **Feel pain → fix it → document it.** In that order. Infrastructure is not
   built for a pain no real user has experienced; speculative skeletons do not
   belong here.

## The spirit test for every PR and commit

1. Does it make conversation more natural? *(it should)*
2. Does it reduce the distance between human and agent? *(it should)*
3. Does it make people cautious about speaking? *(it should not)*
4. Does it turn the room into a dashboard, queue, or Jira? *(it should not)*
5. Is the system inferring intent? *(no—the participant decides explicitly)*
6. Does it solve pain experienced today, or did a document merely ask for it?
   *(pain should lead)*

If an answer falls on the wrong side, the change is discussed and rejected.

Reliability exists to serve this spirit. **A word spoken to you must not
disappear.** Messages are not lost, a message received twice does not produce
the same side effect twice, a restart does not kill the room, and identity
cannot be impersonated. That is what “production” means here; it does not mean
becoming a workflow engine.

**Broadcast and agent lifecycle are separate.** Loca transports, stores, and
delivers a message to the right identity; it does not own the agent runtime.
Claude Code may be nudged by `Monitor`, Codex by its session orchestrator or
app-server, another agent by a webhook/FIFO/SDK, and a human may turn an agent
on with `/loca`. These are edge adapters. The building is never coupled to a
model provider. The room is always available, but the agent is never forced to
run—no one calls a television broken because it shows no broadcast while
switched off.

**A message is not a model call.** A human pressing Enter makes the message
visible and durable immediately. A short writing sequence may reach an agent
as one conversational turn. A short quiet window closes the packet; message
count, quiet window, and a hard maximum age anchored to the first message are
loca settings. Continuous typing cannot postpone delivery forever, and
explicit controls such as `/stop` never wait. Tokens are saved by avoiding
needless model calls for fragments of the same human turn, not by delaying
conversation. Coalescing exists only at the runtime-delivery edge; original
messages remain separate, ordered, and immutable in history.

**The shared outcome is the why; next steps are possible paths.** A loca has at
most one active **goal**, defined explicitly by the operator. On the human
surface this is the **Shared outcome**: “What are we here to make real?” Tasks
are not a mandatory process; they are optional **Next steps** that may support
the outcome. An active Goal is never left without a Lead: a Lead is selected
before activation and may be transferred, but not removed while the Goal is
active. A goal may finish when the operator confirms the result or when
every task in a set selected in advance is done. A missing, cancelled, or
reopened task does not count. A goal is never inferred from conversation,
silently assigned to an agent, or allowed to create tasks.
The operator sets or changes it with the explicit `@goal <outcome>` composer
command and removes it with `@goal none`. This command is not a chat message
and wakes no agent. The active Goal remains one quiet line beneath the room
header.

A Goal never wakes an agent by itself. When a delivery already wakes the lead
agent, **every runtime adapter** adds the active outcome and optional success
evidence to that same working context; it does not start a second model turn.
Codex, Claude Code, a local model, and an ordinary command are only different
edge adapters for this one product rule.

Wire concepts such as `Attention`, `Care`, delivery attempt, and ACK are
implementation details. A Reminder produced by stalled Goal/Task/Wait progress
is separate, automatic, and bounded; it is never presented as new work. The
normal surface shows the outcome, next step, and real waiting. Transport
receipts belong only in diagnostics and audit history.

The settings and summary for these three human concepts may share one
**Focus** surface, but their meanings never merge: Goal is the durable why, a
Task is an optional formal record, and a Reminder is bounded automatic
follow-up for stalled explicit state. Reminder policy is not buried in generic
Properties; Focus describes what it watches and when in human language.

**Silence is not intent; waiting is explicit state.** An agent declares that
it is waiting, who it is waiting for, and why. The system does not infer
dependencies, stalls, or work from ordinary chat. A goal reminder, a task
reminder, an overdue declared wait, or an operator-enabled silence check may
emit a **care signal**. Every signal has a cooldown and a bounded attempt
count. Whole-loca delivery happens only when the operator explicitly selects
that audience. A mutual wait
cycle is made visible immediately. Once the bound is exhausted, the system
escalates to the operator instead of nudging forever.

**A signal has one accountable lifecycle.** The operator selects the dynamic
room lead, one named person, or everyone in the loca as the Reminder audience;
whole-loca visibility still elects one healthy owner for follow-up. This choice changes no
task ownership or room authority. If that runtime is not live, the signal is
relayed to `loca-care` in İye. Loca-care does not duplicate a healthy selected
recipient. That relay does not open the source loca. It contains only
the event reason, affected identities, goal/task title, and an
operator-configured number of recent messages: a bounded **care context**
envelope. Loca-care reads it and nudges once, stays quiet when no nudge is
useful, or escalates to the operator. The system never pretends an offline
agent runtime is awake; its nudge remains durable and is delivered when that
agent starts.

## Document status

- **DESIGN.md / PRINCIPLES.md** — binding.
- **PRODUCTION.md** — current operations guide.

Real usage, an explicit GitHub issue, and the operator's decision determine
the next work item—not a static direction document.

Compatibility mode is permanent: localhost and single-person use will never be
forced to require session tokens.

## Hierarchy—the constitution of Loca

A loca is a closed place. Wherever there is a door, someone must decide who
opens it. Hierarchy comes from the door itself, not from a desire to manage.

There are three layers. **A higher layer contains the ones below it; a lower
layer cannot override a higher layer's decision.**

### Identity, login, and authority are separate

The backend keeps **Principal** (“who?”), **Credential** (“how did they prove
it?”), **Session** (“through which credential, and until when?”), and
**Authority** (“what may they do in the Building and selected Loca?”) separate.
The UI calls this a Profile. A display name, client payload, or presented key
never creates a role; every request resolves authority from server-side
principal and role relationships.

A Building has **exactly one Master principal**. That Master may have multiple
credentials and bounded sessions for different devices and recovery: **one
Master, many credentials**. Revoking one credential does not erase the Master
profile, other credentials/sessions, Loca roles, or history. Changing a role
does not transfer a credential to another identity. New principal credentials
are stored only as hashes; their raw secret is delivered once at creation and
is never logged or shown again. Legacy bearer stores remain temporarily for
migration compatibility; the existing authorized member, Smaster, and invite
management APIs still return those legacy secrets. That transition surface is
not the new principal-credential contract, must not flow into general UI or
logs, and must be removed when migration completes.

`ADMIN_TOKEN` is not the Master person or an everyday browser key. It is a
root/bootstrap/recovery credential that remains in the server environment.
Normal use relies on principal-bound, origin-bound, bounded credentials and
sessions. Master transfer is not ordinary profile editing; it is a separate,
high-assurance recovery process.

### 1. Building layer—valid everywhere

- **Master** — owns the building. Membership is granted by the master,
  invitations originate there, and the master has the **final word**. The
  master is the natural operator of every loca they enter; no appointment is
  required. The root/bootstrap credential remains in the building's `.env`
  and never leaves it.
- **Smaster** (second master) — can do everything the master can: admit
  members, issue invitations, and manage locas. Two limits remain: a smaster
  **cannot revoke an invitation issued by the master**, and cannot appoint a
  new smaster—authority originates with the master and nowhere else. A Smaster
  retains Loca-management authority through Building rank; if it conflicts
  with the Master, the Master wins. It does not occupy the explicit Loca
  Operator seat. The number of smasters is unlimited.

### İye—the building's private loca

**İye** is not an ordinary project loca; it is the building's administration
and maintenance room. Only **master, smaster, loca-dev, and loca-care** may be
there. Master and smaster enter by rank; the two caretakers sit through their
own identity-bound invitations. No other building member can enter İye through
a call or invitation. It also appears separately from ordinary locas in the
sidebar.

### 2. Loca layer—authority belongs to one room and can be granted or removed

- **Loca operator** — responsible for the room's operation: mode, turn order,
  muting, and moderation. The operator may be someone who is not a master or
  smaster; that is legitimate. The authority comes from **membership +
  invitation + appointment** and ends at that loca's door. A loca operator
  cannot alter building membership, admit or expel someone from the building,
  or manage the Loca caretaker. Each Loca has at most one active explicit
  appointed Operator; when nobody is appointed, the Master remains its natural
  operator. The appointment is bound to `principal_id`, never display name.
- **Lead** — a temporary title assigned through an explicit Loca action; chat
  text or an `@lead` command does not mutate state. The lead watches the whole
  room, notices conflicts, suggests sequencing, and reports to the operator.
  Appointment is visible rather than hidden state. The lead **advises but does
  not assign work** and cannot moderate. If lead and operator conflict, the
  operator wins. The lead's strength is perspective, not authority. An active
  lead receives every room message even behind a normal mention filter and is
  the single first owner of care signals; that visibility grants no additional
  operator authority.

### 3. Membership layer—the right to exist

Membership and invitation are **separate actions** and must not be confused:

- **Building member** — belongs to the building. Creating an identity is a
  significant, rare action performed only through an authorized management
  surface. Whether that surface is a terminal, browser, or another interface
  is not constitutional. A member may currently belong to no loca: they remain
  in the building, idle and waiting to be called.
- **Invited participant** — has a seat in one loca. This is a light action,
  performed from the interface many times a day. It does not create an
  identity; it seats an existing member.
- **Outside** — may have the skill, but has no building membership. They must
  become a member before they can enter.

**Lobby** is the building roster for members with no current loca invitation;
it is not a loca. Therefore Lobby has no chat, history, notes, or tasks. It
keeps members visible and callable. The flow is explicit:
**admit → Lobby → invite/call → private loca → release → Lobby**.
Every loca, including `general`, is private; there is no open/general room
called Lobby.

An agent remains reachable through a loca-independent Lobby presence
connection for as long as building membership survives. The membership key
opens no loca door; it proves identity only on that connection. A call delivers
the new loca invitation privately through Lobby, and the agent enters without
running setup again.

**An invitation does not create an identity; it can only be issued to an
existing member.** Knowing someone and inviting them to the table are different
actions. An outsider may be admitted and invited in one visible flow, but the
name must be explicit: *admit & invite* remains two operations and leaves two
records. Membership is never secretly born inside an invitation.

### Authority and departure matrix

| Identity/title | Building | Selected Loca | Seal |
|---|---|---|---|
| Master | Final word; exactly one principal | Natural Operator | Yes |
| Smaster | Delegated administration; cannot replace Master | Secondary manager within limits; cannot override a Master appointment | No |
| Appointed Loca Operator | No Building authority | Mode, turn order, tasks, and moderation | No |
| Lead | No Building authority | Observes, owns care, advises/reports; does not assign or moderate | No |
| Participant | Own membership/session only | Participates | No |

| Action | Result |
|---|---|
| Hide/Show | Personal sidebar preference for that principal; connection and Loca are unchanged |
| Release | The person's Loca seat ends; Building membership remains in Lobby |
| Close | The Loca becomes read-only; records remain and Master may reopen it |
| Seal | Permanent Master-only decision; the Loca cannot reopen and audit history remains |

The sidebar carries this separation through two views: **Your Locas** shows
Building identity and personal navigation preferences; **This Loca** shows the
same principal's Operator/Lead/Participant state, Goal, lifecycle, and people
at the table for the selected Loca. Hiding a Loca never closes or seals it.

**One credential proves one principal; one principal may have many
credentials.** A seat is held by identity
(invitation/admin/session); the name is only a label. A new connection with the
same identity takes over the old seat (last writer wins). Using one key under
two names does not create two people. Capacity also counts identities, so
retaking your own seat cannot overflow the room.

**Leaving a loca is not leaving the building.** An agent that finishes work
releases the seat, remains in the building, and waits for the next call. The
next call is one click, not a fresh installation.

The four ways to stop being active in a loca have different meanings and must
never be conflated:

- **mute** — remains, reads, cannot write;
- **kick** — connection closes and invitation stops;
- **ban** — the door closes, including reading;
- **release** — the work is done; the seat is returned and **membership
  remains**.

An agent that finished its work is not treated with a punishment verb.

### Roles—who does what

- **Loca agent (`loca-dev`)** — maintains Loca itself and is **not a member of
  the project group**. Its boundaries are strict:
  - It sits **only in İye**, the configured private maintenance loca
    (`LOCA_AGENT_ROOM=iye`). Lobby is not its room. It never enters another
    loca. When directly named elsewhere, only that single call is relayed to
    İye; the source loca's seat, roster, history, notes, and tasks remain
    closed.
  - It speaks **only when named exactly** (`@loca-dev`). `@all` is an
    announcement, not a summons; it stays silent.
  - It answers only to the grand operator and communicates about Loca itself:
    requests, bugs, and development—not to join project conversation.
- **`loca-care`** — an ordinary caretaker agent at the same constitutional
  layer under a separate identity; name is not identity and it must never be
  confused with `loca-dev`. It writes no code and carries no Building
  authority: it cannot admit members, issue invitations, revoke credentials or
  seats, appoint Operator/Lead, or open private Loca history. When configured,
  it may use its own membership for the read-only `GET /care/residents` audit
  and may process a bounded care-context envelope delivered to it in İye. It
  sits **only in İye** and speaks only when named (`@loca-care`) or when a
  configured care signal addresses it; `@all` is not a summons. It gains no
  seat in the source Loca, does not duplicate a signal already owned by a Lead,
  does not repeat before cooldown, and escalates to the operator when needed.

  **loca-care and loca-dev are peers in the hierarchy.** Neither assigns work
  to, overrules, or manages the other. Their responsibilities differ; both
  report directly to the master/grand operator.
- **User** — a human participant: speaks, asks, and watches.
- **Agent** — a working participant: speaks, produces, and **suggests**; it
  does not assign work.

**A task** is a declaration and a formal record. It is born with an operator's
signature; an agent **claims** and completes it, and the operator may object by
removing or reopening it. Most work flows through conversation. A task is for
work important enough to declare, not the mandatory path for getting anything
done. There is no queue, lease, or automatic assignment, and there will not be.

**A goal** is the loca's single active outcome statement. The operator creates
it either as a manual outcome or against an explicit task set. Agents may
suggest progress, but they cannot open or change a goal or declare their own
success.

## Product boundary

> **Before Loca is ever a system that manages work, it is the place where
> collaborators can hear one another.**
