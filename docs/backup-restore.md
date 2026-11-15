# Backup and restore

A complete Loca snapshot contains SQLite **and** all attachment blobs indexed
by that database. SQLite's backup API alone does not copy uploaded files.
Use Python 3.11+ and `scripts/storage_snapshot.py` from the same source checkout
as the server. This is an offline operation: stop every server instance and
blob sweeper that can write this data directory before confirming
`--server-stopped`. The flag is an operator confirmation, not a process detector.

## Create and check a snapshot

For a systemd deployment, stop its actual service first. For Compose, stop the
server container, then find the bind mount or volume's data directory with
`docker compose config` / `docker volume inspect`. Do not assume a host path or
run a second server against the production volume.

```bash
python3 scripts/storage_snapshot.py backup /var/lib/loca/loca.db \
  /var/backups/loca-verified-snapshot --server-stopped
python3 scripts/storage_snapshot.py verify /var/backups/loca-verified-snapshot
```

Choose a new destination for each snapshot. The tool refuses overwrite,
symlinked database/blob files, bad SQLite integrity, missing blobs, or hashes
that disagree with content IDs. Only a fully checked staging directory is
published. Pending uploads indexed in SQLite are included too. Unindexed
temporary files are not part of the snapshot. Restart the production service
after the snapshot finishes (also after a failed snapshot).

Snapshots contain identity/session credentials and private room history.
They have private file permissions; encrypt off-site copies and restrict who
can read them. Checksums detect corruption, not malicious modification by a
person who can replace both data and manifest.

## Restore into a separate empty location

```bash
python3 scripts/storage_snapshot.py restore /var/backups/loca-verified-snapshot \
  /var/lib/loca-restore-check --server-stopped
python3 scripts/storage_snapshot.py verify /var/lib/loca-restore-check
```

The destination must **not exist**. Restored SQLite is named `loca.sqlite3`;
its blobs are at `attachments/<first-two-hash-characters>/<sha256>` beside it.
Point a compatible server binary's `DB_PATH` to that file. Bind a restore-test
server to loopback and different ports, never expose copied credentials or
start it against the live store.

Verify `/health`, room/message/Notes/Journal history, and download a real
attachment and compare its SHA-256 to the original. Verify session and invite
policy too. Shut down the restore-test server before any production cutover.
For actual rollback, stop production, keep its current directory recoverable,
and select the verified restore directory plus the compatible old binary.
Never edit production SQLite by hand or assume newer schema works with an
older release. Database and blob directory must always move together.

## Rolling back is restoring a snapshot

Rolling back an upgrade means stopping the new release, restoring the
snapshot taken before the upgrade into a new location as above, verifying
it, and running the old binary on that copy. Everything written after the
upgrade is lost. Starting an older build on the database a newer one has
written is not a rollback, even when the older build starts:

- Once v0.14.0 has opened a database, neither v0.13.0 nor
  v0.12.10 starts on it: the open stops with `no such column: token`,
  because `smasters` no longer has that column (see the next section). No
  other earlier build starts on it either: `members` and `invites` no longer
  have that column.
- v0.12.10 starts on a database written by v0.13.0, but while it runs it
  does not do what v0.13.0 added. A direct message is open to the Master (v0.13.0
  answers 404) and, on a Building that sets neither `ROOM_TOKEN` nor
  `REQUIRE_INVITE`, to anyone, who can also write in it; once v0.13.0 runs
  again, both people see what was written there. Saving a loca's settings
  rewrites them without the loca's extension choice, so once v0.13.0 runs
  again the loca has the default extensions (a Wiki that was turned off is
  back).

## Upgrading a binary deployment with one command

`scripts/upgrade-server.py` upgrades a server that runs the `room-server`
file from a release archive (`loca-<version>-linux-amd64.tar.gz`) under a
service manager such as systemd. It guesses nothing: every path and both
service commands are arguments, and `--health-url` must be the server's
loopback `/health`. It does not handle the Compose deployment, whose image is
built from a checkout and whose data sits in a named volume: upgrade that one
as [Self-host Loca](self-host.md#5-backup-upgrade-rollback) describes.

```bash
python3 scripts/upgrade-server.py \
  --archive /srv/loca-dl/loca-X.Y.Z-linux-amd64.tar.gz \
  --sums /srv/loca-dl/SHA256SUMS --version X.Y.Z \
  --binary /usr/local/bin/room-server --db-path /var/lib/loca/loca.db \
  --work-dir /var/backups/loca-upgrade-X.Y.Z \
  --health-url http://127.0.0.1:8787/health \
  --stop-command 'systemctl stop loca' --start-command 'systemctl start loca' \
  --dry-run
```

`--dry-run` runs the checks below and prints every step with its exact paths
and commands; it stops, writes and changes nothing. Run it first, then run
the same command without `--dry-run`. Before it stops anything, it states
that the upgrade is one way: once the new version has started on the
database, no earlier build opens it, so going back, then or later, means
restoring the snapshot this command takes and starting the old binary on
that copy, losing what was written after the upgrade started (see
[Rolling back is restoring a snapshot](#rolling-back-is-restoring-a-snapshot)).
In order, the command:

1. checks, changing nothing: the archive against the one `SHA256SUMS` line
   for its name, that it holds `loca-X.Y.Z-linux-amd64/room-server`, that the
   running server is healthy and older than X.Y.Z, that `--binary` and
   `--db-path` are regular files in writable directories, that the snapshot
   tool runs, that `--work-dir` does not exist yet, and the free space for
   the snapshot and for the first start (twice the database, see below);
2. extracts the new binary into `--work-dir`, keeps a copy of the current
   one there, and starts each of them once with no database on a random
   loopback port: the new one must report X.Y.Z and the current one the
   running version;
3. stops the service and waits until a plain TCP connection to the health
   address is refused (a connection that is accepted or times out means a
   process is still there, whatever it answers; no HTTP is spoken for this
   check, so an error status, a redirect or a proxy cannot pass for a
   stopped server);
4. takes a snapshot into `--work-dir/snapshot` with `storage_snapshot.py`
   and verifies it;
5. installs the new binary at `--binary` (atomic rename, same mode and
   owner), starts the service and waits for `/health` to report X.Y.Z.

It refuses, with nothing stopped or changed, when any check in 1 or 2 fails,
including whenever it cannot take a snapshot. If the stop or the snapshot
fails, it starts the unchanged old version again. If anything fails after
the snapshot, before the new version reports healthy with X.Y.Z, it rolls
back: it stops the service, and only when the stop command succeeded and
the health port refuses connections does it go on. If the server is not
known to have stopped it touches nothing (the database, its `-wal`/`-shm`,
`attachments/` and the new binary stay where they are) and ends with exit
`5`, naming the snapshot and the old binary to finish by hand. Otherwise it
restores the snapshot beside the database, moves the current database, its `-wal`/`-shm` and `attachments/` aside into
`.loca-failed-upgrade-<time>/` in the database's directory, puts the restored
copy in their place, puts the old binary back, starts it and requires the
old version. **Rolling back discards everything written after the upgrade
started**: it is kept in that directory, not merged back. Exit codes: `0`
upgraded, `1` refused, `3` stopped but unchanged and the old version is
running again, `4` rolled back, `5` the server is not back and the output
lists the snapshot, the old binary and the moved data to finish by hand.

Run it as the user that may stop the service and write the binary and the
database (usually root); restored files get the database's owner. By default
it uses the `storage_snapshot.py` beside it; `--snapshot-tool` selects
another, such as the one shipped in the running release's archive. Keep
`--work-dir` (the verified snapshot and the old binary) until you would no
longer go back; going back later is the manual rollback above.

## Upgrading past v0.13.0: a one-way migration

The first start of v0.14.0 rebuilds the tables that held
secrets in clear text: `smasters` keeps only the hashes of the Smaster codes
and the identifiers derived from them, `admin_sessions` keeps only the hashes
of the Master and Smaster browser session tokens, and `members`, `invites`
and `member_suspensions` keep only the hashes of the membership keys and
invites with the ids (`mbid_`, `ivid_`) derived from them. Everyone signed in
before the upgrade stays signed in, and every member and invite keeps working
with the key or invite it has. This cannot be undone in place: v0.13.0 reads
`smasters.token`, and every earlier build reads `members.token`, every time
it opens a database, so once the new release has opened a database, no
earlier build starts on it, an unreleased development build included.

1. Stop the server and take a snapshot, then verify it, exactly as above.
   Keep it until the new release has run long enough that you would not go
   back.
2. Start the new release. The migrations and one clean-up for all of them
   (`VACUUM` and a truncating WAL checkpoint) run once, before the server
   serves. `VACUUM` rewrites the whole database and can need **up to twice
   the database's size in additional free disk space** while it runs; leave
   room beyond that for the existing write-ahead log and for the snapshot
   from step 1. `VACUUM` also writes a temporary copy of the database to
   SQLite's temporary directory (`SQLITE_TMPDIR`, else `TMPDIR`, else
   `/var/tmp` or `/tmp`), so that directory needs room for one more copy;
   the migrations themselves keep their temporary data in memory. Measured
   once on one machine: a database of 68 MB (2,000 members, 5,000 invites,
   200,000 messages) started in under 3 seconds, its files peaked at about
   twice its size and ended at 53 MB.
   If the disk fills, or another process has the database open, the start
   fails with that reason and the clean-up stays pending: the migrations
   themselves are already committed, so free the space or stop the other
   process and start again. Check the space before you start, not after.
3. To roll back, stop the new release, restore the snapshot from step 1 into
   a new location, verify it, and run the old binary on it. Everything
   written after the upgrade is lost.

Snapshots and backups taken before the upgrade still contain every Smaster
code, unexpired browser session token, membership key and invite in clear
text: protect them as before. After the upgrade the database holds a key or an
invite in clear text only in its bounded delivery copy (an approved join
request's key, a claimed key and an invite minted for delivery, each until it
is acknowledged or first used, and never past 24 hours); a snapshot taken
while such a copy exists contains it. Snapshots still hold private room
history and every identity's hashes and ids, so protect new snapshots too.
The clean-up removes the old secrets from the live database file and its
WAL; it cannot reach older copies, backups, or blocks the filesystem or disk
kept elsewhere.
