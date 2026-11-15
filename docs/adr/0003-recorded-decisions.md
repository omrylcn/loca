# ADR 0003: Why existing decisions are the way they are

- Status: Accepted as a record of decisions already in the code. Entries that
  say "Reason not recorded" are waiting for the owner to supply the reason.
- Date: 2026-11-14
- Scope: contributors and reviewers, human or agent

## Context

Loca is changed by evolving what is there, not by rewriting it. Several
decisions keep being proposed for removal in review because the reason for
them is written nowhere a newcomer would look. This record gives each one in
the same shape: what the decision is, where it lives in the code, why it was
made, and what it costs. Line numbers were checked against `master` at
`f85c0cc`.

Where no reason was found in `PRINCIPLES.md`, `DESIGN.md`, an ADR, a commit
message or `CHANGELOG.md`, the entry says "Reason not recorded" and describes
only what the code implies. It does not supply a reason of its own.

Before proposing to change one of these decisions, read its entry. If the
change still makes sense, say in the pull request which entry it touches and
why the cost has changed.

## Decisions

### 1. Identity names are ASCII only

**Decision.** A display name or identity name is 1 to 64 characters from ASCII
letters, digits, `-`, `_` and `.`. No door admits a new member named `all`, in
any letter case, because `all` is an address. A name checked by
`validate_display_name` also may not be a rank, title or addressing word
(`master`, `smaster`, `operator`, `lead`, `admin`, `root`, `all`, `everyone`,
`here`, `goal`). The admission doors refuse only `all`: titles stay ordinary
member names there. Names are compared without regard to letter case: no
door admits a new member, and no Smaster is created or renamed, under a name
that a live member, the Master, a Smaster or a configured caretaker already
wears in any letter case. A caretaker's own exact name is how that caretaker
is admitted, so the doors let it through.

**Where.**
- `crates/server/src/hub.rs:529-608` (`RESERVED_NAMES`, `ADDRESS_WORDS`,
  `is_address_word`, `valid_agent_name`, `admission_name_refusal`,
  `validate_display_name`). `validate_display_name` serves Smaster codes
  (`routes/keys.rs:76`, `routes/membership.rs:494`) and the Master/Smaster
  profile rename (`hub.rs:4687`), and refuses the `RESERVED_NAMES` and the
  `ADDRESS_WORDS`. `admission_name_refusal` is the three admission doors'
  rule: join requests apply it to the name they file
  (`routes/membership.rs:858`); direct admission and the loopback master
  desk (`routes/membership.rs:276`, `main.rs:875`) leave it to
  `Hub::admit_member` (`hub.rs:1813`), which first looks for an existing
  member by that exact name and returns it if found, and otherwise applies
  the rule; join-request approval applies it again (`hub.rs:2953`). Each door
  refuses with the same sentence, 400 at admission and 409 at approval.
- `crates/server/src/hub.rs:2004-2056` (`new_member_name_clashes`,
  `rank_name_clashes`, folded with `store::search_fold`): a name worn in
  another letter case is refused with 409 and one sentence
  (`NAME_WORN_RULE`, `routes/membership.rs:815`) at the three admission
  doors (`Hub::admit_member` for direct admission and the desk; the join
  request in `hub.rs:2832`), with 409 at both Smaster doors
  (`Hub::add_smaster`, `hub.rs:1407`), and with 409 at the profile rename for
  a caretaker's name (`hub.rs:4703`; the rename already refused another
  member's or rank's name in any case). A join request is also refused while
  a request in another letter case waits (`store.rs:1057-1061`, SQLite
  `lower()`). Approving one admits a new name, so the approval meets both
  rules again, with 409 before anything is written
  (`Hub::approve_join_request`, `hub.rs:2932`); the members table is
  re-checked inside the approval's transaction too (`store.rs:1135`). The
  lookup of an existing member, each of these checks and the write happen
  under one lock, `Hub::name_claims` (`hub.rs:310-323`, where the lock order
  is stated), so requests racing for names that fold together let one in,
  and a revoke between a door's request and its write cannot let a name the
  rule refuses through.
- `crates/server/src/routes/access.rs:31-39` (`valid_identity_name`): a named
  Reminder recipient (`routes/rooms.rs:323`).

**Why.** The only recorded reason for the characters is the code comment at
`hub.rs:590-592`: the name follows "the join-request rule, so it survives
`@mention`". The web composer completes a mention with `/@[\w-]*$/`
(`web/assets/api.js:83`) and recognises one addressed to the reader with a
`\b` boundary (`web/assets/chat.js:294`); in JavaScript both match ASCII
word characters only. The admission doors applied that rule to join requests
only until direct admission and the desk were brought in line, so that no
door admits a name another refuses. `all` is refused at every door because
a `target` of `all` or an `@all` in the text calls everyone at the table
(`main.rs:1366`, `main.rs:1376`), so a member named `all` could not be
addressed alone: a message meant for them woke every agent in the loca. The
reserved words follow `docs/concepts.md` "Names and titles": a name says
who, a rank says what. They are not refused at admission: the web client's
default name is `operator` (`web/assets/state.js:4`), and members named
`operator` are admitted in use and throughout the tests. No principle, ADR
or CHANGELOG entry records the ASCII choice itself. Letter case is ignored
because the server and both clients ignore it when they find who a message
calls: `@Bob` in the text wakes `bob` and `Bob` alike (`main.rs:1370-1378`,
`chat.js:294`, the kit's `addresses()`), and a `target` or `@` word naming a
caretaker in any letter case is relayed to that caretaker in Care with the
message as its context (`hub/attention/addressing.rs:8-41`). A member
admitted as `LOCA-CARE` received the messages meant for it, and the
caretaker `loca-care` received each of them too, from another loca.

**Cost.** People whose names use other letters (`Ayşe`, `Ömer`) sign in
under an ASCII spelling. The rule applies to a name being admitted now: a
member admitted earlier under a name the doors now refuse (a space, a
non-ASCII letter, `all`) is not re-checked and keeps that name. A member can
be admitted under a title (`operator`, `lead`) that a rename refuses, but not
under the Master's display name in any letter case: while the Master is
called `Master` (the default), no new member is named `master`. Members an
older server admitted under names that differ only by letter case are kept:
`@bob` still wakes both, and a member stored as `LOCA-CARE` still has every
message addressed to it relayed to the caretaker as well. The join-request
door asks for no credential, so its 409 now also tells a caller that a rank
or a caretaker wears a name, as it already told that a member did. A
memory-only Building stores no profiles, so there the Master's display name
is not checked. The server's check for whether a message names people
(`hub.rs:5948-5979`, used at `hub.rs:3449` to narrow an agent's `@all`)
accepts Unicode letters, which the web client's patterns do not.

### 2. The `bb-` prefix belongs to birebirs

**Decision.** A direct message's loca id is `bb-` followed by a hash of its two
principal ids. No ordinary loca may have an id that starts with `bb-`.

**Where.**
- `crates/server/src/hub/dm.rs:103-119` (`DM_PREFIX`,
  `dm_room_id`; the hash is `store.rs:1510-1513`).
- The door refuses the prefix for anything that is not a direct message:
  `hub.rs:1521-1526`. Creation refuses it: `hub.rs:4220-4222`. Rename refuses
  it as source or target: `store/rooms.rs:99`. Group moves refuse it:
  `hub/groups.rs:909`, `hub/groups.rs:1066-1067`. Extensions treat it as a
  direct message: `hub/extensions.rs:107`, `:115`, `:147`.

**Why.** PRINCIPLES "Birebir" (`PRINCIPLES.tr.md:300-303`, en `336-339`): a
direct message is bound to identity, not to names, and "the loca id carries no
name". The code adds two reasons: the id carries no name "so a rename never
moves it and a URL never reveals who talks to whom" (`hub/dm.rs:114-115`),
and the id space is reserved "so a pair's room can never be squatted with
someone else's invites in it" (`hub.rs:1521-1523`). `DESIGN.md:151-152` and
CHANGELOG 0.13.0 (the `ROOM_RENAME` fix) restate the reservation.

**Cost.** No one can name an ordinary loca `bb-…`. Because the door refuses
the prefix before anything else, an ordinary loca created under such a name
before direct messages existed would be unreachable; no migration for that case is
in `crates/server/src/store`. The check "is this a direct message" is made in two
ways, by the stored pair (`is_dm`) and by the prefix; several of the
places listed above ask both.

### 3. `messages.principal` is the scope of `op_id`, not the writer

**Decision.** The `principal` column of `messages` holds the key under which a
client's `op_id` is unique. A retry with the same `(room, principal, op_id)`
returns the first message instead of storing a second one. The column does
not say who wrote the message. The writer is a separate column,
`messages.sender_principal`, the verified principal id (PR #133:
`crates/server/src/store.rs:1552-1586`, `crates/protocol/src/lib.rs:71`).
This entry describes `principal` only.

**Where.**
- Column and unique index: `crates/server/src/store.rs:717`, `:732-736`.
- Written only when the message has an `op_id`: `hub.rs:3434`,
  `store/messages.rs:133-157`.
- Looked up before storing, in `Hub::post`: the database, under the key and
  then under the older REST key, and memory, under the key. A record found
  under the older key answers the re-send only when it is provably the
  sender's (`Hub::legacy_operation_owner`): it names the sender as its
  writer, or it names nobody and its key is a membership, or the sender is
  unverified and is its label. A display name proves nothing for a verified
  sender, because names are shared, re-admitted and renamed without a
  history. A record nobody can be shown to own is answered with an explicit
  conflict, never with silent success and never by posting again.

The value is the sender, built by `Hub::operation_scope` for both
transports: the REST route (`post_message` in `routes/content.rs`) and a
WebSocket `send` frame (`ws_session` in `main.rs`). It is `pr:<principal id>` when the door
verified a principal: the session's; without a session, the invite's or the
membership credential's; else the one behind an admin credential (the root
key resolves to the Master, a Smaster code to the Smaster). Otherwise it is
`mb:<membership id>` (the membership's `mbid_`) when a membership names the
sender with no principal (a memory-only store), and `label:user:<name>` or
`label:agent:<name>` when nothing verified the sender.

Until this version the value depended on the transport. Over REST it was
`mb:` followed by the membership key for a session taken with an invite, and
otherwise `user:<display name>` or `agent:<display name>` (a session taken
with a membership key, a Master or Smaster session, or no session, where the
name was the body's `sender`). Over the WebSocket it was the connection's
seat identity (`Hub::seat_identity`): `sm:pr:<principal id>`, `@master`,
`mb:<membership key>`, `dv:<invite>`, `sm:st:<hash>`, `st:<hash>` or
`name:<display name>`. Rows keep the value they were stored with, except
that the open rewrites `mb:<membership key>` to `mb:<membership id>` and
`dv:<invite>` to `dv:<invite id>` (`rewrite_key_scopes` in `store.rs`). A post
also looks for the older REST value built from its own request, and answers
with that row unless it names a different writer in `sender_principal`. An
older WebSocket value is found only where it equals that REST value
(`mb:<membership key>` under an invite session, found as `mb:<membership id>`
since the rewrite; an invite session names its membership by that id).

**Why.** Commit `c4014d5` ("feat(protocol): make message retries
idempotent", 2026-07-24) added the column and the index. Its note quotes
PRINCIPLES: a message received twice must not produce its side effect twice
(`PRINCIPLES.tr.md:43-46`, en `48-52`). The column was named before the
`principals` table existed (commit `24cd31b`, 2026-08-23), so "principal"
there meant "whoever sends", not the later Principal concept. That naming is
not recorded as a decision.

**Cost.** The name suggests the writer's principal id. For a verified
sender it now holds that id with a `pr:` prefix, but it is still the scope,
not the writer: `sender_principal` is. An operation stored under any other
older WebSocket value is not recognised after the upgrade, so re-sending it
posts once more. Rows stored before identifiers stopped carrying keys under
`mb:<membership key>` (an invite session over REST; an invite, an invite session or
a membership credential over WebSocket) or `dv:<invite>` held a usable key in
clear text; the open now rewrites them to the id form, in one transaction,
with `secure_delete` and the owed WAL checkpoint, and new rows never hold a
key. Two other columns are still built the way REST built this one, and are
rewritten the same way: `message_reactions.principal` is
`mb:<membership id>` for an invite session and
`reactor:<name from the request body>` otherwise (`routes/content.rs`);
`attachment_pending.uploader` is `mb:<membership id>` for an invite session,
`user:<name>` or `agent:<name>` for any other session, and `anon` without
one (`routes/attachments.rs`).

### 4. One group per loca, many groups per person

**Decision.** A loca belongs to at most one group; a loca with no group is in
the default group. A person may belong to any number of groups.

**Where.**
- `crates/server/src/store/groups.rs:62-74`: `group_members` is keyed by
  `(group_id, principal_id)`; `group_rooms` is keyed by `room` alone.
- `crates/server/src/hub/groups.rs:22-30` (`GroupBook`: one group id per room)
  and `hub/groups.rs:284-288` (`group_gate`: entry is decided by that one
  group's members).

**Why.** PRINCIPLES "Groups" (`PRINCIPLES.tr.md:435-458`, en `481-506`) says a
person may be in more than one group and describes a group loca as belonging
to one group: only that group's members see and enter it, and entry is "the
same rule at every door". `DESIGN.md:164-165` states "a Loca is bound to one
group". Reason not recorded for why a loca may not be shared by two groups.
What the code implies: entry to a group loca is one membership check against
one list, so the door has one answer and nothing to reconcile.

**Cost.** Work shared by two groups needs one group that contains everyone,
or a loca in the default group reached by invite. Moving a loca between groups
is Master-only and revokes invites of people outside the new group
(`docs/concepts.md` "Groups").

### 5. A Goal is optional

**Decision.** A loca may exist and be used without any Goal. When there is
one, there is at most one active Goal, set explicitly by the Operator.

**Where.**
- Opening a loca takes no Goal: `hub.rs:4210` (`create_loca_in`).
- Focus, which carries Goal and Tasks, is not ticked for new locas:
  `crates/protocol/src/lib.rs:338-344` (`default_on_new: false`).
- The Lead rule applies only while a Goal is active: `hub.rs:3699-3707`.

**Why.** `DESIGN.md:147` says "Goal isteğe bağlıdır" (a Goal is optional).
It was written in commit `9bd762d` (2026-11-14) as the operator's direction,
without a reason. Reason not recorded. What the texts imply: a Goal is the
"why" of a loca and Tasks are optional paths (`PRINCIPLES.tr.md:64-76`, en
`74-87`), and the English text says a loca has "at most one active goal".
Nothing in PRINCIPLES makes a Goal a condition for talking, which is the
product boundary (`PRINCIPLES.tr.md:478-481`, en `528-531`).

**Cost.** A loca without a Goal gets no Goal reminders and no Goal context in
the Lead's turn (ADR 0002). With Focus off, Goal and Task routes answer 403
and no Goal or Task follow-up is generated (CHANGELOG 0.13.0, Fixed: "Do not
generate automatic follow-up while the Focus extension is disabled").

### 6. Seven seats by default

**Decision today.** Every loca seats at most `LOCA_CAPACITY` identities,
humans and agents alike; the default is 7 and the value is clamped to 2–32.
Watchers take no seat (`hub.rs:2994-2995`), and a direct message is not counted
toward loca capacity (`DESIGN.md:158-159`).

**Decision for the next step (owner, November 2026).** The seat count is
chosen per loca, within the Building-wide `LOCA_CAPACITY` (default 7). Seven
is the default ceiling, not a fixed size for every loca. This is not
implemented yet: `RoomSettings` has no seat-count field
(`crates/protocol/src/lib.rs:223`).

**Where.**
- `crates/server/src/hub.rs:2992-3015` (`DEFAULT_LOCA_CAPACITY`, bounds,
  `set_loca_capacity`), read from the environment at `main.rs:261-265`.
- Enforced when an invite is issued (`hub.rs:2128-2141`) and when a seat is
  taken (`hub.rs:3059-3063`). Capacity counts identities, not names
  (`PRINCIPLES.tr.md:278-282`, en `307-311`).

**Why.** Commit `760beff` (2026-07-19) set the number to seven: a loca is by
the meaning of the word a small place; anything larger is a different place
(a "salon", never built); and in practice a crowded room cannot talk without
mentions and live mode stops making sense. Commit `1507b85` repeats this when
invites were introduced. The code comment at `hub.rs:2992-2995` points to
"DESIGN §5g", which no longer exists in `DESIGN.md`. Why the number became
configurable is not recorded.

**Cost.** Today one number applies to every loca in the Building. Lowering it
refuses new seats but never takes away seats already held (`hub.rs:3009-3010`).

### 7. The extension catalog is compiled in

**Decision.** A loca's optional tabs (Notes, Focus, Journal, Loca Wiki, Live)
are switched on or off per loca from a fixed catalog that ships in the
server. Chat is always on and is not an extension. A loca saved before
extensions existed keeps the frozen legacy set of all five.

**Where.**
- Catalog and legacy set: `crates/protocol/src/lib.rs:327-370`; the per-loca
  set is `RoomSettings.extensions` (`lib.rs:307`, `lib.rs:386-403`).
- Changing the set: `crates/server/src/hub/extensions.rs:136-225`.
- Refusal of a switched-off extension, reads and writes alike:
  `crates/server/src/routes/extensions.rs:16-24` (route segment to extension)
  and `:46-75` (`extension_fence`).

**Why.** PRINCIPLES "Loca extensions" (`PRINCIPLES.tr.md:382-431`, en `427-477`):
no code is loaded or run, an extension "turns on a capability written and
tested inside the server"; a switched-off extension is refused, not hidden,
and removing one deletes nothing; existing locas keep what they had
(`PRINCIPLES.tr.md:410-411`). Reason not recorded for which extensions are ticked
by default (Notes, Journal, Loca Wiki on; Focus and Live off).

**Cost.** A new extension needs a server release. A new route under
`/rooms/:id/<segment>` that belongs to an extension must be added to
`routes/extensions.rs:16-24`, or it is not refused when the extension is off.
Switching the Journal off also hides the record of extension changes (see
"Mandatory records and the working Journal" below).

### 8. Scopes: what each credential and session reaches

This entry reads "scopes" as the reach of credentials and sessions. If the
plan meant another kind of scope, this entry needs the owner's correction.

**What PRINCIPLES says.** "Üyelik anahtarı hiçbir locanın kapısını açmaz;
yalnız bu hatta kimliği kanıtlar." (`PRINCIPLES.tr.md:238-239`; en `263-265`:
"The membership key opens no loca door; it proves identity only on that
connection.") The three credential layers "are not interchangeable": the
Master/Smaster key proves Building authority, membership makes an agent
reachable in Lobby, a loca invitation seats it in one named loca
(`PRINCIPLES.tr.md:247-253`, en `273-280`). For groups: "entering a loca =
invitation **or** group membership; for a group loca, group membership only"
(`PRINCIPLES.tr.md:453-454`, en `501-502`).

**What the code does today.** One function, `Hub::enter_decision`
(`crates/server/src/hub.rs:1483-1599`), decides every door: REST
(`routes/access.rs:118-124`, `routes/rooms.rs:637-649`) and the WebSocket
handshake (`main.rs:1164-1170`). It reads three inputs: the `x-admin-token`
credential, the `x-room-token` credential, and the session. Over REST the
`x-room-token` slot falls back to `x-admin-token` when it is absent
(`routes/access.rs:72-77`); on the WebSocket both arrive as subprotocol
credentials, without that fallback. A credential in `x-admin-token` is
resolved to its principal (`hub.rs:1189-1193`), whatever its role. The
branches run in this order:

1. A session confined to another loca is refused (`:1447-1449`).
2. Direct message: admitted if the session's principal or the `x-admin-token`
   principal is one of the pair (`:1453-1459`), or the `x-room-token` is an
   invite for this direct message held by one of the pair (`:1462-1466`); otherwise
   refused. No rank and no ban check.
3. Any other `bb-` id is refused (`:1473-1475`).
4. Group loca: `group_door` (`:1575-1610`). The Master is admitted by rank
   (`:1584-1590`). Anyone else is admitted only if one of these principals is
   a member of the loca's group: the session's (unless confined to another
   loca), the `x-admin-token` credential's, or that of the member holding an
   invite for this loca in `x-room-token` (`:1591-1603`); then bans apply.
5. Ordinary loca: a Building admin credential (Master or Smaster) in
   `x-admin-token`, or a Master or Smaster session, is admitted
   (`:1489-1494`). Then bans apply (`:1496-1501`). Then, in order: the
   principal of the session or of the `x-admin-token` credential is this
   loca's Operator (`:1503-1510`); the `x-room-token` is an invite for this loca,
   or passes the legacy building-key check (`:1514-1516`, `may_enter`
   `:1399-1416`, `is_member` `:1354-1365`); the session's principal holds a
   live invite for this loca (`:1520-1524`); the session was taken for this
   loca (`:1529`), or has no loca while the legacy `ROOM_TOKEN` is set
   (`:1542`). Otherwise refused.

What that gives, credential by credential, in a closed Building
(`ADMIN_TOKEN` set, `REQUIRE_INVITE=1`, no `ROOM_TOKEN`):

| Credential at the door | Ordinary loca | Group loca | Direct message |
|---|---|---|---|
| Root key (`ADMIN_TOKEN`) | admitted (`:1489`; in `x-room-token` through `is_member`, `:1360-1361`) | admitted (`:1584-1586`) | only through the principal the key resolves to, if it is one of the pair |
| Master session | admitted (`:1492`) | admitted (`:1587`) | only as one of the pair |
| Smaster code (`sm_`) or Smaster session | admitted (`:1489-1494`) | only as a member of the group | only as one of the pair |
| Membership key (`mb_`) in `x-room-token` | refused (`may_enter` gives `is_member` the membership, which is not the building key) | refused (no invite for this loca) | refused |
| Membership key (`mb_`) in `x-admin-token` | only where its principal is the Operator (`:1503-1510`) | if its principal is in the group (`:1598`) | if its principal is one of the pair (`:1456`) |
| Session taken with a membership key | where its principal is the Operator, or holds a live invite (`:1520-1524`) | if its principal is in the group (`:1594-1595`) | if its principal is one of the pair (`:1454-1455`) |
| Invite (`dv_`) | its own loca only (`:1514`, `:1400-1402`) | its own loca, if the invite's member is in the group (`:1599`) | its own direct message, if the invite's member is one of the pair (`:1462-1466`) |
| Session taken with an invite | its own loca only (`:1447-1449`, `:1529`) | its own loca, if its principal is in the group | its own direct message, as one of the pair |

A profile credential (`ak_`) acts as the principal it belongs to: in
`x-admin-token` it takes the row of that principal's role; a member's `ak_` in
`x-room-token` is treated like that member's membership key
(`member_for_credential`, `hub.rs:1905-1923`), and a session taken with it
like a session taken with a membership key (`routes/membership.rs:656-665`).

What is tested: a session taken with a membership key enters an ordinary loca
where its principal holds a live invite, not one where it does not, and loses
it when the invite is revoked (`crates/server/tests/ws/groups.rs:1247-1302`);
a raw membership key in `x-admin-token` acts in the loca where its principal
is the Operator (`crates/server/tests/ws_roundtrip.rs:1240-1248`). The other
cells are read from the branches above and are not each covered by a test
named here.

**Where the two differ.** The PRINCIPLES sentence is narrower than what the
code does on purpose: a session taken with a membership key enters the
ordinary locas where its principal holds a live invite, its group's locas and
its own direct messages. Bringing the text and the behaviour together is plan item
3.21 and is the owner's decision; this record changes neither. Whether a raw
membership key presented in `x-admin-token` should act as its principal at
the Operator, group and direct message branches is not recorded either way; it
follows from `x-admin-token` being resolved to any principal
(`hub.rs:1189-1193`) and belongs to the same item. Separately, a
defect is being fixed in another branch: when the legacy `ROOM_TOKEN` is set,
every session without a loca, including one taken with a membership key,
reaches every ordinary loca (`hub.rs:1593`). Without `REQUIRE_INVITE` and
without `ROOM_TOKEN`, the legacy building-key check admits everyone to
ordinary locas (`hub.rs:1390-1391`); that is the local sandbox.

**Why.** PRINCIPLES "Identity, login, and authority are separate"
(`PRINCIPLES.tr.md:136-142`, en `149-155`) and the passages quoted above. Commit
`1507b85` (2026-07-19): an invite is bound to one loca and "reading is gated
the same as writing", so an invite for one loca does not open another's
history, roster, notes or search. CHANGELOG 0.13.0, Fixed: "Keep
room-invitation sessions confined to their room; they cannot manage profile
credentials, open unrelated rooms, or discover private conversations." The
membership-session invite branch (`hub.rs:1569-1575`) explains itself: "the
invite is their seat, whichever credential they came with".

**Cost.** The rule lives in one function with many exits and two credential
slots, and a credential's reach depends on which slot carries it.
An agent seated in several locas holds one invite per loca as well as its
membership. An invite session cannot manage the person's own credentials or
see their groups (`routes/membership.rs:25-32`, `:70`).

### 9. A direct reply restarts the clock of a declared wait

**Decision.** When someone has declared that they are waiting for a person,
and that person then posts an ordinary message whose structured `target` is
the waiter, the server wakes the waiter and restarts that wait's reminder
clock: `since` moves to the reply, and `last_signal_at` and `signal_count`
start again. Nothing else about the wait changes. Speech does not create a
wait, complete it, clear it or change who waits, for whom or why; ending a
wait stays an explicit act of the waiter. Naming the waiter only in the text
of a message (`@name`), addressing `all`, or replying to someone who is not
waiting for the sender touches no wait. Decided on 2026-11-14 by the lead of
the stabilisation plan, when the principle tests listed this as a possible
gap.

**Where.**
- `crates/server/src/hub.rs:3481-3535` (`Hub::post`, the block marked
  "P0#1"): the trigger and the three fields it resets, written in the same
  transaction as the message.
- `crates/server/tests/ws/principles.rs`
  (`speech_creates_no_work_record_and_explicit_acts_do`): pins that a direct
  reply changes those three columns of that one `waits` row and no other
  work record, and that a free-text mention changes nothing.
- `crates/server/tests/ws/attention.rs` (`wait_reply_*`): the wake reaches
  the waiter once, is durable for an absent waiter, never wakes its own
  sender, and a later overdue reminder is a new one.

**Why.** PRINCIPLES "Silence is not intent; waiting is explicit state"
(`PRINCIPLES.tr.md:95`, en `107-115`): the waiter declared the wait, the person
and the reason, so the server infers no dependency. What the reply restarts
is the reminder lifecycle the same paragraph gives every signal (a cooldown
and a bounded attempt count). Without the restart, a waiter who has just
been answered would be reminded that the wait is overdue, and the attempts
already spent before the answer would count against the wait after it, so
the escalation to the operator would come early. The trigger is the
structured address, never parsed text, which is what "Speaking must never
create a hidden side effect" (`PRINCIPLES.tr.md:28`, en `32`) rules out.

**Cost.** An ordinary message does write to a work record, in three columns
of one row. The principle sentence, read literally, has no exception for
it; this entry and the test are where the exception is stated. A sender
cannot address a waiter directly without restarting the clock, even when the
message is not the answer the waiter wanted; the waiter then waits again
from that moment, and the reminder comes later than it would have.

**Proposed wording for PRINCIPLES (the owner decides; not applied).** After
"The system does not infer dependencies, stalls, or work from ordinary
chat": "A message addressed to someone who declared they are waiting for
its sender wakes them and restarts that wait's reminder clock; it never
creates, completes or changes the wait."

## Mandatory records and the working Journal

### What the Journal is

The Journal is the loca's record of finished work in the doer's own words
(`crates/server/src/store.rs:443-456`; `crates/protocol/src/lib.rs:638-661`;
the catalog summary at `lib.rs:349`). A participant writes a line with
`POST /rooms/:id/journal` (`crates/server/src/routes/content.rs:235-266`).
Lines are never edited or deleted (a loca rename moves them with the loca,
`store/rooms.rs:23`), and nothing in the server reads them to decide
anything: they are stored and sent to the loca's live connections
(`hub.rs:4786-4811`), loaded at start-up (`hub.rs:812-818`) and listed
(`routes/content.rs:231-234`). The Journal is an extension and can be
switched off per loca.

The author `by` comes from the session (`routes/access.rs:37-51`). Where
sessions are not required and the request carries no session, the body's
`by` is accepted, although `JournalEntry` documents it as not claimable
(`protocol/src/lib.rs:642-643`).

### What the server records on its own

These rows are written by the server when an action happens, whatever anyone
says about it. None of them takes its content from a participant's own
words, and none can be switched off.

| Record | Table and code | Kept as | Read through |
|---|---|---|---|
| Building admission | `members` (`store.rs:465-472`: `admitted_by`, `joined_at`, `revoked_at`) | row with lifecycle times | `GET /keys` |
| Invite issue and revoke | `invites` (`store.rs:433-441`: `issued_by`, `issued_at`, `revoked_at`) | row with lifecycle times | `GET /keys` |
| Smaster codes | `smasters` (`store.rs:1646-1659`: `issued_at`, `revoked_at`) | row with lifecycle times | `GET /keys` |
| Master pairing codes | `pairing_log` (`store.rs:647-655`) | row with lifecycle times | `GET /keys` |
| Admission rights | `admission_stock` (`store.rs:565-572`) | row with lifecycle times | `GET /keys` |
| Join requests | `join_requests` (`store.rs:582-594`: `decided_by`, `decided_at`) | row with lifecycle times | join-request views |
| Credentials and sessions | `credentials`, `principal_sessions` (`store.rs:510-533`) | row with lifecycle times | profile credential list |
| Operator appointments | `room_operator_assignments` (`store.rs:547-556`) | history; a row is closed, never overwritten | `GET /rooms/:id/operators` (Building admins) |
| Group lifecycle and membership | `group_log` (`store/groups.rs:75-83`, written at `:86-99`) | append-only history | `GET /groups/:id/log` |
| Mute and ban | `bans` (`store.rs:480-488`: `decided_by`, `authority_rank`) | current state only; lifting deletes the row (`store/identity.rs:829-837`) | `GET /building/bans` |
| Kick and release | `invites.revoked_at` only (`hub.rs:4004`, `hub.rs:2321-2325`) | when, not who | `GET /keys` |
| Membership suspension | `member_suspensions` (`store.rs:505-509`) | current state only; resuming deletes the row (`store/identity.rs:37-46`) | People list |
| Master called into a direct message | `birebirs` (`store.rs:656-667`) | current state only; dismissing clears it (`hub/dm.rs:404-422`) | direct message views |
| Close and reopen | `rooms.settings.archived` (`hub.rs:3835-3837`) | current state only; no who or when | loca settings |
| Seal | `rooms.sealed_at` (`store/rooms.rs:237-255`) | when, not who | none beyond the refusal |

`GET /keys` is a view over the first five tables (`store/keys.rs:157-357`),
not a separate log.

### Where the two are mixed today

1. **Extension changes are written into the Journal.** Each change writes a
   line such as `extensions: added Focus; removed Live` with the actor's
   display name, in the same transaction as the setting
   (`hub/extensions.rs:162-217`, `store/rooms.rs:284-305`). This is what
   PRINCIPLES asks for (`PRINCIPLES.tr.md:425-426`, en `471-472`) and what
   `DESIGN.md:182-183` and `docs/concepts.md` "Loca extensions" describe.
2. **Opening a loca with a chosen set writes a Journal line**
   (`extensions: opened with …`, `hub.rs:4274-4286`). A failure to write it is
   logged and the loca is created anyway.
3. **Moving a loca between groups writes a Journal line** `moved this loca to
   …` (`hub/groups.rs:1044-1050`). The author is the fixed text `Master`, not
   the Master's display name, the write happens after the move has committed,
   and a failure is ignored. The move itself is recorded in `group_log` as
   `loca_left` and `loca_joined` (`store/groups.rs:122-131`).

Because these lines share the Journal table and shape, three things follow:

- With the Journal switched off, `GET /rooms/:id/journal` answers 403
  (`routes/extensions.rs:20`, `:65-74`), so the record of extension changes
  cannot be read, including the line that records the Journal being switched
  off. A loca opened "with chat only" has its opening line in a Journal no one
  can read.
- A `JournalEntry` has no field saying whether the server or a participant
  wrote it (`protocol/src/lib.rs:638-649`). A participant can write a line
  that reads exactly like a server line; only the `by` name tells them apart.
- Nothing in the code treats a Journal line as evidence. The documents do:
  PRINCIPLES, DESIGN and concepts present the Journal as where extension
  changes are recorded, and `docs/concepts.md` calls the Journal "append-only
  evidence of completed work and decisions".

Two server records live in the conversation instead of a table:

- A Lead change is stored only as the current `settings.lead` and an
  announcement from the sender `loca` (`hub.rs:3685-3740`). The chat line is
  the only history of who set which Lead. `loca` is not in `RESERVED_NAMES`
  (`hub.rs:502-504`), so a participant could take that display name.
- A join request is announced in the home loca's chat (`hub.rs:2603-2640`).
  Here the record itself is the `join_requests` row; the chat line is only a
  notice.

### What separating them would take

- A per-loca record table, shaped like `group_log` (loca, action, subject,
  principal id, time), written in the same transaction as the change it
  records, and readable by the loca's Operator and Building admins whether or
  not the Journal is on. Extension changes, the opening set, group moves and
  Lead changes would be written there instead of to the Journal or chat.
- Existing Journal lines stay where they are: the Journal is append-only, and
  copying old lines into a new table would need a one-time migration that can
  only guess which lines the server wrote (by their `extensions:` and `moved
  this loca to` text).
- `PRINCIPLES.tr.md:425-426` (en `471-472`) says these changes are written "to
  the Journal". Moving them needs the owner to change that sentence first;
  `DESIGN.md:182-183` and `docs/concepts.md` would follow.
- Keeping history for the current-state records in the table above (bans,
  kicks, suspensions, direct message visits, close and seal) is a separate decision:
  today lifting a ban or ending a visit leaves no trace in the database.
