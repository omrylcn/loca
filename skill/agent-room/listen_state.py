"""What the listener keeps on disk and in memory between frames: the cursor,
room sessions, the membership claim, and each loca's invite and seat.
"""

import hashlib
import json
import os
import re
import sys
import threading
import time
import urllib.request
from urllib.parse import urlparse
from credentials import (
    LOBBY_ROOM,
    CredentialError,
    env_key_id,
    update_env_values,
    validate_server_scope,
)
from listen_ws import connect


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
        if os.name != "nt":
            directory = os.open(parent, os.O_RDONLY)
            try:
                os.fsync(directory)
            finally:
                os.close(directory)
    finally:
        try:
            os.unlink(temp)
        except FileNotFoundError:
            pass


# One lock for every check-then-write on this identity's kept credentials.
# The env file has its own cross-process lock, but each helper below first
# compares against ``os.environ`` and updates it after the file: without a
# process-wide lock a room thread forgetting an ended invite could run between
# the Lobby thread's file write and its ``os.environ`` update, and erase the
# fresh invite from the file while the process went on using it until restart.
CREDENTIAL_LOCK = threading.RLock()


# One listener process may sit in several locas, and a session minted from a
# loca's invite opens that loca only. The env file has a single compatibility
# ``LOCA_SESSION`` slot, so the sessions this process mints are also kept
# here, one per loca (by the loca's ``DAVET_`` key). Without that, two locas
# renewing at the same time overwrite each other's session in the one slot:
# each then presents the other's, is refused, renews and overwrites again,
# for as long as their retries stay in step.
ROOM_SESSIONS = {}


def session_for_room(room):
    """The session to present for ``room``.

    The one this process minted for that loca; before it has minted one, the
    compatibility slot (what the env file held when the process started).
    The Lobby room has none: the listener reads it with the membership key
    alone, and a loca's session presented there would close its door.
    """
    if room == LOBBY_ROOM:
        return None
    return ROOM_SESSIONS.get(room_env_key(room)) or os.environ.get("LOCA_SESSION")


def persist_env_value(key, value):
    """Atomically replace one credential in this identity's 0600 env file."""
    with CREDENTIAL_LOCK:
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


def durable_flush(sinks):
    """Commit every file before advancing cursors or sending transport ACK."""
    for sink in sinks:
        if sink is not None:
            sink.flush()
            os.fsync(sink.fileno())


def room_env_key(room):
    suffix = re.sub(r"[^A-Za-z0-9_]", "_", room)
    return "DAVET_" + suffix


def claim_membership(url):
    """Claim this identity's own Lobby key with one of its live invites.

    The claim runs as `connect.sh setup` runs it (`join_request.claim_key`):
    the set-up's claim id lives in ~/.loca/<name>.claim, so a listener that
    restarts after a lost answer asks for the same key instead of minting
    another, and the key is verified, filed in the identity env and only then
    acknowledged. Nothing here prints the key or the claim id.
    """
    current = os.environ.get("LOCA_MEMBERSHIP")
    if current:
        return current
    # An identity can hold several loca credentials and one of them may have
    # been released while another is still live.  Trying only the first env
    # entry made migration to the lobby depend on file ordering: a stale
    # DAVET_sb_dev before a valid DAVET_sb_mobile permanently kept the agent
    # out of the lobby.
    invites = list(
        dict.fromkeys(
            value
            for key, value in os.environ.items()
            if key.startswith("DAVET_") and value
        )
    )
    if not invites:
        return None
    import join_request

    parsed = urlparse(url)
    scheme = "https" if parsed.scheme == "wss" else "http"
    rest = (
        f"{scheme}://{parsed.hostname}:"
        f"{parsed.port or (443 if parsed.scheme == 'wss' else 80)}"
    )
    name = os.environ.get("LOCA_NAME", "")
    path = os.environ.get("LOCA_ENV") or os.path.join(
        os.path.expanduser("~"), ".loca", "env"
    )
    state_file = os.path.join(
        os.path.expanduser("~"),
        ".loca",
        re.sub(r"[^A-Za-z0-9_-]", "_", name) + ".claim",
    )
    for invite in invites:
        if join_request.claim_key(rest, name, invite, state_file, path) not in (0, 6):
            continue
        membership = join_request.read_env_membership(path)
        if membership:
            os.environ["LOCA_MEMBERSHIP"] = membership
            sys.stderr.write("building lobby membership claimed\n")
            return membership
    sys.stderr.write("lobby membership claim failed: no live invite\n")
    return None


def store_room_invite(room, token):
    """Persist a call and discard a session scoped to an older seat.

    There is one compatibility ``LOCA_SESSION`` slot but an identity may hold
    several loca invites.  A Lobby call is authoritative and can replace a
    revoked seat, so carrying the old session into the new room makes the
    first write fail even though the fresh invite is valid.
    """
    with CREDENTIAL_LOCK:
        key = room_env_key(room)
        # Replayed lobby snapshots are deliberately idempotent.  Rewriting an
        # unchanged invite would also discard the live room session on every lobby
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
        ROOM_SESSIONS.pop(key, None)
        return True


def reconcile_room_invite_snapshot(invites):
    """Atomically replace the local invite cache from one lobby snapshot.

    Room names alone are insufficient: a revoked/re-issued invite can keep the
    same room while changing token.  Applying the full authoritative snapshot
    in one env-file transaction prevents a listener from observing a mixture
    of old and new credentials.
    """
    with CREDENTIAL_LOCK:
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
        current_ids = {env_key_id(key) for key in current}
        gone = {key for key in local_keys if env_key_id(key) not in current_ids}
        updates = {key: None for key in gone}
        updates.update(current)
        changed = any(os.environ.get(key) != value for key, value in current.items())
        changed = changed or bool(gone)
        if not changed:
            return []
        updates["LOCA_SESSION"] = None
        path = os.environ.get("LOCA_ENV") or os.path.join(
            os.path.expanduser("~"), ".loca", "env"
        )
        update_env_values(path, updates)
        for key in gone:
            os.environ.pop(key, None)
            ROOM_SESSIONS.pop(key, None)
        for key, value in current.items():
            if os.environ.get(key) != value:
                ROOM_SESSIONS.pop(key, None)
        os.environ.update(current)
        os.environ.pop("LOCA_SESSION", None)
        return [key.removeprefix("DAVET_") for key in sorted(gone)]


def fetch_member_locas(url, membership):
    """Return the locas ``/whoami`` lists for this membership, or ``None``.

    ``None`` means "no authoritative answer": a network fault, a timeout, an
    HTTP error, an unparsable body, or an answer that is not a complete member
    snapshot.  Callers must never remove a kept invite on ``None``.
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
        return None
    locas = identity.get("locas") if isinstance(identity, dict) else None
    if (
        not isinstance(identity, dict)
        or identity.get("kind") != "member"
        or not isinstance(locas, list)
        or not all(isinstance(room, str) and room for room in locas)
    ):
        sys.stderr.write("[lobby] invite reconciliation skipped: invalid membership snapshot\n")
        return None
    return list(locas)


def drop_unlisted_room_invites(rooms):
    """Remove kept invites for locas absent from an authoritative list.

    Only a complete answer from the server (a ``seats`` list or a valid
    ``/whoami`` member snapshot) may be passed here.
    """
    with CREDENTIAL_LOCK:
        active = {env_key_id(room_env_key(str(room))) for room in rooms}
        stale = sorted(
            key
            for key in list(os.environ)
            if key.startswith("DAVET_") and env_key_id(key) not in active
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
            ROOM_SESSIONS.pop(key, None)
        os.environ.pop("LOCA_SESSION", None)
        return [key.removeprefix("DAVET_") for key in stale]


def reconcile_room_invites(url, membership):
    """Drop local invites absent from the membership's server snapshot.

    A room can be released while this process is down.  Without reconciliation
    the dead ``DAVET_*`` line survives forever and ``status`` reports the whole
    identity as stale even after a later, valid call into another loca.
    Network or schema errors are non-destructive: only a successful member
    response is authoritative enough to remove local credentials.
    """
    rooms = fetch_member_locas(url, membership)
    if rooms is None:
        return []
    return drop_unlisted_room_invites(rooms)


def invite_management_id(token):
    """Return the server's public id (``ivid_``) for one invite.

    Mirrors the server's ``invite_management_id``: ``"ivid_"`` followed by the
    first 16 bytes of SHA-256(``"invite:" + invite``) in lower-case hex.  The
    id is not a secret and is safe to compare; the invite never leaves here.
    """
    digest = hashlib.sha256(("invite:" + token).encode("utf-8")).hexdigest()
    return "ivid_" + digest[:32]


def parse_seat_snapshot(seats):
    """Validate a ``seats`` list into ``{room: invite_id}``.

    Raises ``ValueError`` (with a message that holds no value from the frame)
    when the list is not a complete, well-formed answer.  Extra fields on a
    seat are ignored.
    """
    if not isinstance(seats, list):
        raise ValueError("lobby seat list must be a list")
    parsed = {}
    for seat in seats:
        if not isinstance(seat, dict):
            raise ValueError("lobby seat list contains a non-object")
        room = seat.get("room")
        expected = seat.get("invite_id")
        if not isinstance(room, str) or not room.strip():
            raise ValueError("lobby seat list contains an empty room")
        if not isinstance(expected, str) or not expected.strip():
            raise ValueError("lobby seat list contains an empty invite_id")
        room, expected = room.strip(), expected.strip()
        if parsed.get(room, expected) != expected:
            raise ValueError("lobby seat list names one loca twice")
        parsed[room] = expected
    return parsed


def kept_invite_matches(room, expected_id):
    """True when this identity keeps an invite for ``room`` that fits its seat.

    ``expected_id`` is the seat's ``invite_id``; ``None`` means the server
    named the loca without an id (``/whoami``), so any kept invite is tried.
    """
    token = os.environ.get(room_env_key(room))
    if not token:
        return False
    return expected_id is None or invite_management_id(token) == expected_id


def invite_status_path():
    """Non-secret per-identity file read by ``connect.sh status``/``doctor``."""
    env_path = os.environ.get("LOCA_ENV") or os.path.join(
        os.path.expanduser("~"), ".loca", "env"
    )
    return env_path + ".davet-status.json"


def write_invite_status(needs_fresh, suspended=False):
    """Record which seated locas need a fresh invite (loca names only), or that
    the server said this membership is suspended."""
    path = invite_status_path()
    try:
        if suspended:
            save_cursor_state(path, {"membership_suspended": True})
        elif needs_fresh:
            save_cursor_state(path, {"needs_fresh_davet": sorted(needs_fresh)})
        else:
            os.unlink(path)
    except FileNotFoundError:
        pass
    except OSError as error:
        sys.stderr.write(f"[lobby] invite status not written: {error}\n")


def clear_room_invite(room, expected_token=None):
    """Forget a revoked seat without erasing a newer raced-in call."""
    with CREDENTIAL_LOCK:
        key = room_env_key(room)
        if expected_token is not None and os.environ.get(key) != expected_token:
            return
        persist_env_value(key, "")
        ROOM_SESSIONS.pop(key, None)
