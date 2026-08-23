# ADR 0002: Goal, Attention, and Care

- Status: Accepted
- Date: 2026-08-17
- Scope: public/open release contract
- Depends on: [ADR 0001](0001-agent-runtime-v2.md)

## Decision

Loca keeps the room's durable purpose, runtime delivery, and stalled-work
supervision as three connected but distinct concepts:

- A **Goal** is the one operator-declared outcome that keeps the loca open.
  It does not wake a model and does not create tasks.
- A **Task** is an optional, explicit path toward that outcome. Conversation
  never becomes a task implicitly.
- A **Wait** is an agent's explicit declaration that progress depends on one
  named participant.
- **Context** is durable room history. Context can be visible to a lead without
  creating one model call per message.
- **Runtime attention records** are internal durable wake bookkeeping. They are
  not tasks and are not exposed as a user-managed focus surface.
- **Reminder/Care** is the server-side, condition-driven producer of one
  bounded reminder delivery when explicit progress has stalled.

The core relationship is therefore:

```text
Goal / Task / Wait state
          │
          │ explicit progress stops and a configured threshold expires
          ▼
 Reminder/Care scheduler
          │ lead/caretaker · cooldown · bounded attempts · bounded context
          ▼
 Runtime attention record
          │
          ▼
Runtime adapter → Loca reply → lifecycle receipt
```

A goal does not contain care, and care does not complete a goal. Care only
makes stalled explicit state visible to the one coordinator responsible for
deciding what happens next.

## Progress semantics

Goal/task aging is measured from `progress_at`, not from the latest room
message. Only an explicit state transition advances it:

- create, assign, take, reopen, complete, cancel, or edit a task;
- create or edit a goal;
- change a task explicitly linked to the active goal;
- achieve or cancel the goal.

Unrelated chat never postpones a reminder. A no-op PATCH never manufactures
progress. Existing persisted rows migrate with `progress_at = created_at`.

Plain room silence remains a separate, operator-enabled condition. This keeps
"the room is quiet" distinct from "the declared outcome has not advanced."

An active Goal may name one optional `checkpoint` (the next observable proof,
not an implicit task) and a per-goal `stale_after_secs`. The threshold inherits
the room's Goal reminder setting when omitted and is disabled by explicit `0`.
Changing the checkpoint is explicit progress; chat is not. When progress
resumes, the old stalled-condition Attention is resolved and its pending Care
deliveries are retired atomically. If work later stalls at the new checkpoint,
that progress generation receives a new Attention.

## Care ownership and noise control

Each care signal has exactly one owner:

1. the room's lead, but only while both its seat and authenticated runtime
   heartbeat are healthy;
2. otherwise the configured `loca-care` identity in İye, again only with a
   healthy runtime;
3. if neither exists, the durable condition remains unresolved and attempts
   do not burn while nobody can receive them.

The scheduler runs independently of chat and emits at most one signal per
loca per sweep. Explicit wait cycles and overdue waits have priority; optional
task, goal, and room-silence conditions follow. A repeated condition carries a
stable subject, cooldown, bounded attempt count, and an escalation marker.

Waking is not speaking. The owner reads the signal and its bounded context,
then either acts, routes one direct nudge, escalates, or remains silent when it
has no new information. Care must never create periodic "still waiting"
chatter or an `@all` storm.

## Privacy boundary

A lead already belongs to the source loca and sees that loca's context.
`loca-care` does not gain a seat in the source loca. Its durable care envelope
contains only:

- source loca name and reason;
- subject, target, and relevant participant names;
- the configured last `N` messages (0–20).

The envelope is delivered in İye and ACKed only after the listener stores it
durably. It does not copy the source room's full history into İye chat.

## Reminder ownership and runtime lifecycle

Goal/Task/Wait reminders are the sole user-facing Care surface. The operator routes them
to the dynamic room lead, one named person, or everyone in the loca. The all
audience is visible to every live room runtime but retains one accountable
owner; an unhealthy selected runtime falls back to `loca-care`. The product UI does not present them
as separate operator focus records. The persistence layer uses an internal
attention ledger for Care delivery records, but presentation keeps that transport
state out of declared work. Multiple
Care attempts for one stalled condition share one stable internal condition id
and each attempt has its own ACK id, so retries never manufacture apparent
tasks or operator focus records.

Presence is independent from model progress. Milestones are independent
timestamps/receipts, not one linear enum:

```text
STORED          durable inbox/ledger write
ACCEPTED        runtime accepted start/steer ownership
FIRST_RESPONSE  first completed commentary accepted by Loca
FINAL_RESPONSE  final completed output accepted by Loca
TURN_COMPLETED  runtime turn reached a terminal boundary
GOAL_COMPLETED  product outcome was explicitly achieved
```

`TURN_COMPLETED` never implies `GOAL_COMPLETED`. A completed turn may still
have a reply retry pending. Runtime health is degraded only for overdue,
reply-required attention; transport presence alone never proves health.

## Human surface

The Web client exposes one **Focus** tab rather than a project-management
dashboard. It colocates three summaries without collapsing their semantics:

- Goal: one room-purpose line, managed explicitly with `@goal <outcome>` and
  `@goal none`, also visible as a compact room-wide strip;
- Tasks: optional explicit owner/status records;
- Reminders: bounded follow-up thresholds for stalled Goal, Task, Wait, or
  explicitly enabled room silence.

`@goal` is a control command rather than a persisted chat message, so setting
the purpose never manufactures a model turn. Reminder settings do not live in
generic room Properties; their receipts stay in Reminder history.

Loca-care's public audit reports both axes:

```text
online/loca | online/lobby | away/invited | away/lobby
runtime=healthy | runtime=degraded/<stage> | runtime=unverified
```

## Human message batching

Every human message is written to room history immediately and independently.
Runtime delivery may combine a short burst into one attention bundle:

- default maximum: 4 messages;
- quiet window: 5 seconds after the latest message;
- hard maximum age: 15 seconds from the first message;
- security/control events bypass batching;
- different attention classes are not mixed.

This reduces one-Enter/one-LLM-call waste without rewriting chat history or
delaying indefinitely.

## Failure behavior

- Listener delivery, runtime wake, response relay, and completion ACK are
  supervised and reported separately.
- A runtime that is online but not progressing is `degraded`, not healthy.
- Care signals use a durable ACKed outbox and replay with the same identity.
- Direct user messages remain FIFO and are never compacted away.
- Normal attention uses `turn/start` or `turn/steer`; hard interrupt is limited
  to explicit stop/security/emergency control.
- Stable operation IDs make reply retry idempotent.
- A failed adapter does not complete, cancel, or mutate a goal.

## Public-release acceptance

The open release must prove all of the following:

1. Goal/task/wait and care state survive a server restart.
2. Unrelated chat does not reset goal/task progress age.
3. Linked task progress resets its goal reminder age.
4. The selected healthy lead/person is the sole first care owner.
5. A transport-online but runtime-degraded selected recipient is skipped for healthy
   `loca-care`.
6. Care replay is exactly-once at the model-effect boundary after ACK retry.
7. Loca-care's read-only audit exposes away and runtime-degraded states without
   revealing credentials or private room history.
8. The server's message bundle limits are covered by tests and visible in room
   settings.
9. Codex and Claude Code each pass delivery → wake → reply → ACK conformance.
10. Runtime-v2 live relay remains a separate canary promotion; shipping its
    shadow code does not silently replace the proven responder.

## Consequences

This adds explicit progress timestamps and a small amount of operational
state, but avoids a task scheduler or autonomous workflow engine. Loca remains
a place where humans and agents work together: the room remembers why it is
open, the runtime knows what deserves attention, and one caretaker notices
when the two stop moving together.
