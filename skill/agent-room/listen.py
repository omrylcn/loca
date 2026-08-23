#!/usr/bin/env python3
"""Minimal dependency-free WebSocket listener for loca.

Keeps a connection open (so the agent shows as ONLINE) and appends each incoming
chat message as one JSON line to a log file. Pure stdlib — no websockets pkg.

Usage:  listen.py <ws_url> <out.jsonl> [--skip-own NAME]
        [--cursor FILE] [--turn-log FILE]
  e.g.  listen.py "ws://127.0.0.1:8787/ws?room=general&name=Claude&type=agent" claude.jsonl

Multi-room: give `room=` a comma list (room=general,mobile) — ONE process opens
one WS per room (a thread each, shared output/hook/cursor). Fewer processes to
leak, one thing to stop. The cursor file then holds JSON {room: last_id}.

--cursor FILE persists the last-delivered message id; on every (re)connect the
listener backfills anything missed via REST (?since=), so a message that
arrived while the connection was down is not lost.
"""
import base64
import json
import os
import re
import socket
import ssl
import struct
import sys
import threading
import time
import urllib.request
import uuid
from urllib.error import HTTPError
from urllib.parse import parse_qs, quote, urlparse, urlencode

from credentials import (
    CredentialError,
    load_local_env,
    token_for_room,
    update_env_values,
    validate_server_scope,
)

ADAPTER_PROTOCOL_VERSION = "1"
DELIVERY_DEADLINE_MS = 4000
BACKFILL_PAGE_SIZE = 200
DEFAULT_TURN_MAX_MESSAGES = 4


def directly_names(message, name):
    """Return true only for an exact target or exact @name token."""
    expected = name.casefold()
    target = str(message.get("target") or "").casefold()
    if target == expected:
        return True
    # Agents often address the room (`target=all`) while explicitly naming
    # one or more people in the text.  The exact @name is still a direct call;
    # treating target=all as authoritative made those calls disappear for
    # --only-direct listeners.  Bare @all remains filtered because it does not
    # contain this identity's exact name.
    mentions = re.findall(r"(?<![A-Za-z0-9_-])@([A-Za-z0-9_-]+)", message.get("text") or "")
    return any(mention.casefold() == expected for mention in mentions)


def load_cursor_state(path, initial_rooms):
    """Restore every persisted room cursor, including Lobby-called locas."""
    last = {room: 0 for room in initial_rooms}
    if not path or not os.path.exists(path):
        return last
    try:
        with open(path, encoding="utf-8") as cursor_file:
            raw = cursor_file.read().strip()
        if raw.startswith("{"):
            data = json.loads(raw)
        else:
            # Legacy single-room cursor.
            data = {initial_rooms[0]: int(raw or 0)}
        if isinstance(data, dict):
            for room, message_id in data.items():
                last[str(room)] = int(message_id)
    except (OSError, TypeError, ValueError, json.JSONDecodeError):
        pass
    return last


def save_cursor_state(path, state):
    """Atomically persist listener delivery cursors with private permissions."""
    if not path:
        return
    parent = os.path.dirname(os.path.abspath(path))
    os.makedirs(parent, exist_ok=True)
    temp = os.path.join(parent, f".{os.path.basename(path)}.{os.getpid()}.tmp")
    fd = os.open(temp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            json.dump(state, handle, ensure_ascii=False, sort_keys=True)
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temp, path)
        os.chmod(path, 0o600)
    finally:
        try:
            os.unlink(temp)
        except FileNotFoundError:
            pass


def persist_env_value(key, value):
    """Atomically replace one credential in this identity's 0600 env file."""
    path = os.environ.get("LOCA_ENV") or os.path.join(
        os.path.expanduser("~"), ".loca", "env"
    )
    try:
        update_env_values(path, {key: value or None})
    except (CredentialError, OSError) as e:
        sys.stderr.write(f"credential update failed: {e}\n")
        return False
    if value:
        os.environ[key] = value
    else:
        os.environ.pop(key, None)
    return True


def room_env_key(room):
    suffix = re.sub(r"[^A-Za-z0-9_]", "_", room)
    return "DAVET_" + suffix


def claim_membership(url):
    """Claim the permanent lobby identity behind one of our live davets."""
    current = os.environ.get("LOCA_MEMBERSHIP")
    if current:
        return current
    # An identity can hold several loca credentials and one of them may have
    # been released while another is still live.  Trying only the first env
    # entry made migration to the lobby depend on file ordering: a stale
    # DAVET_sb_dev before a valid DAVET_sb_mobile permanently kept the agent
    # out of the lobby.
    davets = list(
        dict.fromkeys(
            value
            for key, value in os.environ.items()
            if key.startswith("DAVET_") and value
        )
    )
    if not davets:
        return None
    parsed = urlparse(url)
    scheme = "https" if parsed.scheme == "wss" else "http"
    rest = (
        f"{scheme}://{parsed.hostname}:"
        f"{parsed.port or (443 if parsed.scheme == 'wss' else 80)}"
    )
    last_error = None
    for davet in davets:
        req = urllib.request.Request(
            f"{rest}/membership/claim", data=b"", method="POST"
        )
        req.add_header("x-room-token", davet)
        try:
            with urllib.request.urlopen(req, timeout=10) as response:
                body = json.loads(response.read().decode())
        except Exception as e:
            last_error = e
            continue
        membership = body.get("membership_token")
        if not membership:
            continue
        if not persist_env_value("LOCA_MEMBERSHIP", membership):
            return None
        sys.stderr.write("building lobby membership claimed\n")
        return membership
    sys.stderr.write(f"lobby membership claim failed: {last_error or 'no live davet'}\n")
    return None


def store_room_invite(room, token):
    """Persist a call and discard a session scoped to an older seat.

    There is one compatibility ``LOCA_SESSION`` slot but an identity may hold
    several loca davets.  A Lobby call is authoritative and can replace a
    revoked seat, so carrying the old session into the new room makes the
    first write fail even though the fresh davet is valid.
    """
    key = room_env_key(room)
    # Replayed lobby snapshots are deliberately idempotent.  Rewriting an
    # unchanged davet would also discard the live room session on every lobby
    # reconnect, producing the periodic 401/rejoin loop users experienced.
    if os.environ.get(key) == token:
        return False
    path = os.environ.get("LOCA_ENV") or os.path.join(
        os.path.expanduser("~"), ".loca", "env"
    )
    try:
        update_env_values(path, {key: token, "LOCA_SESSION": None})
    except (CredentialError, OSError) as error:
        sys.stderr.write(f"credential update failed: {error}\n")
        return False
    os.environ[key] = token
    os.environ.pop("LOCA_SESSION", None)
    return True


def reconcile_room_invite_snapshot(invites):
    """Atomically replace the local davet cache from one lobby snapshot.

    Room names alone are insufficient: a revoked/re-issued davet can keep the
    same room while changing token.  Applying the full authoritative snapshot
    in one env-file transaction prevents a listener from observing a mixture
    of old and new credentials.
    """
    if not isinstance(invites, list):
        raise ValueError("lobby invite snapshot must be a list")
    current = {}
    for invite in invites:
        if not isinstance(invite, dict):
            raise ValueError("lobby invite snapshot contains a non-object")
        room = str(invite.get("room") or "").strip()
        token = str(invite.get("token") or "").strip()
        if not room or not token:
            raise ValueError("lobby invite snapshot contains an empty room/token")
        current[room_env_key(room)] = token

    local_keys = {key for key in os.environ if key.startswith("DAVET_")}
    updates = {key: None for key in local_keys - set(current)}
    updates.update(current)
    changed = any(os.environ.get(key) != value for key, value in current.items())
    changed = changed or bool(local_keys - set(current))
    if not changed:
        return []
    updates["LOCA_SESSION"] = None
    path = os.environ.get("LOCA_ENV") or os.path.join(
        os.path.expanduser("~"), ".loca", "env"
    )
    update_env_values(path, updates)
    for key in local_keys - set(current):
        os.environ.pop(key, None)
    os.environ.update(current)
    os.environ.pop("LOCA_SESSION", None)
    return [key.removeprefix("DAVET_") for key in sorted(local_keys - set(current))]


def reconcile_room_invites(url, membership):
    """Drop local davets absent from the membership's server snapshot.

    A room can be released while this process is down.  Without reconciliation
    the dead ``DAVET_*`` line survives forever and ``status`` reports the whole
    identity as stale even after a later, valid call into another loca.
    Network or schema errors are non-destructive: only a successful member
    response is authoritative enough to remove local credentials.
    """
    validate_server_scope(url)
    parsed = urlparse(url)
    scheme = "https" if parsed.scheme == "wss" else "http"
    rest = (
        f"{scheme}://{parsed.hostname}:"
        f"{parsed.port or (443 if parsed.scheme == 'wss' else 80)}"
    )
    request = urllib.request.Request(f"{rest}/whoami", method="GET")
    request.add_header("x-room-token", membership)
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            identity = json.loads(response.read().decode())
    except Exception as error:
        sys.stderr.write(f"[lobby] invite reconciliation skipped: {error}\n")
        return []
    if identity.get("kind") != "member" or not isinstance(identity.get("locas"), list):
        sys.stderr.write("[lobby] invite reconciliation skipped: invalid membership snapshot\n")
        return []

    active = {room_env_key(str(room)) for room in identity["locas"]}
    stale = sorted(
        key
        for key in list(os.environ)
        if key.startswith("DAVET_") and key not in active
    )
    if not stale:
        return []
    path = os.environ.get("LOCA_ENV") or os.path.join(
        os.path.expanduser("~"), ".loca", "env"
    )
    updates = {key: None for key in stale}
    updates["LOCA_SESSION"] = None
    try:
        update_env_values(path, updates)
    except (CredentialError, OSError) as error:
        sys.stderr.write(f"credential update failed: {error}\n")
        return []
    for key in stale:
        os.environ.pop(key, None)
    os.environ.pop("LOCA_SESSION", None)
    return [key.removeprefix("DAVET_") for key in stale]


def clear_room_invite(room, expected_token=None):
    """Forget a revoked seat without erasing a newer raced-in call."""
    key = room_env_key(room)
    if expected_token is not None and os.environ.get(key) != expected_token:
        return
    persist_env_value(key, "")


RENEW_BASE_BACKOFF = 1.0
RENEW_MAX_BACKOFF = 30.0
RENEW_MAX_ATTEMPTS = 5


def renew_backoff_plan(
    attempts,
    *,
    base=RENEW_BASE_BACKOFF,
    cap=RENEW_MAX_BACKOFF,
    max_attempts=RENEW_MAX_ATTEMPTS,
):
    """Decide the backoff for a renewed-but-still-rejected handshake session.

    A bare ``continue`` after a successful renew skips the loop's tail sleep,
    so a session that renews yet still 401s spins with no delay and hammers
    /sessions. Always sleep; grow the delay with consecutive renews and reset
    the counter at the cap so a persistent rejection settles into a hard
    backoff instead of a CPU spin. Returns ``(sleep_seconds, next_attempts)``.
    """
    attempts += 1
    sleep_seconds = min(cap, base * (2 ** (attempts - 1)))
    next_attempts = 0 if attempts >= max_attempts else attempts
    return sleep_seconds, next_attempts


def renew_session(url):
    """Mint a fresh session token from the room key and rewrite the URL.

    Sessions live in the server's memory, so a restart invalidates them and
    the WS handshake starts answering 401. Rather than dying (or looping on a
    dead token), swap in a new one — the room key is what actually proves
    membership.
    """
    validate_server_scope(url)
    u = urlparse(url)
    q = parse_qs(u.query)
    # Renew against the davet for THIS loca — that is what proves entry.
    room = q.get("room", ["general"])[0]
    tok = token_for_room(room)
    if not tok:
        return None
    scheme = "https" if u.scheme == "wss" else "http"
    rest = f"{scheme}://{u.hostname}:{u.port or (443 if u.scheme == 'wss' else 80)}"
    name = os.environ.get("LOCA_NAME") or q.get("name", ["agent"])[0]
    # Scope the session to this loca, matching connect.sh. Without `loca` the
    # server can only give a building-key session, which davet mode refuses to
    # widen — so the renewed session would open nothing.
    body = json.dumps({"name": name, "kind": "agent", "loca": room}).encode()
    req = urllib.request.Request(f"{rest}/sessions", data=body, method="POST")
    req.add_header("content-type", "application/json")
    req.add_header("x-room-token", tok)
    try:
        with urllib.request.urlopen(req, timeout=10) as r:
            new = json.loads(r.read().decode()).get("session_token")
    except HTTPError as e:
        if e.code in (401, 403):
            raise RoomAccessRevoked(
                f"loca davet rejected while renewing session (HTTP {e.code})"
            ) from e
        sys.stderr.write(f"session renew failed: {e}\n")
        return None
    except Exception as e:
        sys.stderr.write(f"session renew failed: {e}\n")
        return None
    if not new:
        return None
    os.environ["LOCA_SESSION"] = new
    # persist so a restart of THIS process doesn't have to renew again
    persist_env_value("LOCA_SESSION", new)
    sys.stderr.write("session renewed (server had restarted)\n")
    return u._replace(query=urlencode({k: v[0] for k, v in q.items()})).geturl()


def connect(url, protocols=None):
    validate_server_scope(url)
    u = urlparse(url)
    host = u.hostname
    port = u.port or (443 if u.scheme == "wss" else 80)
    path = u.path + ("?" + u.query if u.query else "")
    s = socket.create_connection((host, port), timeout=10)
    if u.scheme == "wss":
        s = ssl.create_default_context().wrap_socket(s, server_hostname=host)
    key = base64.b64encode(os.urandom(16)).decode()
    protocol_header = ""
    if protocols:
        protocol_header = "Sec-WebSocket-Protocol: " + ", ".join(protocols) + "\r\n"
    req = (
        f"GET {path} HTTP/1.1\r\n"
        f"Host: {host}:{port}\r\n"
        "Upgrade: websocket\r\n"
        "Connection: Upgrade\r\n"
        f"Sec-WebSocket-Key: {key}\r\n"
        "Sec-WebSocket-Version: 13\r\n"
        f"{protocol_header}\r\n"
    )
    s.sendall(req.encode())
    # Read handshake response headers.
    buf = b""
    while b"\r\n\r\n" not in buf:
        chunk = s.recv(1024)
        if not chunk:
            raise ConnectionError("closed during handshake")
        buf += chunk
    if b" 101 " not in buf.split(b"\r\n", 1)[0]:
        raise ConnectionError("handshake failed: " + buf.split(b"\r\n", 1)[0].decode(errors="replace"))
    # A healthy server sends WebSocket pings regularly. Keep a bounded read
    # timeout so a half-open nginx/TCP connection cannot make an agent look
    # alive locally while it has vanished from the server roster forever.
    s.settimeout(45)
    return s, buf.split(b"\r\n\r\n", 1)[1]


def send_control_frame(s, opcode, payload=b""):
    """Send one masked client control frame (pong/close)."""
    payload = bytes(payload)
    if len(payload) > 125:
        raise ValueError("WebSocket control payload is too large")
    mask = os.urandom(4)
    masked = bytes(value ^ mask[index % 4] for index, value in enumerate(payload))
    s.sendall(bytes([0x80 | opcode, 0x80 | len(payload)]) + mask + masked)


def frames(s, leftover=b""):
    """Yield decoded text payloads from the socket."""
    buf = leftover

    def read_more():
        data = s.recv(4096)
        if not data:
            raise ConnectionError("WebSocket closed")
        return data

    while True:
        # Ensure at least 2 header bytes.
        while len(buf) < 2:
            buf += read_more()
        b0, b1 = buf[0], buf[1]
        opcode = b0 & 0x0F
        masked = b1 & 0x80
        ln = b1 & 0x7F
        idx = 2
        if ln == 126:
            while len(buf) < 4:
                buf += read_more()
            ln = struct.unpack(">H", buf[2:4])[0]; idx = 4
        elif ln == 127:
            while len(buf) < 10:
                buf += read_more()
            ln = struct.unpack(">Q", buf[2:10])[0]; idx = 10
        mask = b""
        if masked:
            while len(buf) < idx + 4:
                buf += read_more()
            mask = buf[idx:idx + 4]; idx += 4
        while len(buf) < idx + ln:
            buf += read_more()
        payload = bytearray(buf[idx:idx + ln])
        buf = buf[idx + ln:]
        if masked:
            for i in range(len(payload)):
                payload[i] ^= mask[i % 4]
        if opcode == 0x8:      # close
            return
        if opcode == 0x9:      # ping
            send_control_frame(s, 0xA, payload)
            continue
        if opcode in (0x1, 0x2):
            yield payload.decode("utf-8", "replace")
        # ignore pong/continuation for this simple listener


def exact_mentions(message):
    return {
        mention.casefold()
        for mention in re.findall(
            r"(?<![A-Za-z0-9_-])@([A-Za-z0-9_-]+)",
            str(message.get("text") or ""),
        )
    }


def broadcasts(message):
    return (
        str(message.get("target") or "").casefold() == "all"
        or "all" in exact_mentions(message)
    )


def delivery_priority(event, identity, is_lead=False):
    """Classify one delivery according to Adapter Protocol v1."""
    if event.get("t") == "control":
        return "security_control"
    if event.get("t") == "care":
        return "care_signal"
    if event.get("t") == "task":
        return "explicit_task"
    messages = event.get("messages") if event.get("t") == "turn" else [event]
    messages = [message for message in messages if isinstance(message, dict)]
    user_messages = [
        message for message in messages if message.get("sender_type") == "user"
    ]
    if any(directly_names(message, identity) for message in user_messages):
        return "direct_user"
    if any(broadcasts(message) for message in messages):
        return "broadcast"
    if is_lead:
        # The server widens a lead's mentions stream to the whole room.
        return "lead_room"
    if any(
        message.get("sender_type") == "agent"
        and directly_names(message, identity)
        for message in messages
    ):
        return "addressed_agent"
    return "informational"


def server_origin(url):
    parsed = urlparse(url)
    scheme = "https" if parsed.scheme == "wss" else "http"
    default_port = 443 if scheme == "https" else 80
    port = parsed.port or default_port
    suffix = "" if port == default_port else f":{port}"
    return f"{scheme}://{parsed.hostname}{suffix}"


def make_delivery(
    room,
    event,
    identity="",
    server="",
    deadline_ms=DELIVERY_DEADLINE_MS,
    is_lead=False,
):
    """Wrap one runtime turn in a durable, idempotent delivery envelope."""
    messages = event.get("messages") if event.get("t") == "turn" else [event]
    messages = [m for m in messages if isinstance(m, dict)]
    last_id = max((m.get("id") or 0) for m in messages) if messages else 0
    care_id = (
        str((event.get("signal") or {}).get("id") or "")
        if event.get("t") == "care"
        else ""
    )
    if care_id:
        signal_room = str((event.get("signal") or {}).get("room") or "")
        if signal_room != room:
            raise ValueError(
                f"cross-room care signal: socket room={room!r}, "
                f"signal room={signal_room!r}"
            )
        delivery_id = f"{room}:care:{care_id}"
    elif last_id:
        delivery_id = f"{room}:{last_id}"
    else:
        delivery_id = f"{room}:control:{uuid.uuid4().hex}"
    return {
        "protocol_version": ADAPTER_PROTOCOL_VERSION,
        "delivery_id": delivery_id,
        "server": server,
        "room": room,
        "identity": identity,
        "priority": delivery_priority(event, identity, is_lead),
        "attempt": 1,
        "deadline_ms": deadline_ms,
        "received_at_ms": int(time.time() * 1000),
        "last_id": last_id or None,
        "event": event,
    }


def addresses(m, name):
    """Client-side mirror of the server's msg_addresses (for REST backfill,
    which is unfiltered): target == name/all, or word-boundary @name/@all."""
    if m.get("target") in (name, "all"):
        return True
    needle = "@" + name.lower()
    tok = ""
    for c in m.get("text", "").lower() + " ":
        if c.isalnum() or c in "@-_":
            tok += c
        else:
            if tok in (needle, "@all"):
                return True
            tok = ""
    return False


class BackfillError(RuntimeError):
    """A reconnect is incomplete until its durable history gap is fetched."""


class RoomAccessRevoked(RuntimeError):
    """The server rejected the room's durable davet, not merely its session."""


def backfill_pages(url, last_id, skip_own, is_lead=False):
    """Yield bounded ``(page_cursor, messages)`` REST recovery pages.

    The caller checkpoints ``page_cursor`` only after every eligible message
    in that page reached its durable sink.  Never flatten an outage-sized gap
    into one list: doing so used unbounded memory and turned 1,000 missed chat
    messages into one enormous runtime call.
    """
    u = urlparse(url)
    q = parse_qs(u.query)
    room = q.get("room", ["general"])[0]
    name = q.get("name", [""])[0]
    # Credentials no longer live in WS query strings. REST backfill must use
    # the same room-scoped local davet as the WebSocket subprotocol.
    token = token_for_room(room)
    scheme = "https" if u.scheme == "wss" else "http"
    rest = f"{scheme}://{u.hostname}:{u.port or (443 if u.scheme == 'wss' else 80)}"
    mentions_only = q.get("filter", [""])[0] == "mentions"
    rest_cursor = int(last_id)
    while True:
        params = urlencode({"since": rest_cursor, "limit": BACKFILL_PAGE_SIZE})
        req = urllib.request.Request(f"{rest}/rooms/{room}/messages?{params}")
        if token:
            req.add_header("x-room-token", token)
        if os.environ.get("LOCA_SESSION"):
            req.add_header("x-session-token", os.environ["LOCA_SESSION"])
        try:
            with urllib.request.urlopen(req, timeout=10) as r:
                msgs = json.loads(r.read().decode())
        except Exception as e:
            raise BackfillError(f"backfill failed: {e}") from e
        if not isinstance(msgs, list):
            raise BackfillError("backfill failed: server returned non-list JSON")
        if not msgs:
            return
        page_cursor = max(int(m.get("id") or 0) for m in msgs)
        if page_cursor <= rest_cursor:
            raise BackfillError("backfill failed: server page did not advance")
        eligible = []
        for m in msgs:
            if skip_own and m.get("sender") == skip_own:
                continue
            # REST is unfiltered. Re-apply the runtime's mentions policy, but
            # only after current lead state is known: a reconnecting lead must
            # recover plain room messages just like its live WebSocket does.
            if mentions_only and name and not is_lead and not addresses(m, name):
                continue
            eligible.append(m)
        rest_cursor = page_cursor
        yield page_cursor, eligible
        if len(msgs) < BACKFILL_PAGE_SIZE:
            return


def runtime_turn_max(url, settings=None):
    """Mirror the server's bounded turn size for reconnect deliveries."""
    query = parse_qs(urlparse(url).query)
    candidate = query.get("turn_max", [None])[0]
    if candidate is None and isinstance(settings, dict):
        candidate = settings.get("turn_max_messages")
    try:
        value = int(candidate)
    except (TypeError, ValueError):
        value = DEFAULT_TURN_MAX_MESSAGES
    return max(1, min(value, 16))


def chunk_messages(messages, size):
    """Yield bounded, ordered message groups without copying the full gap."""
    for start in range(0, len(messages), size):
        yield messages[start : start + size]


def fetch_room_settings(url):
    """Return the room's settings, or raise if they cannot be read."""
    u = urlparse(url)
    q = parse_qs(u.query)
    room = q.get("room", ["general"])[0]
    token = token_for_room(room)
    scheme = "https" if u.scheme == "wss" else "http"
    rest = f"{scheme}://{u.hostname}:{u.port or (443 if u.scheme == 'wss' else 80)}"
    req = urllib.request.Request(f"{rest}/rooms/{room}/settings")
    if token:
        req.add_header("x-room-token", token)
    if os.environ.get("LOCA_SESSION"):
        req.add_header("x-session-token", os.environ["LOCA_SESSION"])
    with urllib.request.urlopen(req, timeout=10) as response:
        settings = json.loads(response.read().decode())
    if not isinstance(settings, dict):
        raise ValueError("settings response is not an object")
    return settings


def fetch_room_lead(url):
    """Return the room's current lead, or raise if settings cannot be read."""
    lead = fetch_room_settings(url).get("lead")
    return str(lead) if lead else None


def acknowledge_care(url, signal_id, name):
    """ACK one care signal only after it reached the local durable sink."""
    u = urlparse(url)
    q = parse_qs(u.query)
    room = q.get("room", ["general"])[0]
    token = token_for_room(room)
    scheme = "https" if u.scheme == "wss" else "http"
    rest = f"{scheme}://{u.hostname}:{u.port or (443 if u.scheme == 'wss' else 80)}"
    encoded = quote(str(signal_id), safe="")
    body = json.dumps({"by": name}).encode()
    req = urllib.request.Request(
        f"{rest}/rooms/{quote(room, safe='')}/care/{encoded}/ack",
        data=body,
        method="POST",
    )
    req.add_header("content-type", "application/json")
    if token:
        req.add_header("x-room-token", token)
    if os.environ.get("LOCA_SESSION"):
        req.add_header("x-session-token", os.environ["LOCA_SESSION"])
    try:
        with urllib.request.urlopen(req, timeout=10) as response:
            return response.status == 204
    except Exception as error:
        sys.stderr.write(f"[{room}] care ACK failed: {error}\n")
        return False


def should_deliver_message(message, skip_own, only_direct, current_lead):
    """Apply local runtime policy without hiding the room from its lead."""
    if skip_own and message.get("sender") == skip_own:
        return False
    if (
        only_direct
        and not current_lead
        and not directly_names(message, only_direct)
        and not broadcasts(message)
    ):
        return False
    return True


def authenticated_room_url(url, room):
    """Build one room's credential-free WS URL after local setup."""
    parsed = urlparse(url)
    query = {key: values[0] for key, values in parse_qs(parsed.query).items()}
    query["room"] = room
    query.pop("token", None)
    query.pop("session", None)
    query.pop("admin", None)
    # Chat messages remain immediate and individually persisted. Runtime turn
    # coalescing belongs to the loca's settings (default: four messages, five
    # quiet seconds, bounded hard deadline). Explicit query values, when the
    # caller supplied them, remain a per-runtime compatibility override.
    return parsed._replace(query=urlencode(query)).geturl()


def room_protocols(room):
    """Authenticate a WS without leaking bearer credentials into its URL."""
    protocols = ["loca.v1"]
    token = token_for_room(room)
    if token:
        protocols.append(f"loca.room.{token}")
    session = os.environ.get("LOCA_SESSION")
    if session:
        protocols.append(f"loca.session.{session}")
    return protocols


def main():
    if len(sys.argv) < 3:
        print(
            "usage: listen.py <ws_url> <out.jsonl|-> "
            "[--skip-own NAME] [--only-direct NAME] [--cursor FILE] "
            "[--turn-log FILE]",
            file=sys.stderr,
        )
        return 2
    url, out = sys.argv[1], sys.argv[2]
    query = parse_qs(urlparse(url).query)
    requested_name = query.get("name", [""])[0].strip()
    try:
        load_local_env(requested_name)
        validate_server_scope(url)
    except CredentialError as error:
        print(f"credential configuration error: {error}", file=sys.stderr)
        return 2
    membership = claim_membership(url)
    skip_own = None
    if "--skip-own" in sys.argv:
        i = sys.argv.index("--skip-own")
        skip_own = sys.argv[i + 1] if len(sys.argv) > i + 1 else None
    # --only-direct NAME: ignore @all. An announcement to the room is not a
    # summons — used by the loca agent, who tends the place rather than
    # sitting at the table.
    only_direct = None
    if "--only-direct" in sys.argv:
        i = sys.argv.index("--only-direct")
        only_direct = sys.argv[i + 1] if len(sys.argv) > i + 1 else None
    cursor = None
    if "--cursor" in sys.argv:
        i = sys.argv.index("--cursor")
        cursor = sys.argv[i + 1] if len(sys.argv) > i + 1 else None
    turn_log = None
    if "--turn-log" in sys.argv:
        i = sys.argv.index("--turn-log")
        turn_log = sys.argv[i + 1] if len(sys.argv) > i + 1 else None

    # Multi-room: room=a,b,c -> one WS per room in one process.
    u = urlparse(url)
    q = parse_qs(u.query)
    rooms = [r.strip() for r in q.get("room", ["general"])[0].split(",") if r.strip()]
    origin = server_origin(url)

    # Open the output ONCE. Reopening it per reconnect is fatal for /dev/stdout
    # (Errno 6 the second time) and pointless for files.
    sink = None
    if out not in ("-", "/dev/stdout", "/dev/fd/1"):
        sink = open(out, "a")
        os.chmod(out, 0o600)
    turn_sink = None
    if turn_log:
        os.makedirs(os.path.dirname(os.path.abspath(turn_log)), exist_ok=True)
        turn_sink = open(turn_log, "a", encoding="utf-8")
        os.chmod(turn_log, 0o600)

    # Last delivered id per room. With --cursor this survives restarts, so a
    # relaunched listener also backfills what it missed while not running.
    # File format: JSON {room: id}; a legacy plain integer is honoured for a
    # single-room setup.
    last = load_cursor_state(cursor, rooms)

    lock = threading.Lock()   # one writer at a time: history, inbox, cursor

    def advance_cursor_locked(room, message_id):
        """Persist a durable checkpoint before publishing it in memory."""
        message_id = int(message_id or 0)
        if message_id <= last[room]:
            return
        updated = dict(last)
        updated[room] = message_id
        if cursor:
            save_cursor_state(cursor, updated)
        last[room] = message_id

    def checkpoint_cursor(room, message_id):
        """Advance over a fully processed REST page, including filtered rows."""
        with lock:
            advance_cursor_locked(room, message_id)

    def emit(room, messages, is_lead=False):
        """Deliver one queued agent turn and advance that room's cursor once."""
        if isinstance(messages, dict):
            messages = [messages]
        if not messages:
            return
        event = messages[0] if len(messages) == 1 else {
            "t": "turn",
            "room": room,
            "messages": messages,
        }
        delivery = make_delivery(
            room,
            event,
            requested_name,
            origin,
            is_lead=is_lead,
        )
        with lock:
            try:
                if sink is not None:
                    # The file is durable message history for the runtime, not
                    # the wake channel. Preserve one original message per line.
                    for message in messages:
                        sink.write(json.dumps(message, ensure_ascii=False) + "\n")
                    sink.flush()
                else:
                    # Claude Monitor treats stdout as the wake channel: exactly
                    # one line here means exactly one model turn.
                    print(json.dumps(event, ensure_ascii=False), flush=True)
                if turn_sink is not None:
                    turn_sink.write(json.dumps(delivery, ensure_ascii=False) + "\n")
                    turn_sink.flush()
            except (BrokenPipeError, OSError) as e:
                # Our reader is gone. Do NOT keep running: a listener that
                # can't deliver still holds the WS open, so the server counts
                # us ONLINE while nobody reads — the "ghost listener". Die.
                sys.stderr.write(f"output gone ({e}) — exiting so no ghost listener remains\n")
                sys.stderr.flush()
                os._exit(1)
            mid = max((m.get("id") or 0) for m in messages)
            # Cursor failure is not cosmetic. Reconnect and replay the stable
            # delivery id instead of claiming a checkpoint that was never
            # made durable.
            advance_cursor_locked(room, mid)

    def emit_immediate(event):
        """Deliver a control/announcement that must bypass turn batching."""
        room = str(event.get("room") or "")
        # State-only housekeeping is useful in the audit log but must never
        # spend a model turn. Stop/close events remain actionable.
        actionable = event.get("cmd") in ("stop", "room-closed")
        delivery = (
            make_delivery(room, event, requested_name, origin)
            if actionable
            else None
        )
        with lock:
            try:
                line = json.dumps(event, ensure_ascii=False)
                if sink is not None:
                    sink.write(line + "\n")
                    sink.flush()
                else:
                    print(line, flush=True)
                if turn_sink is not None and delivery is not None:
                    turn_sink.write(json.dumps(delivery, ensure_ascii=False) + "\n")
                    turn_sink.flush()
            except (BrokenPipeError, OSError) as e:
                sys.stderr.write(
                    f"output gone ({e}) — exiting so no ghost listener remains\n"
                )
                sys.stderr.flush()
                os._exit(1)

    def watch(room, starting_token):
        wurl = authenticated_room_url(url, room)
        current_lead = False
        turn_limit = runtime_turn_max(wurl)

        def eligible(message):
            if not should_deliver_message(
                message, skip_own, only_direct, current_lead
            ):
                return False
            return (message.get("id") or 0) > last[room]

        renew_attempts = 0
        while True:
            s = None
            try:
                s, leftover = connect(wurl, room_protocols(room))
                # A live handshake means the (possibly renewed) session works;
                # clear the renew backoff counter.
                renew_attempts = 0
                sys.stderr.write(f"[{room}] listening -> {out}\n"); sys.stderr.flush()
                try:
                    settings = fetch_room_settings(wurl)
                    current_lead = str(settings.get("lead") or "").casefold() == (
                        requested_name.casefold()
                    )
                    turn_limit = runtime_turn_max(wurl, settings)
                except Exception as lead_error:
                    current_lead = False
                    turn_limit = runtime_turn_max(wurl)
                    sys.stderr.write(
                        f"[{room}] lead status unavailable: {lead_error}\n"
                    )
                # Pull anything that arrived while we were disconnected. Done
                # after the WS is up so the gap between backfill and live push
                # is minimal (duplicates are prevented by the id check below).
                if last[room] > 0:
                    recovered = 0
                    for page_cursor, page_messages in backfill_pages(
                        wurl, last[room], skip_own, current_lead
                    ):
                        eligible_messages = [
                            message for message in page_messages if eligible(message)
                        ]
                        for batch in chunk_messages(eligible_messages, turn_limit):
                            emit(room, batch, current_lead)
                            recovered += len(batch)
                        # Filtered/self-authored messages were deliberately
                        # consumed too. Checkpoint the raw page watermark only
                        # after every eligible batch reached both durable sinks.
                        checkpoint_cursor(room, page_cursor)
                    if recovered:
                        sys.stderr.write(
                            f"[{room}] backfilled {recovered} missed message(s) "
                            f"in batches of at most {turn_limit}\n"
                        )
                        sys.stderr.flush()
                for raw in frames(s, leftover):
                    try:
                        f = json.loads(raw)
                    except Exception:
                        continue
                    t = f.get("t")
                    if t == "evicted":
                        # A newer session took our name (deliberate takeover).
                        # Reconnecting would steal it back and the two processes
                        # would fight forever — the whole listener steps down.
                        sys.stderr.write(f"[{room}] evicted: a newer session holds this name — exiting\n")
                        os._exit(0)
                    if t == "kicked":
                        reason = "banned" if f.get("banned") else "released"
                        sys.stderr.write(f"[{room}] {reason} — returning to lobby\n")
                        clear_room_invite(room, starting_token)
                        return   # only this room; the lobby connection remains
                    if t == "control":
                        event = dict(f)
                        event.setdefault("room", room)
                        emit_immediate(event)
                        continue
                    if t == "care":
                        signal = f.get("signal") or {}
                        signal_room = str(signal.get("room") or "")
                        if signal_room != room:
                            sys.stderr.write(
                                f"[{room}] rejected cross-room care signal "
                                f"for {signal_room or '<missing>'}\n"
                            )
                            sys.stderr.flush()
                            continue
                        if str(signal.get("owner") or "").casefold() == requested_name.casefold():
                            emit(room, f, current_lead)
                            if not acknowledge_care(
                                wurl, signal.get("id"), requested_name
                            ):
                                raise BackfillError(
                                    "care event was persisted locally but server ACK failed"
                                )
                        continue
                    if t in ("msg", "turn"):
                        incoming = (
                            [f["message"]]
                            if t == "msg"
                            else f.get("messages", [])
                        )
                        # Lead assignment/removal is an announce. Refresh before
                        # local filtering so a newly named lead immediately
                        # hears the whole room, and a replaced lead immediately
                        # returns to direct-only mode.
                        if any(
                            isinstance(message, dict)
                            and message.get("kind") == "announce"
                            for message in incoming
                        ):
                            try:
                                current_lead = (
                                    fetch_room_lead(wurl) or ""
                                ).casefold() == requested_name.casefold()
                            except Exception as lead_error:
                                sys.stderr.write(
                                    f"[{room}] lead refresh failed: {lead_error}\n"
                                )
                        emit(
                            room,
                            [m for m in incoming if eligible(m)],
                            current_lead,
                        )
            except Exception as e:
                # A 401 on the handshake means our session died with the
                # server's memory: renew it instead of retrying a dead token.
                # Renew if we hold ANY key — a per-loca davet or the old building
                # key. The old check required ROOM_TOKEN, so in davet mode (no
                # building key) it never renewed and the listener spun forever on
                # a dead token. This is exactly where sb-feature got stuck.
                have_key = os.environ.get("ROOM_TOKEN") or any(
                    k.startswith("DAVET_") for k in os.environ
                )
                if "401" in str(e) and have_key:
                    try:
                        renewed = renew_session(wurl)
                    except RoomAccessRevoked as revoked:
                        clear_room_invite(room, starting_token)
                        sys.stderr.write(
                            f"[{room}] {revoked} — returning to lobby\n"
                        )
                        sys.stderr.flush()
                        return
                    if renewed:
                        wurl = renewed
                        # Do NOT `continue` straight into a fresh handshake: a
                        # renewed-but-still-rejected session would spin with no
                        # delay and hammer /sessions. Back off first, and cap
                        # consecutive renews before a harder backoff.
                        backoff, renew_attempts = renew_backoff_plan(renew_attempts)
                        if renew_attempts == 0:
                            sys.stderr.write(
                                f"[{room}] renewed session still rejected; "
                                f"hard backoff {backoff:.0f}s\n"
                            )
                            sys.stderr.flush()
                        time.sleep(backoff)
                        continue
                sys.stderr.write(f"[{room}] reconnect after error: {e}\n"); sys.stderr.flush()
            finally:
                if s is not None:
                    try:
                        send_control_frame(s, 0x8)
                    except (OSError, ValueError):
                        pass
                    try:
                        s.close()
                    except OSError:
                        pass
            time.sleep(2)

    active = {}
    active_lock = threading.Lock()

    def start_room(room):
        """Start at most one listener for the loca's current davet."""
        token = token_for_room(room)
        if not token:
            return
        with active_lock:
            current = active.get(room)
            if current and current.is_alive():
                return
            last.setdefault(room, 0)

            def run():
                try:
                    watch(room, token)
                finally:
                    restart = False
                    with active_lock:
                        if active.get(room) is threading.current_thread():
                            active.pop(room, None)
                        # A call may race the old socket's release frame. If a
                        # newer davet is already present, start it after this
                        # generation has fully stepped aside.
                        newer = token_for_room(room)
                        restart = bool(newer and newer != token)
                    if restart:
                        start_room(room)

            thread = threading.Thread(
                target=run, name=f"loca-room-{room}", daemon=True
            )
            active[room] = thread
            thread.start()

    def lobby_watch():
        parsed = urlparse(url)
        lobby_url = parsed._replace(path="/lobby/ws", query="").geturl()
        while True:
            try:
                sock, leftover = connect(
                    lobby_url, ["loca.v1", f"loca.membership.{membership}"]
                )
                sys.stderr.write("[lobby] connected — waiting for calls\n")
                sys.stderr.flush()
                for raw in frames(sock, leftover):
                    try:
                        frame = json.loads(raw)
                    except Exception:
                        continue
                    kind = frame.get("t")
                    if kind == "lobby_ready":
                        snapshot = frame.get("invites")
                        if isinstance(snapshot, list):
                            try:
                                removed = reconcile_room_invite_snapshot(snapshot)
                            except (CredentialError, OSError, ValueError) as error:
                                sys.stderr.write(
                                    f"[lobby] invite snapshot rejected: {error}\n"
                                )
                                sys.stderr.flush()
                                continue
                            for invite in snapshot:
                                start_room(str(invite["room"]))
                        else:
                            # Backward compatibility with pre-snapshot servers.
                            removed = reconcile_room_invites(lobby_url, membership)
                        if removed:
                            sys.stderr.write(
                                "[lobby] removed stale local davet cache: "
                                + ", ".join(removed)
                                + "\n"
                            )
                            sys.stderr.flush()
                    elif kind == "called":
                        room = str(frame.get("room") or "").strip()
                        token = str(frame.get("token") or "").strip()
                        if room and token and store_room_invite(room, token):
                            sys.stderr.write(f"[lobby] called into {room}\n")
                            sys.stderr.flush()
                            start_room(room)
                    elif kind == "membership_revoked":
                        persist_env_value("LOCA_MEMBERSHIP", "")
                        sys.stderr.write("[lobby] building membership revoked — exiting\n")
                        return
            except Exception as e:
                if "401" in str(e):
                    sys.stderr.write(f"[lobby] membership rejected: {e}\n")
                    return
                sys.stderr.write(f"[lobby] reconnect after error: {e}\n")
                sys.stderr.flush()
                time.sleep(2)

    if membership:
        # The lobby is the process anchor. Room listeners come and go as calls
        # arrive and release returns the member here. Do not start from cached
        # DAVET_* values first: after a revoke/re-call that token can be stale,
        # and its forever-retrying thread would block the fresh lobby token
        # from starting for the same room.
        lobby_watch()
        return

    # Compatibility for old/open servers with no building membership model.
    for room in rooms:
        start_room(room)
    threads = list(active.values())
    for thread in threads:
        thread.join()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
