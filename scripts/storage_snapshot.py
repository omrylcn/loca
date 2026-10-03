#!/usr/bin/env python3
"""Offline, verified SQLite + attachment snapshots. Requires a stopped server.

No tar extraction, no credential printing, and no overwrite of existing data.
Snapshots contain credentials: keep the directory private and encrypt off-site.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import sqlite3
import tempfile


def digest(path):
    if path.is_symlink() or not path.is_file():
        raise ValueError("snapshot requires regular files (no symlinks)")
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def refs(db):
    with sqlite3.connect(f"{db.as_uri()}?mode=ro", uri=True) as conn:
        if conn.execute("PRAGMA integrity_check").fetchall() != [("ok",)]:
            raise ValueError("SQLite integrity check failed")
        rows = conn.execute("SELECT sha, size FROM attachment_blobs").fetchall()
    for sha, size in rows:
        if not re.fullmatch(r"[0-9a-f]{64}", sha) or not isinstance(size, int) or size < 0:
            raise ValueError("invalid attachment metadata")
    return dict(rows)


def blob_path(root, sha):
    for parent in (root / "attachments", root / "attachments" / sha[:2]):
        if parent.is_symlink():
            raise ValueError("attachment directory must not be a symlink")
    return root / "attachments" / sha[:2] / sha


def copy_private(source, target):
    target.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    # Open without following the leaf, even if replaced after validation.
    fd = os.open(source, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
    with os.fdopen(fd, "rb") as stream, target.open("xb") as dest:
        os.chmod(target, 0o600)
        shutil.copyfileobj(stream, dest)
        dest.flush()
        os.fsync(dest.fileno())


def validate(snapshot):
    if snapshot.is_symlink():
        raise ValueError("snapshot directory must not be a symlink")
    manifest_path = snapshot / "manifest.json"
    if manifest_path.is_symlink():
        raise ValueError("manifest must not be a symlink")
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if not isinstance(manifest, dict):
        raise ValueError("snapshot manifest must be an object")
    db = snapshot / "loca.sqlite3"
    if manifest.get("format") != 1 or manifest.get("database_sha256") != digest(db):
        raise ValueError("snapshot database checksum mismatch")
    references = refs(db)
    if manifest.get("attachments") != references:
        raise ValueError("snapshot manifest does not match database references")
    for sha, size in references.items():
        path = blob_path(snapshot, sha)
        if digest(path) != sha or path.stat().st_size != size:
            raise ValueError("attachment checksum or size mismatch")
    return references


def publish(destination, populate):
    if destination.exists() or destination.is_symlink():
        raise ValueError("destination must not exist; never overwrite a live store")
    destination.parent.mkdir(parents=True, exist_ok=True)
    stage = Path(tempfile.mkdtemp(prefix=".loca-snapshot-", dir=destination.parent))
    try:
        populate(stage)
        validate(stage)
        if os.name == "posix":
            for root, _dirs, files in os.walk(stage, topdown=False):
                for name in files:
                    fd = os.open(Path(root) / name, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
                    try:
                        os.fsync(fd)
                    finally:
                        os.close(fd)
                fd = os.open(root, os.O_RDONLY)
                try:
                    os.fsync(fd)
                finally:
                    os.close(fd)
        # Fail rather than replace a destination created during validation.
        if destination.exists() or destination.is_symlink():
            raise ValueError("destination appeared during snapshot creation")
        stage.rename(destination)
        if os.name == "posix":
            fd = os.open(destination.parent, os.O_RDONLY)
            try:
                os.fsync(fd)
            finally:
                os.close(fd)
    finally:
        if stage.exists():
            shutil.rmtree(stage)


def backup(database, destination):
    if database.is_symlink() or not database.is_file():
        raise ValueError("database must be an existing regular file")
    def populate(stage):
        output = stage / "loca.sqlite3"
        with sqlite3.connect(f"{database.as_uri()}?mode=ro", uri=True) as source:
            with sqlite3.connect(output) as target:
                source.backup(target)
        os.chmod(output, 0o600)
        references = refs(output)
        for sha in references:
            source = blob_path(database.parent, sha)
            if digest(source) != sha:
                raise ValueError("source attachment checksum mismatch")
            copy_private(source, blob_path(stage, sha))
        manifest = {"format": 1, "database_sha256": digest(output), "attachments": references}
        (stage / "manifest.json").write_text(json.dumps(manifest, sort_keys=True) + "\n", encoding="utf-8")
        os.chmod(stage / "manifest.json", 0o600)
    publish(destination, populate)


def restore(snapshot, destination):
    references = validate(snapshot)
    def populate(stage):
        copy_private(snapshot / "loca.sqlite3", stage / "loca.sqlite3")
        copy_private(snapshot / "manifest.json", stage / "manifest.json")
        for sha in references:
            copy_private(blob_path(snapshot, sha), blob_path(stage, sha))
    publish(destination, populate)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("backup", "verify", "restore"))
    parser.add_argument("source", type=Path)
    parser.add_argument("destination", nargs="?", type=Path)
    parser.add_argument("--server-stopped", action="store_true", help="confirm all server writers and blob sweepers are stopped")
    args = parser.parse_args()
    try:
        source = args.source.absolute()
        if args.action == "verify":
            validate(source)
        else:
            if not args.server_stopped or args.destination is None:
                parser.error("backup/restore requires a destination and --server-stopped")
            action = backup if args.action == "backup" else restore
            action(source, args.destination.absolute())
        print("snapshot verified" if args.action == "verify" else f"{args.action} completed and verified")
    except (ValueError, OSError, sqlite3.Error, KeyError, TypeError) as error:
        parser.exit(1, f"snapshot failed: {error}\n")


if __name__ == "__main__":
    main()
