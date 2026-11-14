"""Bounded recipient-only Building announcement cache, separate from room work.

Announcements never create a model turn or a room reply. Lobby frames merely
wake bounded REST recovery; the next real attention carries recent context.
The server remains the archive and read-state authority.
"""
import argparse
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import sys
import threading
import urllib.error
import urllib.request
from urllib.parse import urlencode

from credentials import load_local_env, validate_server_scope

PAGE_SIZE = 16
CACHE_LIMIT = 128
CONTEXT_LIMIT = 4
MAX_RESPONSE_BYTES = 512 * 1024


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None  # Never forward membership credentials to a redirect target.


def fetch_page(server, membership, **query):
    validate_server_scope(server)
    if not membership:
        raise ValueError("Building notification reads require this identity's membership")
    request = urllib.request.Request(f"{server.rstrip('/')}/notifications?{urlencode(query)}")
    request.add_header("x-room-token", membership)
    # Deliberately do NOT attach the compatibility room-scoped LOCA_SESSION.
    with urllib.request.build_opener(NoRedirect()).open(request, timeout=5) as response:
        data = response.read(MAX_RESPONSE_BYTES + 1)
    if len(data) > MAX_RESPONSE_BYTES:
        raise ValueError("notification response exceeds the bounded page budget")
    page = json.loads(data)
    if not isinstance(page, dict) or not isinstance(page.get("notifications"), list):
        raise ValueError("invalid notification page")
    return page


def public_notice(row):
    if not isinstance(row, dict) or type(row.get("id")) is not int or row["id"] <= 0:
        raise ValueError("invalid notification id")
    for field, limit in (("title", 120), ("body", 2000), ("from", 256)):
        if not isinstance(row.get(field), str) or len(row[field]) > limit:
            raise ValueError(f"invalid notification {field}")
    if type(row.get("created_at")) is not int or row["created_at"] < 0:
        raise ValueError("invalid notification timestamp")
    return {**{key: row[key] for key in ("id", "title", "body", "from", "created_at")},
            "scope": "building" if (row.get("audience") or {}).get("all") else "group"}


class NotificationCache:
    def __init__(self, path, server, identity, membership):
        self.path = Path(path)
        self.path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        fd = os.open(self.path, os.O_CREAT | os.O_RDWR, 0o600)
        os.close(fd)
        os.chmod(self.path, 0o600)
        self.server = server.rstrip("/")
        self.identity = identity
        if not membership:
            raise ValueError("notification cache requires a recipient membership")
        self.recipient_hash = hashlib.sha256(membership.encode()).hexdigest()
        self.cache_identity = f"{identity}:{self.recipient_hash}"
        with self.connect() as connection:
            connection.executescript("""
                PRAGMA journal_mode=WAL;
                CREATE TABLE IF NOT EXISTS checkpoint (
                    server TEXT NOT NULL, identity TEXT NOT NULL, last_id INTEGER NOT NULL,
                    PRIMARY KEY(server, identity));
                CREATE TABLE IF NOT EXISTS notices (
                    server TEXT NOT NULL, identity TEXT NOT NULL, id INTEGER NOT NULL, payload TEXT NOT NULL,
                    PRIMARY KEY(server, identity, id));
            """)

    @contextmanager
    def connect(self, timeout=5):
        connection = sqlite3.connect(self.path, timeout=timeout)
        try:
            with connection:
                yield connection
        finally:
            connection.close()

    def cursor(self):
        with self.connect() as connection:
            row = connection.execute("SELECT last_id FROM checkpoint WHERE server=? AND identity=?",
                (self.server, self.cache_identity)).fetchone()
        return row[0] if row else 0

    def recover_page(self, membership):
        if hashlib.sha256(membership.encode()).hexdigest() != self.recipient_hash:
            raise ValueError("notification cache recipient mismatch")
        since = self.cursor()
        page = fetch_page(self.server, membership, since=since, limit=PAGE_SIZE, order="asc")
        if page.get("order") != "asc":
            raise ValueError("notification recovery requires server support for order=asc; upgrade server")
        rows = page["notifications"]
        if len(rows) > PAGE_SIZE or (not rows and page.get("more")):
            raise ValueError("invalid notification recovery page size")
        notices = [public_notice(row) for row in rows]
        ids = [notice["id"] for notice in notices]
        if ids != sorted(set(ids)) or any(message_id <= since for message_id in ids):
            raise ValueError("notification recovery page did not advance in order")
        with self.connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            connection.executemany("INSERT OR IGNORE INTO notices VALUES (?, ?, ?, ?)",
                [(self.server, self.cache_identity, n["id"], json.dumps(n, ensure_ascii=False)) for n in notices])
            connection.execute("INSERT INTO checkpoint VALUES (?, ?, ?) ON CONFLICT(server, identity) "
                "DO UPDATE SET last_id=MAX(last_id,excluded.last_id)",
                (self.server, self.cache_identity, ids[-1] if ids else since))
            # This is a bounded context cache, not the archive. Server records
            # and read receipts are untouched; older items stay readable there.
            connection.execute("DELETE FROM notices WHERE server=? AND identity=? AND id NOT IN "
                "(SELECT id FROM notices WHERE server=? AND identity=? ORDER BY id DESC LIMIT ?)",
                (self.server, self.cache_identity, self.server, self.cache_identity, CACHE_LIMIT))
        return bool(page.get("more"))

    def context(self):
        try:
            # Optional announcement context cannot stall a direct room wake
            # behind a cache writer, damaged cache or filesystem problem.
            with self.connect(timeout=0.05) as connection:
                rows = connection.execute("SELECT payload FROM notices WHERE server=? AND identity=? "
                    "ORDER BY id DESC LIMIT ?", (self.server, self.cache_identity, CONTEXT_LIMIT + 1)).fetchall()
        except (sqlite3.Error, OSError):
            return {"recipient": self.identity, "items": [], "more_available": True, "unavailable": True}
        return {"recipient": self.identity, "items": [json.loads(row[0]) for row in reversed(rows[:CONTEXT_LIMIT])],
                "more_available": len(rows) > CONTEXT_LIMIT}


class NotificationRecovery:
    def __init__(self, cache, membership):
        self.cache, self.membership = cache, membership
        self.wake = threading.Event()
        self.stopping = threading.Event()
        self.thread = threading.Thread(target=self.run, name="loca-notifications", daemon=True)

    def start(self):
        self.thread.start()

    def stop(self):
        self.stopping.set(); self.wake.set()
        self.thread.join(timeout=6)

    def run(self):
        delay = 0
        failures = 0
        while not self.stopping.is_set():
            self.wake.wait(delay)
            self.wake.clear()
            if self.stopping.is_set():
                return
            try:
                more = self.cache.recover_page(self.membership)
                failures = 0
                delay = 0.1 if more else 5
            except Exception as error:
                failures += 1
                # No body, credential, or notification contents in diagnostics.
                detail = f"HTTP {error.code}" if isinstance(error, urllib.error.HTTPError) else str(error)
                if failures == 1:
                    sys.stderr.write(f"[notifications] recovery unavailable: {detail}\n")
                    sys.stderr.flush()
                delay = min(60, 2 ** min(failures, 6))
                # Live frames cannot bypass a transport/auth failure cooldown.
                self.stopping.wait(delay)
                delay = 0


def main():
    parser = argparse.ArgumentParser(description="Read this identity's Building notifications; no room or send authority")
    parser.add_argument("server")
    parser.add_argument("identity")
    parser.add_argument("--before", type=int)
    args = parser.parse_args()
    try:
        load_local_env(args.identity)
        validate_server_scope(args.server)
        if os.environ.get("LOCA_NAME") != args.identity:
            raise ValueError("notification reads require the requested identity's own named configuration")
        query = {"limit": 20}
        if args.before is not None:
            query["before"] = args.before
        page = fetch_page(args.server, os.environ.get("LOCA_MEMBERSHIP"), **query)
        print(json.dumps(page, ensure_ascii=False))
    except Exception as error:
        print(f"notification read failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
