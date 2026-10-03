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
attachment and compare its SHA-256 to the original. Verify session and davet
policy too. Shut down the restore-test server before any production cutover.
For actual rollback, stop production, keep its current directory recoverable,
and select the verified restore directory plus the compatible old binary.
Never edit production SQLite by hand or assume newer schema works with an
older release. Database and blob directory must always move together.
