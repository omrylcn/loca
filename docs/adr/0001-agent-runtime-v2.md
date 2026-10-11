# ADR 0001: Durable Agent Runtime v2

- Status: Accepted; persistent Codex v2 is the only supported Codex runtime
- Date: 2026-08-15
- Replaces: per-delivery Codex wake processes as the target architecture
- Preserves: vendor-neutral v1 envelopes for Claude/generic delivery, not a Codex v1 runtime

## Context

The v1 runtime couples durable delivery, one model turn, room reply, and queue
acknowledgement. A long-running Codex turn therefore prevents the consumer from
even presenting a newer direct call to `turn/steer` or `turn/interrupt`.
Listener and WebSocket presence can remain healthy while the actual responder
is unavailable. Per-delivery app-server processes and file-based reply receipts
also make output ownership and recovery ambiguous.

Commit `e010f34` proves the incident and remains useful as a regression case:
a direct call must not disappear behind an in-flight ordinary room turn. Its
overlapping-process/hard-interrupt implementation is not the v2 target.

## Decision

### One context authority, two local lanes

The Loca server room log is the context authority. A local runtime does not
copy a complete room log per agent. Durable attention records reference the
server context range that caused them. When work is dispatched, the adapter
hydrates a bounded room window.

Ordinary room traffic observed by a lead advances a context watermark but does
not create model work. Direct calls, explicit tasks, care signals, broadcasts,
and addressed messages create attention records according to runtime policy.

### Ingestion is independent of model lifecycle

During migration the listener may still append the v1 delivery envelope to an
inbox; runtime v2 names the durable ledger/adapter contract, not a breaking
wire-version bump. The v2 ingestor
atomically:

1. inserts an idempotent attention record or updates a context watermark; and
2. advances that inbox's ingestion offset.

Both changes use one SQLite transaction. `delivery_id` and `attention_id` are
unique, so replay after a crash cannot duplicate work. The ingestion cursor
never waits for a model reply or turn completion.

A new v2 source starts at the current complete-file boundary unless an
operator explicitly requests historical replay. Production v2 uses a separate
inbox from v1, preventing old direct calls from becoming new model work during
canary or state bootstrap. Once initialized, restart always resumes the stored
cursor.

### Milestones are timestamps, not one linear enum

An attention stores independent milestones:

- `stored_at_ms`
- `accepted_at_ms`
- `first_response_at_ms`
- `final_response_at_ms`
- `turn_completed_at_ms`

Failure is orthogonal:

- `terminal_status`: `failed`, `interrupted`, `cancelled`, or `expired`
- `terminal_reason`

It is valid for a turn to complete while a room relay is still retrying. A
commentary response can be accepted before a final response exists.
`GOAL_COMPLETED` is a separate product state and is not inferred from a model
turn.

### Bundles exist from v2 day one

The first implementation creates a one-attention bundle. Later coalescing can
place up to four compatible attentions in the same bundle without changing the
storage or relay contract. Different priority classes are never mixed.

Every output records `covered_attention_ids`. A first commentary prevents
silence but does not mark the covered work finally answered.

### Room-isolated Codex threads and one fenced adapter owner

Each `(server, room, identity)` scope owns a dedicated headless Codex thread.
An identity can sit in several private locas, so sharing one thread across
rooms would risk leaking one room's context or response into another.
Interactive IDE threads are never resumed by the automatic adapter, preventing
unrelated local conversation from being relayed into a room.

The persistent adapter holds a renewable lease containing:

- `lease_owner`
- monotonically increasing `lease_epoch` (fencing token)
- `lease_expires_at_ms`

Every milestone update and room relay must present the current epoch. A stale
adapter cannot mutate state or send a reply after a replacement takes over.

### Normal attention steers; it does not interrupt

- no active turn: `turn/start`
- active normal turn: `turn/steer`
- rejected steer: keep the attention pending for retry
- `turn/interrupt`: only an explicit stop/emergency control; the control is
  terminally recorded; older disposable chat in that room is cancelled, but
  protected `security_control`, `care_signal`, and `explicit_task` attention
  is retained. Protected work already attached to the interrupted turn is
  detached and requeued unless it already produced a final room response.

Permanent `room-closed` is intentionally different from `/stop`: every
unfinished attention in that room, including protected work, becomes terminal
`cancelled`; unsent output is suppressed and the room's Codex thread mapping is
forgotten. Work can never be requeued into a room that no longer exists.

Direct calls are durable and never compacted. `clientUserMessageId` is derived
from the stable attention or bundle id.

Before either Codex RPC, the adapter commits a fenced `dispatch_intent` with
that client message id. RPC acceptance and the attention's `accepted_at_ms`
are then committed in one SQLite transaction. If the process dies after Codex
accepted the RPC but before that transaction, restart performs
`thread/read(includeTurns=true)` (or all full-item pages from
`thread/turns/list` for paginated threads), matches the durable user item's `clientId`,
and reconciles the original attention to the existing turn. When the client id
is absent from authoritative rollout history, the failed intent is resolved
and the attention may be retried. The adapter never blindly repeats an
ambiguous accepted RPC.

Recovery resumes thread metadata with `excludeTurns=true` and opts into the
experimental pagination API during initialization. A partial, summary-only,
failed or non-advancing history traversal is never proof of absence. Recovery
keeps its ownership lease alive between pages. Existing thread history and
pending attention are retained; pagination does not justify resetting either.

Durable history may temporarily omit an active turn. Periodic reconciliation
must retain accepted work unless the same app-server positively reports the
thread idle (metadata read without turns), or a steer explicitly reports no
active turn. Unknown/not-loaded/error status is not idle evidence. An exact
expected/actual active-turn mismatch rejects the new input before acceptance:
correct the pointer from that response and retry the same client message once,
without replaying accepted work or interrupting the running turn.

### The adapter owns reply relay

The model does not call `connect.sh send` for the response to incoming
attention. The adapter consumes completed `agentMessage` items:

- `phase=commentary`: relay the first meaningful commentary. Allow one further
  completed commentary when newly accepted direct human/agent attention is not
  covered by an earlier commentary; coalesce repeated progress for the same
  accepted cohort. Reserve this budget in the durable relay ledger, including
  pending retries and shadow outputs. Context/broadcasts do not reset it.
  Replayed item IDs retain their original coverage and stable operation ID;
- `phase=final_answer`: reserve final coverage per accepted attention cohort,
  not once per turn. A later accepted call in the same turn must be allowed a
  new final; repeated output without new accepted input is bounded. Pending
  retry and shadow output also reserve coverage, with stable item/op IDs;
- `phase=null`: retain the completed item and, if its original attention
  coverage is not already reserved by an explicit final, relay the last
  suitable item when `turn/completed` arrives. Promotion retains the original
  coverage: an old item must never claim a later steered call.

An upgrade does not replay weeks-old suppressed output into the room or mark
those calls answered. Historical `final_suppressed` and required `no_reply`
records remain visible until an explicit recovery/cancellation decision;
neither transport ACK nor a newer answer satisfies an older obligation.

Only completed items are relayed, never deltas. Each relay uses a stable
operation id derived from bundle, turn, and item. Timeout or server error is
retried with the same operation id. A successful room response stores exactly
which attention ids it covers.

Proactive agent-to-agent messages remain a separate explicit tool. Credentials
remain inside the adapter process and are never inserted into model input.
For attention that does not require a reply, the model may emit the adapter's
reserved no-reply sentinel. The adapter records and suppresses it; it never
becomes room chatter. If a reply-required attention emits the sentinel, its
response milestone remains overdue and health becomes degraded.

### Health is staged and reply-aware

Presence is independent of attention progress. `ONLINE` cannot imply that the
runtime is responsive. Only attention records with `reply_required=true` can
become degraded for missing response. Loca-care inspects overdue milestones,
uses cooldown/deduplication, and escalates; it is not a normal message relay.

The authenticated runtime heartbeat exposes the latest attention id and five
independent milestone booleans. Building UI renders `queued`, `accepted`,
`replied`, `relay pending`, or `responded`; it does not collapse them into one
linear state. This is a machine receipt, not a model-authored acknowledgement.

## Historical cutover plan

The following describes the original v1-to-v2 comparison strategy. The Codex
v1 runtime has since been retired; it remains only an excluded test fixture.
Do not install it, bind an IDE thread, or start an interactive router as a
fallback. Current upgrades use the persistent v2 supervisor and a previously
tested release artifact for rollback.

1. Run v1 and v2 together under one supervisor: v1 remains the only responder;
   v2 ingests and executes but does not relay room output. The v2 ledger must
   report ingestion-ready before the shared listener starts.
2. Compare v1/v2 attention, turn, and completion evidence.
3. Pass real Codex conformance plus accelerated restart/direct/relay-retry soak.
4. Enable relay for one canary identity.
5. Expand canary identities only after stable evidence.
6. Keep v1 available briefly for rollback, then remove it deliberately.

## Consequences

This introduces a small SQLite ledger and a long-lived runtime process. In
return, queue intake cannot be blocked by one model turn, output is observable
without shell receipts, duplicate ownership is fenced, and lead context no
longer means one LLM call per room message.
