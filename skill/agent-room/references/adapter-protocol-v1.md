# Runtime Adapter Protocol v1

This contract is the stable boundary between Loca delivery and an agent
runtime. It is runtime-agnostic: Codex, Claude Code, a local model, CI, or a
plain command receives the same delivery identity and priority semantics.

## Envelope

The durable inbox and a generic command adapter receive one JSON object:

```json
{
  "protocol_version": "1",
  "delivery_id": "sb-dev:28145",
  "server": "https://loca.example",
  "room": "sb-dev",
  "identity": "reviewer",
  "priority": "direct_user",
  "attempt": 1,
  "deadline_ms": 4000,
  "received_at_ms": 1785370000000,
  "last_id": 28145,
  "event": {}
}
```

Required fields are `protocol_version`, `delivery_id`, `server`, `room`,
`identity`, `priority`, `attempt`, `deadline_ms`, and `event`.
`delivery_id` is stable across retries. An adapter must reject an unsupported
protocol version explicitly; it must not silently interpret it as v1.

Priority order is:

1. `security_control` — stop, room close, or security action;
2. `direct_user` — an exact user target or exact `@identity`;
3. `care_signal` — one bounded goal/task/wait/silence attention event;
4. `explicit_task`;
5. `lead_room` — whole-room traffic delivered because this identity is lead;
6. `broadcast` — `target=all` or exact `@all`;
7. `addressed_agent`;
8. `informational`.

Controls, care signals, and tasks are never compacted. A direct user call may
bypass older chat, but ACKing it must not erase an older protected event.

## Process environment

The single-flight command consumer passes the full envelope on stdin and in
`LOCA_DELIVERY`. For backward-compatible event access it also exports:

| Variable | Meaning |
| --- | --- |
| `LOCA_MSG` | The envelope's `event` JSON |
| `LOCA_DELIVERY_ID` | Stable delivery identity |
| `LOCA_OP_ID` | Stable reply key, `loca-<delivery_id>` |
| `LOCA_ATTEMPT` | Current adapter attempt |
| `LOCA_PRIORITY` | Priority class |
| `LOCA_PROTOCOL_VERSION` | `1` |
| `LOCA_ROOM` | Loca name |
| `LOCA_FROM` / `LOCA_SENDER` | Unique senders |
| `LOCA_TARGET` | First message target |
| `LOCA_TEXT` | Message text joined in turn order |

Credentials are not part of the envelope and must not be copied into command
arguments or logs.

## Shared outcome context

When the receiving identity is the loca's lead, every runtime adapter must add
the active Goal to the **same** invocation context before the agent acts. Goal
is orientation, not a delivery: creating or updating it never starts a model
turn by itself. Codex, Claude Code, a generic command, and future runtimes all
implement this same boundary in their own context provider.

An adapter should expose either `context.goal` as the active Goal object or an
equivalent bounded text item containing its `outcome` and optional
`checkpoint`. A temporary Goal lookup failure must not discard the triggering
message or create a second invocation.

## Delivery and ACK

The invariant is:

```text
WebSocket → durable inbox → priority selection → one runtime turn
          → idempotent reply → successful completion → ACK
```

- The listener cursor means **received durably**, not processed.
- The worker cursor means **runtime completed and ACKed**.
- A non-zero exit, timeout, crash, or unsupported capability leaves the
  delivery unacknowledged for retry.
- Only one consumer owns an identity. Adapters are single-flight by default.
  A runtime that explicitly enables native direct-user preemption may briefly
  overlap the yielded adapter with the urgent adapter solely so the latter can
  interrupt the former; the yielded record remains unacknowledged.
- A retry uses the same `delivery_id` and `LOCA_OP_ID`.
- The room message API deduplicates the stable `op_id`; at-least-once runtime
  execution therefore produces at most one room reply.

## Capabilities and health

An adapter declares these states:

- `delivery`: can consume protocol v1;
- `wake`: can start or resume runtime work;
- one of `resume` or `new_turn`;
- `cancel`: can stop an active turn, or explicitly `unsupported`;
- `ack`: ACKs only after completed work;
- `reply`: can post with the stable operation id, or `unverified`;
- `health`: reports delivery, wake, reply, ACK, and version separately.

Unsupported capabilities are `DEGRADED`, never silently `OK`.

Runtime-specific behavior:

- **Claude Code Monitor**: Monitor owns the foreground listener supervisor and
  native wake; the supervisor owns exactly one listener child. The Monitor
  task lifecycle is the wake/ACK boundary.
- **Interactive Codex**: a session router forwards the durable envelope to one
  persistent worker with `followup_task`; the router ACKs after that worker
  completes.
- **Headless Codex**: the v1 consumer invokes `nudge.py`, which owns app-server
  `initialize → thread/resume → turn/start|turn/steer → turn/completed`.
- **Generic command**: the v1 consumer writes the envelope to command stdin and
  ACKs only on exit zero.

## Conformance scenarios

Every adapter must demonstrate:

1. one direct mention produces one runtime turn;
2. 68 old broadcasts do not delay a new direct user call beyond four seconds;
3. pending stop/task survives chat compaction;
4. non-zero exit is retried with the same delivery and operation IDs;
5. a 500 on first reconnect backfill is retried without another network event;
6. runtime work lasting 90 seconds does not stop WebSocket ping/pong;
7. process restart replays unacknowledged work;
8. duplicate delivery produces no duplicate room reply;
9. lead mode receives whole-room messages even if normal mode is direct-only.
