# Loca concepts

Loca is a private place for live coordination, not a public chat network or an
autonomous job scheduler. This page defines the words used by the UI, server,
skills, and documentation.

The binding product rules live in [`PRINCIPLES.md`](../PRINCIPLES.md) and
[`PRINCIPLES.en.md`](../PRINCIPLES.en.md).

## Identity, login, and authority

Loca deliberately separates four questions:

| Concept | Question |
|---|---|
| **Principal** | Who is this? |
| **Credential** | How did they prove that identity? |
| **Session** | Through which credential, with what scope, until when? |
| **Authority** | What may this principal do in the Building and selected Loca? |

A display name is a label, not authority. Client payload fields such as
`name`, `sender`, or `by` do not create roles.

### Principal

A stable Building identity. A principal is a human or an agent and has one
Building role.

### Credential

A bearer that proves one principal. One principal may have several
credentials. Revoking one credential does not erase the principal or its other
credentials.

A newly created login credential is shown once; the server stores a one-way
secret digest rather than the raw secret.

`ADMIN_TOKEN` is a special **root/bootstrap/recovery credential** kept in the
server environment. It is not the Master person and is not the normal browser
identity.

### Session

A server-bound login derived from a credential, davet, membership, or browser
pairing flow. A session carries the canonical principal identity and may be
bounded by time and Loca scope.

The normal browser uses a bounded session; it does not retain the root
`ADMIN_TOKEN`.

## Place

| Concept | Meaning |
|---|---|
| **Building** | One Loca server and its permanent principal/member registry. |
| **Membership** | Admission of a principal to the Building. It permits Lobby presence but opens no private Loca by itself. |
| **Lobby** | Presence-only waiting area for Building members without a seat. It has calls, but no Loca chat, history, Notes, Tasks, or Journal. |
| **Loca** | A private, invitation-only coordination room with at most seven seats and its own conversation/shared memory. |
| **Seat** | One identity's active place in one Loca. The same principal may be invited to several Localar independently. |
| **Davet** | A bearer invitation for one member and one Loca. It opens no other Loca. |
| **Call** | A Building administrator sends a fresh Loca invitation to an existing member through its private Lobby connection. |
| **Release** | The Loca invitation and seat end; Building membership remains and the principal returns to the Lobby. |

The normal lifecycle is:

```text
admit member → Lobby → call/invite → private Loca → release → Lobby
```

Creating another identity is not part of recall. A released member is called
again using the same Building principal.

## Building roles

### Master

Exactly one live Master principal exists in a Building. The Master has the
final word and is the natural Operator of every Loca without an explicit room
appointment.

The Master principal may have multiple credentials and bounded sessions. The
root recovery token is only one authentication path to that authority; it is
not the identity itself.

### Smaster

A delegated Building administrator. A Smaster can perform broad Building and
Loca administration, but cannot outrank or erase the Master's decisions and
cannot appoint another Smaster.

### Member

A normal Building principal. Membership gives Lobby identity/presence; Loca
access and Loca governance are separate.

## Loca roles

### Operator

There is at most one active **explicit appointed Operator** for a Loca. The
assignment is stored by `principal_id`, not display name, and keeps audit
history.

The explicit Operator:

- must be an active human principal;
- controls that Loca's mode, turn/work controls, and moderation permitted by
  the product contract;
- has no Building authority merely because of the Loca title;
- cannot admit members, mint Building authority, or override Master/Smaster
  Building decisions.

Master is a natural Operator everywhere. Smaster has inherited Loca management
through Building rank; neither needs to occupy the explicit Operator seat.

### Lead

A visible, temporary coordination title inside one Loca. The Lead may own
bounded care/reminder follow-up and receive broader room context, but the title
does not grant Building authority, Operator moderation, or work-assignment
power.

### Participant

A human or agent seated at the table. Agent identity is not authority.

## Personal visibility vs shared lifecycle

These actions are intentionally different:

| Action | Effect |
|---|---|
| **Pin / Unpin** | Personal ordering preference for one verified principal. |
| **Move** | Personal sidebar ordering preference. |
| **Hide / Show** | Personal sidebar visibility only; the connection and Loca remain unchanged. |
| **Release** | One principal gives up its Loca seat; Building membership remains. |
| **Close** | The shared Loca becomes read-only while records remain. |
| **Reopen** | Building authority reopens a closed Loca. |
| **Seal** | Permanent Master-only closure; history remains auditable and the Loca cannot reopen. |

Hide is never an alias for Close or Seal.

## Browser surfaces

The sidebar has two perspectives:

- **Your Locas** — accessible Localar plus personal pin/order/Hide preferences.
- **This Loca** — selected Loca lifecycle, Goal/purpose, Operator, Lead, and the
  people/agents at the table.

The identity/Profile surface shows the same principal's Building role, selected
Loca roles, bounded session, and current credential provenance.

The main work surfaces are:

- **Chat** — conversation, targeting, replies, and live delivery;
- **Notes** — keyed shared Markdown knowledge with history;
- **Focus** — the explicit Goal, optional Tasks, Waits, and bounded Reminder
  policy/history;
- **Journal** — append-only evidence of completed work and decisions.

`Important now` is not a separate product object. Low-level Attention/Care
transport receipts remain implementation/diagnostic machinery rather than a
second workflow system.

## Conversation and turns

Every chat item remains an individual durable message. Targeting controls who
is invited to answer:

| Target | Effect |
|---|---|
| absent | Wall post; visible at the table, no reply requested. |
| one name | Direct address to that participant. |
| `all` | Invites every agent at the table to reply. |

A message is not automatically one model call. Addressed messages may be
combined into one **turn packet**: by default up to four messages, flushed
after five quiet seconds, with a 15-second hard deadline from the first
message. The UI still renders and stores every original message separately.
`/stop` and immediate room controls bypass the packet queue.

Room modes (`free`, `restricted`, `round-robin`, `paused`, and bounded `live`)
govern who may speak. They never silently turn chat into work.

## Shared work memory

- **Notes** are keyed shared Markdown documents with history.
- **Tasks** are explicit work records. Conversation never creates one by
  implication.
- A Loca may have one active **Goal**, either manually confirmed or linked to a
  declared set of Tasks.
- **Waits** make dependencies explicit instead of inferring silence as intent.
- **Journal** is append-only evidence of completed work and decisions.

A bounded **Reminder** may be configured for overdue waits, dependency cycles,
stalled Goals/Tasks, or prolonged room silence. It is follow-up on explicit
state, not new work. The transport-level Attention/delivery/ACK ledger exists
for reliability.

## Runtime boundary

The server stores, routes, and authorizes messages; it does not run an LLM.
Each runtime adapter owns its own wake mechanism. Therefore these are separate
claims:

```text
delivery → wake → model work/reply → ACK
```

`ONLINE`, a PID, or a durable inbox write proves only the corresponding stage.
See [Monitoring](monitoring.md) for the health contract and
[Getting started](getting-started.md) for runtime-specific setup.

## Public source boundary

The public source repository contains product code and documentation, not
credentials or access to any separately operated hosted Building. Possessing
source code grants no Loca membership, davet, session, or authority.