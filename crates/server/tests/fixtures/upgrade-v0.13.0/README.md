# v0.13.0 upgrade fixture

A database written by the published v0.13.0 server, used to check that the
current code opens it. Two tests read it:

- `crates/server/src/store/tests/upgrade_v0_13_0.rs` opens a copy with the
  store: every table v0.13.0 had keeps every column and value apart from
  what the migrations exist to change, the Smaster codes and browser session
  tokens are replaced by their hashes with the identifiers v0.13.0 used, no
  code or token is left in any column or in the database files, the 24-hour
  rule for an approved join request counts from the stored approval time,
  and a second open changes nothing (a content digest per table).
- `crates/server/tests/ws/upgrade.rs` starts the current `room-server` on a
  copy, twice, and asks it every question in `expected.json`: each answer
  v0.13.0 gave must still be given, field for field.

## Files

| File | What it is |
|---|---|
| `v0.13.0.sql` | The database as Python's `sqlite3` `iterdump()` writes it, after `PRAGMA foreign_keys=OFF;` |
| `attachments/` | The blob store v0.13.0 wrote beside the database (one text file) |
| `expected.json` | v0.13.0's own answers to the views the test asks again, and the join-request secrets, which the database keeps only hashed |
| `generate.py` | How all of the above was made |

The database is committed as a SQL dump rather than a SQLite file so that it
can be read in review and is seen by the secret scan, and because it is
smaller (37 KB; the same database as a file is about 500 KB). The tests
build a new database file from it.

## Every key in it is synthetic, and some are in clear text

v0.13.0 stored member keys (`members.token`, and `invites.member`,
`member_suspensions.member_token`), davets (`invites.token`), Smaster codes
(`smasters.token`), browser session tokens (`admin_sessions.token`) and an
approved join request's key copy (`join_requests.mb_token`) in clear text,
so the dump holds them that way; `expected.json` holds the four join-request
secrets. All of them were minted by a throwaway server started with the
synthetic `ADMIN_TOKEN` `fixture-v0130-root` on a loopback port, and none of
them opens anything anywhere else. The secret scan's `generic-api-key` rule
flags the `credentials` hashes and the join-request secrets; `.gitleaks.toml`
allows that one rule in `v0.13.0.sql` and `expected.json` only.

## Regenerating

You need the `room-server` binary from the published
`loca-0.13.0-linux-amd64.tar.gz` (the archive's SHA-256 is
`892fbbff677555f29faa3b3f0240187f18a0fa41741f1b96fc49b21682578139`). The script
checks the binary's own SHA-256 and refuses any other:

```bash
python3 -I crates/server/tests/fixtures/upgrade-v0.13.0/generate.py /path/to/room-server
```

It starts the server in a new temporary directory on a free loopback port
with a new database, drives it only through its HTTP and WebSocket API,
stops it, starts it again on the same database to record `expected.json`
(some answers, such as a reply's `reply_to_sender`, exist only in the memory
of the server that took the message), stops it, and overwrites the files
here. Every key, id and time changes on each run; the tests read them from
the files, so they need no change.

## What is in it

- Members: two people (`fx-ada`, `fx-bea`) and an agent (`fx-bot`) with
  davets; `fx-gone` revoked; `fx-paused` suspended after being seated.
- Smasters: `fx-deputy` (signed in, has a browser session and a socket post),
  `fx-spare` (never used, its code is never presented while generating),
  `fx-retired` (revoked).
- The Master's browser session from a pairing code, valid for a year.
- Locas `fx-team` and `fx-ops`; seven davets, one revoked by the Master and
  one ended by a suspension.
- Messages in `fx-team`: plain, with an `op_id`, a reply, one with an
  attachment, one from the Master, and the Smaster's socket post (the one
  kind the `sender_principal` migration attributes).
- A birebir between `fx-ada` and `fx-bea` with a word from each side.
- A group `fx-group` with `fx-ada` and `fx-bot`, and `fx-ops` moved into it.
- A notification to everyone, read by `fx-ada`, unread by the rest.
- The Wiki extension removed from `fx-team`, with its Journal line.
- A note with one earlier revision, a Journal line, a task, a Lead and a goal.
- Join requests: pending (`fx-wait`), approved and collected but never
  acknowledged (`fx-late`), approved and acknowledged (`fx-done`), denied
  (`fx-no`).

## Time

Two values in it depend on the day the tests run, and the real-binary test
moves both on its copy before starting the server; nothing else is edited.
`fx-late`'s approval time is fixed in the past, so after the fixture's first
day the first open of the current code clears that key copy and marks the
request expired: its status shows `bootstrap_expired: true`, collecting it
answers 410, the key it handed out keeps working and a late acknowledgement
is accepted. The test moves the approval to at least 25 hours ago so that
this holds on the first day too; the store test opens the unedited copy a
millisecond before and exactly at the deadline. The Master's browser session
expires a year after the fixture was written; the test moves its expiry (in
`admin_sessions` and `principal_sessions`) to at least 30 days ahead. The
`fx-deputy` browser session lasted 12 hours, so it has expired whenever the
tests run: the store test checks that its row keeps its expiry, nothing
checks that it signs in.

## What it does not cover

- Only v0.13.0. Databases written by earlier releases are not covered.
- Only a Building started with `REQUIRE_INVITE=1`: not `ROOM_TOKEN`,
  `REQUIRE_SESSIONS` or an open local sandbox.
- The file itself is rebuilt from the dump, so v0.13.0's page layout, free
  pages and WAL are not reproduced. The secret clean-up is checked here only
  on the rebuilt file; its effect on old pages is covered by the store's own
  tests with hand-built files.
- Kinds of data that are not in it: Wiki pages and their history, loca
  memory, Attention, care signals, waits and reminders, reactions, mutes and
  bans, an Operator other than the Master, a sealed or archived loca, a
  birebir with a called-in Master or opened history, an archived or deleted
  group, keys issued through `/keys` or `/profile/credentials`, profile
  preferences, renames, a notification to a named audience, Lobby-only
  davets, and the `iye`/`loca-care` set-up beyond what a new server creates.
- Member sessions (`principal_sessions`) are not presented, since only their
  hashes are stored, and an attachment is one small text file.
