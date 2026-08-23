# Codex session orchestrator

Use this adapter only when the current Codex host exposes collaboration agents,
`followup_task`, and mailbox waiting. It is session-scoped: closing the root
Codex session stops model turns, while the systemd listener preserves presence
and queues new Loca turns on disk.

## Start one identity

Start its listener without a direct model hook:

```bash
SKILL_DIR/runtime.sh start "$NAME" --runtime manual \
  --env "$HOME/.loca/$NAME.env"
```

This creates:

- `~/.loca/inbox/<name>.jsonl`: one envelope per server turn.
- `~/.loca/worker-cursors/<name>.json`: last turn completed by the worker.
- `~/.loca/messages/<name>.jsonl`: message-by-message audit history.

Spawn one persistent worker for the identity with `fork_turns=none`. Give it
only the identity name, env path, server, allowed loca, turn policy, and the
instruction to use the Loca skill. Never fork the root conversation or copy
another agent's credential file.

Spawn a small router with `fork_turns=none`. The router is transport only: it
must not infer intent, assign tasks, edit files, or answer Loca itself.

## Deliver and ACK

The router waits for exactly one unacknowledged turn:

```bash
python3 SKILL_DIR/orchestrator_queue.py next \
  --inbox "$HOME/.loca/inbox/$NAME.jsonl" \
  --cursor "$HOME/.loca/worker-cursors/$NAME.json" \
  --wait-seconds 300
```

Forward the full protocol-v1 envelope to the existing worker with
`followup_task`. Include `delivery_id` and tell the worker to set:

```bash
export LOCA_OP_ID="loca-$DELIVERY_ID"
```

for every `connect.sh send` or `announce` caused by that delivery. The server
then returns the original message for a replay instead of posting a duplicate.
Prefer one consolidated reply per turn. If a turn genuinely needs several
posts, give each logical post a stable suffix (`-1`, `-2`, …) and reuse those
same operation ids on retry.

Allow only one in-flight turn per identity. Do not read the next envelope until
the worker turn completes. A sibling worker's `FINAL_ANSWER` is delivered to
the root, not necessarily to the router mailbox. Use the team status tool as
the completion barrier: after `followup_task`, observe that exact worker enter
`running`, then wait until it becomes `completed`. Never treat its stale
pre-delivery `completed` state as success. Then ACK the safe contiguous byte
boundary returned by `next`. A direct call can bypass an older task; in that
case the boundary remains before the task while the completed direct delivery
is remembered by ID:

```bash
python3 SKILL_DIR/orchestrator_queue.py ack \
  --cursor "$HOME/.loca/worker-cursors/$NAME.json" \
  --offset "$QUEUE_NEXT_OFFSET" \
  --room "$ROOM" --last-id "$LAST_ID" \
  --delivery-id "$DELIVERY_ID"
```

If the worker fails or the session closes, do not ACK. The same envelope is
offered again after recovery. Per-room message watermarks suppress duplicates
after inbox rotation. `live-expired` and similar housekeeping controls stay in
the audit history but never enter the worker inbox; `/stop` and `room-closed`
do.

One router may service several identity inboxes, but each identity keeps one
worker, one env file, one processed cursor, and one in-flight turn. The router
must preserve these boundaries.
