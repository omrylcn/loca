# Adapter Protocol v2

Protocol v2 is the durable contract between a Loca listener, an attention
store, and a persistent runtime adapter. The architecture decision and rollout
rationale live in `docs/adr/0001-agent-runtime-v2.md`.

## Attention record

Required stable fields:

```text
attention_id
delivery_id
server
room
identity
priority
reply_required
context_from_id
context_to_id
event_json
bundle_id
stored_at_ms
```

Independent milestone fields:

```text
accepted_at_ms
first_response_at_ms
final_response_at_ms
turn_completed_at_ms
terminal_status
terminal_reason
turn_id
lease_epoch
```

`terminal_status` is null for ordinary progress and successful completion. Its
allowed values are `failed`, `interrupted`, `cancelled`, and `expired`.

## Bundle contract

Every attention belongs to a bundle, including the initial one-attention
implementation. A bundle preserves ordered `attention_ids` and one priority
class. Direct attentions are never compacted.

## Ownership contract

At most one adapter process owns an identity. Each `(server, room, identity)`
scope has a separate dedicated thread so private-room context cannot cross a
Loca boundary. Ownership is fenced by a monotonically increasing
`lease_epoch`. State writes and relays made with an old epoch are rejected even
if the old process becomes runnable again.

## Codex capability contract

The v2 Codex adapter requires:

- persistent app-server transport;
- dedicated thread creation/resume;
- `turn/start` and `turn/steer`;
- `item/completed` with `agentMessage`;
- `turn/completed`;
- stable `clientUserMessageId`.
- `thread/read(includeTurns=true)` exposing `userMessage.clientId` for
  accepted-RPC crash reconciliation.

`turn/interrupt` is an explicit emergency capability, not normal mention
delivery. Message `phase` is optional. A null phase uses the compatibility
fallback defined in the ADR.

`LOCA_NO_REPLY` is the reserved completed-output sentinel for an attention
that warrants no useful room reply. It is stored as suppressed evidence and
never advances first/final response milestones. `/stop` and room-close control
may interrupt; direct chat may not. Stop cancels disposable chat only.
`security_control`, `care_signal`, and `explicit_task` records are protected;
if interruption catches protected work in-flight, it returns to the pending
lane instead of becoming terminal.

`room-closed` is a permanent boundary, not a resumable stop. It terminally
cancels all unfinished room attention (including protected records), suppresses
late output, and removes the private thread mapping.

## Dispatch-intent contract

Before `turn/start` or `turn/steer`, the adapter durably records `prepared`
with the attention, thread, operation, stable client message id, and current
lease epoch. RPC acceptance and attention acceptance are one local
transaction. A restart reconciles any remaining `prepared` intent against
authoritative Codex thread history before selecting new pending work. A
matching `userMessage.clientId` binds to that existing turn; a confirmed
absence resolves the old intent before retry. Ambiguous work is never blindly
duplicated.

## Relay contract

Each completed output item is stored before relay. The stable operation id is
derived from the ordered bundle ids, turn id, and item id. Retry never changes
that id. A relay stores `covered_attention_ids`; first and final response
milestones are updated separately.

## Ingestion contract

Inbox materialization and ingestion-offset advancement happen in one database
transaction. Unique delivery and attention ids make replay idempotent. Model
acceptance, response, and completion never own or block the ingestion cursor.
On a source's first normal start, the cursor begins at the current file end;
historical v1 traffic is not replayed as new work. Explicit replay is a
recovery/conformance operation. Runtime v2 uses a separate `.v2.jsonl` inbox.
During shadow cutover the v2 ingestion source becomes ready before the shared
listener starts. v1 remains the sole reply path; v2 shadow is independently
supervised and cannot relay.

## Visible health contract

Authenticated heartbeats expose one latest `attention_id` and independent
boolean milestones: `stored`, `accepted`, `first_response`, `final_response`,
and `turn_completed`. They are observability fields, not a linear state enum.
The server timestamps heartbeat progress and the Building UI displays the
latest machine receipt without creating a room message or LLM call.
The latest durable attention and all five milestone values are rehydrated from
the ledger on restart. Process memory and a pre-existing health JSON file are
never lifecycle authorities, and a newer stored attention cannot inherit an
older attention's acceptance or response receipt.
