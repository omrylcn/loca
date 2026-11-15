#!/usr/bin/env python3
"""Regenerate the v0.13.0 upgrade fixture from the published v0.13.0 server.

usage: generate.py ROOM_SERVER [OUT_DIR]

ROOM_SERVER is the `room-server` binary from the published
`loca-0.13.0-linux-amd64.tar.gz`. Its SHA-256 is checked before it runs; any
other binary is refused. The server runs in a new temporary directory, on a
free loopback port, with the synthetic ADMIN_TOKEN below and a new database,
and is driven only through its HTTP and WebSocket API. Nothing is written into
the database by hand. OUT_DIR defaults to this script's directory.

Outputs (see README.md):
  v0.13.0.sql   the database as `sqlite3.Connection.iterdump()` writes it
  attachments/  the attachment blob store the server wrote beside it
  expected.json v0.13.0's own answers to the views the test asks again, and
                the join-request secrets, which the database keeps only hashed

Every name and key is synthetic. Python standard library only.
"""

import base64
import contextlib
import hashlib
import json
import os
import shutil
import signal
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

# `room-server` inside the published loca-0.13.0-linux-amd64.tar.gz, whose own
# SHA-256 is 892fbbff677555f29faa3b3f0240187f18a0fa41741f1b96fc49b21682578139.
EXPECTED_SHA256 = "6712fc8967d9abfa73f99d6e70e5678b2168684376bd7ee07a338534e4bc0e36"

# The synthetic ADMIN_TOKEN. The test starts the current server with the
# same value, because the Master's principal id is derived from it.
ROOT = "fixture-v0130-root"

ATTACHMENT_BYTES = b"fixture attachment written by v0.13.0\n"


def sha256_file(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


class Api:
    def __init__(self, port):
        self.port = port
        self.base = "http://127.0.0.1:%d" % port

    def call(self, method, path, body=None, headers=None, raw=None, expect=None):
        data = None
        sent = dict(headers or {})
        if body is not None:
            data = json.dumps(body).encode()
            sent["content-type"] = "application/json"
        if raw is not None:
            data = raw
        request = urllib.request.Request(self.base + path, data=data, method=method, headers=sent)
        try:
            with urllib.request.urlopen(request, timeout=10) as response:
                status, text = response.status, response.read().decode()
        except urllib.error.HTTPError as error:
            status, text = error.code, error.read().decode()
        if expect is not None and status not in expect:
            raise SystemExit("%s %s answered %d: %s" % (method, path, status, text[:300]))
        try:
            return status, json.loads(text) if text else None
        except ValueError:
            return status, text


def admin():
    return {"x-admin-token": ROOT}


def session(token):
    return {"x-session-token": token}


def ws_send(port, path, credential_protocol, frame, api, room, text):
    """Open a WebSocket with the given credential subprotocol, send one text
    frame and wait until the message is in the loca's history."""
    sock = socket.create_connection(("127.0.0.1", port), timeout=10)
    key = base64.b64encode(os.urandom(16)).decode()
    sock.sendall(
        (
            "GET %s HTTP/1.1\r\nHost: 127.0.0.1:%d\r\nUpgrade: websocket\r\n"
            "Connection: Upgrade\r\nSec-WebSocket-Key: %s\r\nSec-WebSocket-Version: 13\r\n"
            "Sec-WebSocket-Protocol: loca.v1, %s\r\n\r\n" % (path, port, key, credential_protocol)
        ).encode()
    )
    head = b""
    while b"\r\n\r\n" not in head:
        chunk = sock.recv(1)
        if not chunk:
            raise SystemExit("websocket handshake closed")
        head += chunk
    if not head.startswith(b"HTTP/1.1 101"):
        raise SystemExit("websocket refused: %r" % head.split(b"\r\n", 1)[0])
    payload = json.dumps(frame).encode()
    mask = os.urandom(4)
    masked = bytes(byte ^ mask[i % 4] for i, byte in enumerate(payload))
    if len(payload) < 126:
        header = bytes([0x81, 0x80 | len(payload)])
    else:
        header = bytes([0x81, 0x80 | 126]) + len(payload).to_bytes(2, "big")
    sock.sendall(header + mask + masked)
    deadline = time.time() + 10
    while time.time() < deadline:
        _, history = api.call("GET", "/rooms/%s/messages" % room, headers=admin(), expect=(200,))
        if any(message.get("text") == text for message in history):
            break
        time.sleep(0.1)
    else:
        raise SystemExit("the socket message never reached the history")
    sock.sendall(bytes([0x88, 0x80]) + os.urandom(4))
    sock.close()


def drive(api, port):
    handles = {}

    # Building members: two people, an agent, one revoked, one suspended.
    keys = {}
    for name, kind in [
        ("fx-ada", "user"),
        ("fx-bea", "user"),
        ("fx-bot", "agent"),
        ("fx-gone", "agent"),
        ("fx-paused", "user"),
    ]:
        _, member = api.call(
            "POST", "/members", {"name": name, "kind": kind}, admin(), expect=(201,)
        )
        keys[name] = member["token"]
    _, residents = api.call("GET", "/residents", headers=admin(), expect=(200,))
    membership = {row["name"]: row["membership_id"] for row in residents}

    # Smasters: one that signs in (used), one never used, one revoked.
    codes = {}
    for name in ["fx-deputy", "fx-spare", "fx-retired"]:
        _, smaster = api.call("POST", "/smasters", {"name": name}, admin(), expect=(201,))
        codes[name] = smaster["token"]
    _, smasters = api.call("GET", "/smasters", headers=admin(), expect=(200,))
    smid = {row["name"]: row["id"] for row in smasters}
    api.call("DELETE", "/smasters/%s" % smid["fx-retired"], headers=admin(), expect=(200, 204))
    _, deputy = api.call(
        "POST",
        "/sessions",
        {"name": "fx-deputy", "kind": "user"},
        {"x-admin-token": codes["fx-deputy"]},
        expect=(201,),
    )

    # The Master's paired browser session, for the longest pairing allows.
    _, pairing = api.call("POST", "/pairings?ttl_hours=8760", headers=admin(), expect=(201,))
    _, master = api.call(
        "POST", "/sessions", {"name": ""}, {"x-pairing-code": pairing["pairing_code"]}, expect=(201,)
    )
    master_session = session(master["session_token"])

    # Locas and davets. fx-paused is seated before its suspension ends that
    # seat; fx-bea's fx-ops davet is revoked by the Master.
    for room in ["fx-team", "fx-ops"]:
        api.call("POST", "/rooms", {"name": room}, admin(), expect=(201,))
    davets = {}
    for room, name in [
        ("fx-team", "fx-ada"),
        ("fx-team", "fx-bea"),
        ("fx-team", "fx-bot"),
        ("fx-team", "fx-paused"),
        ("fx-ops", "fx-ada"),
        ("fx-ops", "fx-bot"),
        ("fx-ops", "fx-bea"),
    ]:
        _, davet = api.call(
            "POST", "/rooms/%s/invites" % room, {"name": name}, admin(), expect=(200, 201)
        )
        davets["%s:%s" % (room, name)] = davet["token"]
    _, ops_invites = api.call("GET", "/rooms/fx-ops/invites", headers=admin(), expect=(200,))
    bea_ops = [row["id"] for row in ops_invites if row["name"] == "fx-bea"]
    api.call(
        "DELETE",
        "/rooms/fx-ops/invites/%s" % bea_ops[0],
        headers=admin(),
        expect=(200, 204),
    )

    # Member sessions, taken with the member keys.
    sessions = {}
    for name, kind in [("fx-ada", "user"), ("fx-bea", "user"), ("fx-bot", "agent")]:
        _, taken = api.call(
            "POST", "/sessions", {"name": name, "kind": kind}, {"x-room-token": keys[name]}, expect=(201,)
        )
        sessions[name] = session(taken["session_token"])

    # Messages: plain, with an op_id, a reply, one with an attachment, one
    # from the Master, and a Smaster's socket post (its operation scope names
    # the Smaster's principal).
    def say(who, sender, sender_type, text, **extra):
        body = {"sender": sender, "sender_type": sender_type, "text": text}
        body.update(extra)
        _, message = api.call("POST", "/rooms/fx-team/messages", body, who, expect=(201,))
        return message

    first = say(sessions["fx-ada"], "fx-ada", "user", "fx hello from ada")
    say(sessions["fx-bot"], "fx-bot", "agent", "fx status from bot", op_id="fx-op-bot-0001")
    say(sessions["fx-bea"], "fx-bea", "user", "fx reply from bea", reply_to=first["id"])
    _, uploaded = api.call(
        "POST",
        "/rooms/fx-team/attachments",
        headers=dict(sessions["fx-ada"], **{"content-type": "text/plain", "x-filename": "fx-note.txt"}),
        raw=ATTACHMENT_BYTES,
        expect=(200, 201),
    )
    say(
        sessions["fx-ada"],
        "fx-ada",
        "user",
        "fx file from ada",
        attachments=[uploaded["id"]],
        op_id="fx-op-ada-0001",
    )
    say(master_session, "Master", "user", "fx word from the master")
    smaster_text = "fx socket word from deputy"
    ws_send(
        port,
        "/ws?room=fx-team&name=fx-deputy&type=user",
        "loca.session.%s" % deputy["session_token"],
        {"t": "send", "text": smaster_text, "op_id": "fx-op-deputy-0001"},
        api,
        "fx-team",
        smaster_text,
    )

    # A birebir between the two people, with a word from each side.
    _, opened = api.call(
        "POST", "/birebirs", {"name": "fx-bea"}, sessions["fx-ada"], expect=(200, 201)
    )
    birebir = opened["birebir"]["room"]
    for name, text in [("fx-ada", "fx private ada to bea"), ("fx-bea", "fx private bea to ada")]:
        api.call(
            "POST",
            "/rooms/%s/messages" % birebir,
            {"sender": name, "sender_type": "user", "text": text},
            sessions[name],
            expect=(201,),
        )

    # A group, and fx-ops moved into it.
    _, group = api.call("POST", "/groups", {"name": "fx-group"}, master_session, expect=(200, 201))
    group_id = group.get("id") or group["group"]["id"]
    _, profiles = api.call("GET", "/profiles", headers=admin(), expect=(200,))
    principal = {row["display_name"]: row["id"] for row in profiles}
    for name in ["fx-ada", "fx-bot"]:
        api.call(
            "PUT",
            "/groups/%s/members/%s" % (group_id, principal[name]),
            headers=master_session,
            expect=(200, 201, 204),
        )
    api.call("PUT", "/rooms/fx-ops/group", {"group": group_id}, master_session, expect=(200, 204))

    # A notification to everyone: fx-ada reads it, fx-bea does not.
    _, sent = api.call(
        "POST",
        "/notifications",
        {"title": "fx notice", "body": "fx notice body", "audience": {"all": True}},
        master_session,
        expect=(200, 201),
    )
    notification_id = sent.get("id") or sent["notification"]["id"]
    api.call(
        "POST", "/notifications/%s/read" % notification_id, {}, sessions["fx-ada"], expect=(200, 204)
    )

    # The Wiki extension turned off on fx-team (this also writes a Journal line).
    api.call("DELETE", "/rooms/fx-team/extensions/wiki", headers=master_session, expect=(200, 204))

    # A note with one earlier revision, a Journal line, a task and a goal.
    api.call(
        "POST",
        "/rooms/fx-team/notes",
        {"key": "fx-plan", "title": "Plan", "body": "fx plan v1", "by": "fx-ada"},
        sessions["fx-ada"],
        expect=(200, 201),
    )
    api.call(
        "PUT",
        "/rooms/fx-team/notes/fx-plan",
        {"body": "fx plan v2", "by": "fx-ada"},
        sessions["fx-ada"],
        expect=(200,),
    )
    api.call(
        "POST",
        "/rooms/fx-team/journal",
        {"text": "fx bot finished the fixture", "by": "fx-bot", "by_type": "agent"},
        sessions["fx-bot"],
        expect=(200, 201),
    )
    api.call(
        "POST",
        "/rooms/fx-team/tasks",
        {"title": "fx write the upgrade test", "assigned_to": "fx-bot", "by": "Master"},
        master_session,
        expect=(200, 201),
    )
    api.call("POST", "/rooms/fx-team/lead", {"lead": "fx-bot"}, master_session, expect=(200, 204))
    api.call(
        "POST",
        "/rooms/fx-team/goals",
        {"outcome": "fx the upgrade opens", "by": "Master"},
        master_session,
        expect=(200, 201),
    )

    # Join requests in every state: pending, approved and collected but never
    # acknowledged, approved and acknowledged, denied.
    api.call("POST", "/admission-stock", {"count": 3}, admin(), expect=(201,))
    for name in ["fx-wait", "fx-late", "fx-done", "fx-no"]:
        _, created = api.call(
            "POST", "/join-requests", {"name": name, "kind": "agent"}, expect=(200, 201)
        )
        handles[name] = {"id": created["request_id"], "secret": created["request_secret"]}
    for name in ["fx-late", "fx-done"]:
        api.call("POST", "/join-requests/%s/approve" % handles[name]["id"], headers=admin(), expect=(200,))
        _, collected = api.call(
            "POST",
            "/join-requests/%s/bootstrap" % handles[name]["id"],
            headers={"x-join-secret": handles[name]["secret"]},
            expect=(201,),
        )
        keys[name] = collected["davet"]
    api.call(
        "POST",
        "/join-requests/%s/ack" % handles["fx-done"]["id"],
        headers={"x-join-secret": handles["fx-done"]["secret"]},
        expect=(200,),
    )
    api.call("POST", "/join-requests/%s/deny" % handles["fx-no"]["id"], headers=admin(), expect=(200,))

    # Last, so nothing above runs as a revoked or suspended member.
    api.call("DELETE", "/members/%s" % membership["fx-gone"], headers=admin(), expect=(200, 204))
    api.call(
        "POST",
        "/members/%s/suspension" % membership["fx-paused"],
        {"suspended": True},
        admin(),
        expect=(200, 204),
    )

    credentials = {"root": ROOT, "master-session": master["session_token"]}
    credentials.update({"member:%s" % name: key for name, key in keys.items()})
    credentials.update({"smaster:%s" % name: code for name, code in codes.items()})
    credentials.update({"davet:%s" % seat: token for seat, token in davets.items()})
    credentials.update({"join:%s" % name: handle["secret"] for name, handle in handles.items()})
    return handles, birebir, uploaded["id"], credentials


def views(birebir, attachment, handles):
    """What the test asks the upgraded server, as (path, header, credential).
    The credential is named, never written here: the test reads it from the
    fixture database (or, for a join request, from `join_requests` below).
    fx-spare's code is deliberately never presented, so it stays unused."""
    team = "davet:fx-team:fx-ada"
    asked = [
        ("/profile", "x-session-token", "master-session"),
        ("/profile/credentials", "x-session-token", "master-session"),
        ("/profile", "x-admin-token", "smaster:fx-deputy"),
        ("/profile", "x-admin-token", "smaster:fx-retired"),
    ]
    for name in ["fx-ada", "fx-bea", "fx-bot", "fx-gone", "fx-paused", "fx-late", "fx-done"]:
        asked.append(("/profile", "x-admin-token", "member:%s" % name))
        asked.append(("/whoami", "x-room-token", "member:%s" % name))
    asked += [
        ("/profile/credentials", "x-admin-token", "member:fx-ada"),
        ("/residents", "x-admin-token", "root"),
        ("/smasters", "x-admin-token", "root"),
        ("/profiles", "x-admin-token", "root"),
        ("/rooms/fx-team/invites", "x-admin-token", "root"),
        ("/rooms/fx-ops/invites", "x-admin-token", "root"),
        ("/rooms/fx-team/messages", "x-room-token", team),
        ("/rooms/fx-team/messages", "x-room-token", "davet:fx-team:fx-bot"),
        ("/rooms/fx-team/messages", "x-room-token", "davet:fx-team:fx-paused"),
        ("/rooms/fx-ops/messages", "x-room-token", "davet:fx-ops:fx-ada"),
        ("/rooms/fx-ops/messages", "x-room-token", "davet:fx-ops:fx-bea"),
        ("/rooms/%s/messages" % birebir, "x-admin-token", "member:fx-ada"),
        ("/rooms/%s/messages" % birebir, "x-admin-token", "member:fx-bea"),
        ("/rooms/%s/messages" % birebir, "x-session-token", "master-session"),
        ("/rooms/%s/messages" % birebir, "x-admin-token", "member:fx-bot"),
        ("/birebirs", "x-admin-token", "member:fx-ada"),
        ("/building/birebirs", "x-session-token", "master-session"),
        ("/groups", "x-session-token", "master-session"),
        ("/notifications", "x-admin-token", "member:fx-ada"),
        ("/notifications", "x-admin-token", "member:fx-bea"),
        ("/notifications/sent", "x-session-token", "master-session"),
        ("/rooms/fx-team/extensions", "x-session-token", "master-session"),
        ("/rooms/fx-ops/extensions", "x-session-token", "master-session"),
        ("/rooms/fx-team/notes", "x-room-token", team),
        ("/rooms/fx-team/notes/fx-plan/history", "x-room-token", team),
        ("/rooms/fx-team/journal", "x-room-token", team),
        ("/rooms/fx-ops/journal", "x-room-token", "davet:fx-ops:fx-ada"),
        ("/rooms/fx-team/tasks", "x-room-token", team),
        ("/rooms/fx-team/goals", "x-room-token", team),
        ("/rooms/fx-team/attachments/%s" % attachment, "x-room-token", team),
        ("/join-requests", "x-admin-token", "root"),
    ]
    for name, handle in sorted(handles.items()):
        asked.append(("/join-requests/%s" % handle["id"], "x-join-secret", "join:%s" % name))
    return asked


def record(api, asked, credentials):
    """v0.13.0's own answer to each view. An attachment is recorded as the
    SHA-256 of its bytes."""
    answers = []
    for path, header, credential in asked:
        request = urllib.request.Request(
            api.base + path, headers={header: credentials[credential]}
        )
        try:
            with urllib.request.urlopen(request, timeout=10) as response:
                status, raw = response.status, response.read()
        except urllib.error.HTTPError as error:
            status, raw = error.code, error.read()
        if "/attachments/" in path:
            body = {"sha256": hashlib.sha256(raw).hexdigest()} if status == 200 else None
        else:
            try:
                body = json.loads(raw.decode())
            except ValueError:
                body = None
        answers.append(
            {"path": path, "header": header, "credential": credential, "status": status, "body": body}
        )
    return answers


@contextlib.contextmanager
def running(binary, work, db):
    """The v0.13.0 server on `db`, from a new empty working directory, on a
    free loopback port; stopped (SIGTERM, then SIGKILL) on leaving."""
    port_file = os.path.join(work, "port")
    if os.path.exists(port_file):
        os.remove(port_file)
    env = {
        "PATH": "/usr/bin:/bin",
        "ADMIN_TOKEN": ROOT,
        "REQUIRE_INVITE": "1",
        "DB_PATH": db,
        "PORT": "0",
        "BOUND_PORT_FILE": port_file,
        "BIND_ADDR": "127.0.0.1",
        "RUST_LOG": "warn",
    }
    log = open(os.path.join(work, "server.log"), "ab")
    server = subprocess.Popen([binary], cwd=work, env=env, stdout=log, stderr=log)
    try:
        for _ in range(100):
            try:
                port = int(open(port_file).read().strip())
                if Api(port).call("GET", "/health")[0] == 200:
                    break
            except (OSError, ValueError):
                pass
            time.sleep(0.1)
        else:
            raise SystemExit("the v0.13.0 server did not come up; see %s" % log.name)
        api = Api(port)
        _, health = api.call("GET", "/health", expect=(200,))
        if health.get("version") != "0.13.0":
            raise SystemExit("unexpected server version %r" % health.get("version"))
        yield api, port
    finally:
        server.send_signal(signal.SIGTERM)
        try:
            server.wait(timeout=10)
        except subprocess.TimeoutExpired:
            server.kill()
            server.wait()
        log.close()


def main(argv):
    if len(argv) not in (2, 3):
        sys.stderr.write(__doc__)
        return 2
    binary = os.path.abspath(argv[1])
    out = os.path.abspath(argv[2]) if len(argv) == 3 else os.path.dirname(os.path.abspath(__file__))
    actual = sha256_file(binary)
    if actual != EXPECTED_SHA256:
        sys.stderr.write("refusing %s: SHA-256 %s is not v0.13.0's %s\n" % (binary, actual, EXPECTED_SHA256))
        return 1
    work = tempfile.mkdtemp(prefix="loca-v0130-fixture-")
    db = os.path.join(work, "loca.db")
    # Write everything, stop, and record the views from a second start on
    # the same database: some answers (a reply's `reply_to_sender`) exist
    # only in the memory of the server that took the message, and the test
    # compares with what v0.13.0 shows for the database it finds on disk.
    with running(binary, work, db) as (api, port):
        handles, birebir, attachment, credentials = drive(api, port)
    with running(binary, work, db) as (api, port):
        answers = record(api, views(birebir, attachment, handles), credentials)

    os.makedirs(out, exist_ok=True)
    connection = sqlite3.connect(db)
    with open(os.path.join(out, "v0.13.0.sql"), "w", encoding="utf-8") as dump:
        # As `sqlite3 .dump` writes it: tables are created in name order, so a
        # foreign key may name a table that comes later.
        dump.write("PRAGMA foreign_keys=OFF;\n")
        for line in connection.iterdump():
            dump.write(line + "\n")
    connection.close()
    blobs = os.path.join(out, "attachments")
    if os.path.exists(blobs):
        shutil.rmtree(blobs)
    # The blobs only: `tmp/` holds unfinished uploads and is empty here.
    shutil.copytree(os.path.join(work, "attachments"), blobs, ignore=shutil.ignore_patterns("tmp"))
    with open(os.path.join(out, "expected.json"), "w", encoding="utf-8") as expected:
        json.dump(
            {"join_requests": handles, "birebir": birebir, "views": answers},
            expected,
            indent=1,
            sort_keys=True,
        )
        expected.write("\n")
    shutil.rmtree(work)
    print("wrote the v0.13.0 fixture to %s" % out)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
