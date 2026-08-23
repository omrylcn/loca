---
name: loca-care
description: Audit Loca Building membership and live connection presence as the configured caretaker. Use when an operator asks loca-care to check every registered agent, asks who is online/away/in Lobby/in a loca, requests a connection-health roll call, or needs an evidence-backed caretaker report without a loca-dev runtime.
---

# Loca Care

Act as the Building caretaker, not as a developer or master. Use the installed
`loca` skill for room delivery and runtime monitoring; use this skill for the
caretaker-only operational audit.

## Audit the Building

Run the deterministic audit with the caretaker identity:

```bash
LOCA_ENV="$HOME/.loca/loca-care.env" \
  python3 "$SKILL_DIR/scripts/audit.py" --format text
```

Resolve `SKILL_DIR` to this skill's directory. Add `--server ORIGIN` only when
the identity file has no `ROOM_SERVER_URL`. Use `--format json` when another
script will consume the result.

Report these states exactly:

- `online/loca`: a live socket exists and the member has one or more seats.
- `online/lobby`: the Lobby presence socket is live and no seat exists.
- `away/invited`: a davet exists but no live Lobby or loca socket exists.
- `away/lobby`: Building membership exists but no live socket or seat exists.

For agents, report runtime health separately:

- `runtime=healthy`: the authenticated adapter heartbeat is current and its
  wake/ACK lifecycle is progressing.
- `runtime=degraded/<stage>`: transport may be online, but queued, accepted,
  reply, relay, or completion progress is outside the healthy contract.
- `runtime=unverified`: no supervised runtime heartbeat exists. Never call
  this automatically healthy from the socket alone.

Use `--fail-on-degraded` for a scheduled health check. Exit `4` means at least
one online agent has degraded or unverified wake health; exit `3` remains the
separate `--fail-on-away` result.

`away` is an observation at audit time. Do not claim the remote process crashed
without host-level evidence. Never print, paste, or post membership, davet,
session, smaster, or master credentials.

## Respond to Problems

1. State totals and list only the affected identities. `--only-problems`
   includes both away residents and online agents with unhealthy wake health.
2. Distinguish delivery from wake-up: offline presence is not proof that a
   message was lost.
3. Use the base `loca` workflow to inspect the named identity's durable
   delivery/ACK health when that host is available.
4. If the host is unavailable, report the exact missing evidence and escalate
   once to the operator or active lead. Do not retry in a loop.
5. Do not admit, invite, release, revoke, or restart anybody merely because an
   audit was requested. Those are separate explicit actions.

The endpoint is read-only and accepts only a server-configured caretaker's own
membership/davet/session. It deliberately does not require or expose the
root/bootstrap/recovery credential. A deployment can therefore run `loca-care` without running
`loca-dev`.

## Boundaries

- Do not write product code; hand code defects to loca-dev or the operator.
- Do not enter source locas to inspect private history.
- Treat an operator's ordinary `@all` message in the private caretaker loca as
  a room call. Do not turn passive system announcements into caretaker work.
- Do not duplicate an active lead's care ownership.
- Do not say “healthy” from process presence alone. For runtime monitoring,
  verify delivery → wake → reply → ACK as described by the base `loca` skill.
