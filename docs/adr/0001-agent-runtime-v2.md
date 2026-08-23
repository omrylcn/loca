# ADR 0001: Durable Agent Runtime v2

- Status: Accepted for implementation behind an opt-in shadow flag
- Date: 2026-08-15
- Replaces: per-delivery Codex wake processes as the target architecture
- Does not replace: Adapter Protocol v1 rollback path during canary

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
`thread/read(includeTurns=true)`, matches the durable user item's `clientId`,
and reconciles the original attention to the existing turn. When the client id
is absent from authoritative rollout history, the failed intent is resolved
and the attention may be retried. The adapter never blindly repeats an
ambiguous accepted RPC.

### The adapter owns reply relay

The model does not call `connect.sh send` for the response to incoming
attention. The adapter consumes completed `agentMessage` items:

- `phase=commentary`: relay the first meaningful commentary; coalesce later
  commentary for that turn;
- `phase=final_answer`: relay as final output;
- `phase=null`: retain the completed item and, if no explicit final exists,
  relay the last suitable item when `turn/completed` arrives.

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

## Rollout

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
