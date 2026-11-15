"""The listener re-enters seated locas from kept invites.

These tests drive the real ``listen.py`` process against a fake Building that
speaks the Lobby WebSocket protocol, in both shapes the kit must handle:

* the current server: ``lobby_ready`` carries ``invites`` (every invite in
  clear text) and replays each one as a ``called`` frame on connect;
* the coming server: ``lobby_ready`` carries ``seats: [{room, invite_id}]``
  and no ``invites`` key, so a listener that restarts must start each loca
  from the ``DAVET_<loca>`` line it kept.

Every value below is synthetic.  No test contacts a real server, and every
test checks that no membership or invite value reaches the listener's output.
"""

import base64
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse


SKILL_DIR = Path(__file__).resolve().parents[1]

MEMBERSHIP = "mb_synthetic_worker_key_0001"
DAVET_DEV = "dv_synthetic_dev_seat_0001"
DAVET_DEV_REISSUED = "dv_synthetic_dev_seat_0002"
DAVET_GONE = "dv_synthetic_gone_seat_0001"
DAVET_OTHER = "dv_synthetic_other_seat_0001"
SECRETS = (MEMBERSHIP, DAVET_DEV, DAVET_DEV_REISSUED, DAVET_GONE, DAVET_OTHER)
# Assertions compare labels, never values, so a failing test cannot print a
# membership or invite (AGENTS.md), even a synthetic one.
LABELS = {
    MEMBERSHIP: "MEMBERSHIP",
    DAVET_DEV: "DAVET_DEV",
    DAVET_DEV_REISSUED: "DAVET_DEV_REISSUED",
    DAVET_GONE: "DAVET_GONE",
    DAVET_OTHER: "DAVET_OTHER",
}

NEEDS_FRESH = "needs a fresh invite — ask the Master to re-send"

# Long enough for the listener to start rooms and finish its deferred seat
# check (shortened below for the test run), short enough to keep the suite fast.
LOBBY_HOLD_SECONDS = 2.0
SEAT_REPORT_GRACE_SECONDS = 0.5


def redact(text):
    for secret, label in LABELS.items():
        text = text.replace(secret, f"<{label}>")
    return text


def invite_id(token):
    """The server's ``ivid_`` (``crates/server/src/hub.rs`` invite_management_id)."""
    return "ivid_" + hashlib.sha256(("invite:" + token).encode()).hexdigest()[:32]


def ws_text_frame(text):
    payload = text.encode()
    if len(payload) < 126:
        header = bytes([0x81, len(payload)])
    else:
        header = bytes([0x81, 126]) + len(payload).to_bytes(2, "big")
    return header + payload


class DaemonHTTPServer(ThreadingHTTPServer):
    daemon_threads = True


class FakeBuilding:
    """A scripted Building: one frame list per Lobby connection.

    A Lobby connection beyond the scripted ones is refused with 401, which
    ends the listener cleanly (``membership rejected``).  Loca sockets are
    accepted only for the invite the test declares live and held open, so a
    running loca listener shows up as exactly one recorded connection.
    """

    def __init__(self, lobby_scripts, *, live_invites=None, whoami=None, lobby_gate=None):
        self.lobby_scripts = list(lobby_scripts)
        # lobby_gate(accepted_so_far) -> False refuses this Lobby handshake
        # with 401 (what an older server answers a suspended membership)
        # before any script is used; a (status, body) pair refuses it with
        # that answer instead (403 b"membership suspended" is the current
        # server's answer to a suspended membership).
        self.lobby_gate = lobby_gate
        self.lobby_refusals = []  # monotonic time of each refused handshake
        # Monotonic time taken just before the latest scripted Lobby frame
        # was written: the listener cannot have seen that frame earlier.
        self.last_frame_at = None
        self.live_invites = dict(live_invites or {})
        # (status, body) or None for "connection dropped".
        self.whoami = whoami if whoami is not None else (500, b"server error")
        self.lock = threading.Lock()
        self.lobby_connections = 0
        self.room_connections = []  # (room, accepted)
        self.paths = []
        building = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def _send(self, status, body=b"", content_type="application/json"):
                self.send_response(status)
                self.send_header("content-type", content_type)
                self.send_header("content-length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def _upgrade(self):
                key = self.headers.get("Sec-WebSocket-Key", "")
                accept = base64.b64encode(
                    hashlib.sha1(
                        (key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()
                    ).digest()
                ).decode()
                self.send_response(101, "Switching Protocols")
                self.send_header("Upgrade", "websocket")
                self.send_header("Connection", "Upgrade")
                self.send_header("Sec-WebSocket-Accept", accept)
                self.send_header("Sec-WebSocket-Protocol", "loca.v1")
                self.end_headers()
                self.wfile.flush()

            def _protocols(self):
                raw = self.headers.get("Sec-WebSocket-Protocol", "")
                return [item.strip() for item in raw.split(",") if item.strip()]

            def do_GET(self):
                parsed = urlparse(self.path)
                with building.lock:
                    building.paths.append(parsed.path)
                if parsed.path == "/lobby/ws":
                    self._lobby()
                elif parsed.path == "/ws":
                    self._room(parse_qs(parsed.query).get("room", [""])[0])
                elif parsed.path == "/whoami":
                    if self.headers.get("x-room-token") != MEMBERSHIP:
                        self._send(401, b"no davet", "text/plain")
                    elif building.whoami is None:
                        self.close_connection = True
                        self.connection.close()
                    else:
                        status, body = building.whoami
                        self._send(status, body)
                elif parsed.path.endswith("/settings"):
                    self._send(200, b"{}")
                else:
                    self._send(404, b'{"error":"not found"}')

            def do_POST(self):
                with building.lock:
                    building.paths.append(urlparse(self.path).path)
                length = int(self.headers.get("content-length", "0"))
                self.rfile.read(length)
                self._send(401, b"davet required", "text/plain")

            def _lobby(self):
                if f"loca.membership.{MEMBERSHIP}" not in self._protocols():
                    self._send(401, b"invalid membership", "text/plain")
                    return
                with building.lock:
                    index = building.lobby_connections
                    verdict = True if building.lobby_gate is None else building.lobby_gate(index)
                    refused = verdict is not True
                    if refused:
                        building.lobby_refusals.append(time.monotonic())
                    else:
                        building.lobby_connections += 1
                if refused:
                    status, body = verdict or (401, b"invalid membership")
                    self._send(status, body, "text/plain")
                    return
                if index >= len(building.lobby_scripts):
                    self._send(401, b"invalid membership", "text/plain")
                    return
                self._upgrade()
                try:
                    for frame in building.lobby_scripts[index]:
                        with building.lock:
                            building.last_frame_at = time.monotonic()
                        self.wfile.write(ws_text_frame(json.dumps(frame)))
                        self.wfile.flush()
                        time.sleep(0.05)
                    time.sleep(LOBBY_HOLD_SECONDS)
                    self.wfile.write(bytes([0x88, 0x00]))
                    self.wfile.flush()
                except OSError:
                    pass
                self.close_connection = True

            def _room(self, room):
                presented = [
                    item[len("loca.room."):]
                    for item in self._protocols()
                    if item.startswith("loca.room.")
                ]
                live = building.live_invites.get(room)
                # One invite, or several a test declares live at once.
                live = (live,) if isinstance(live, str) else tuple(live or ())
                accepted = bool(presented and presented[0] in live)
                with building.lock:
                    building.room_connections.append((room, accepted))
                if not accepted:
                    self._send(401, b"no davet", "text/plain")
                    return
                self._upgrade()
                # Hold the seat open past the end of the test.
                time.sleep(LOBBY_HOLD_SECONDS * 4)
                self.close_connection = True

        self.server = DaemonHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    @property
    def origin(self):
        return f"http://127.0.0.1:{self.server.server_port}"

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *_):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=2)

    def rooms_entered(self, room):
        with self.lock:
            return sum(1 for name, ok in self.room_connections if name == room and ok)

    def rooms_attempted(self):
        with self.lock:
            return [name for name, _ in self.room_connections]


class LobbySeatTests(unittest.TestCase):
    maxDiff = None

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.home = Path(self._tmp.name)
        (self.home / ".loca").mkdir()
        self.identity = self.home / ".loca" / "worker.env"
        self.status_file = Path(str(self.identity) + ".davet-status.json")
        self.outputs = []

    def tearDown(self):
        self._tmp.cleanup()

    def write_identity(self, origin, **invites):
        lines = [
            f"ROOM_SERVER_URL={origin}",
            "LOCA_NAME=worker",
            f"LOCA_MEMBERSHIP={MEMBERSHIP}",
        ]
        lines += [f"DAVET_{key}={value}" for key, value in invites.items()]
        self.identity.write_text("\n".join(lines) + "\n", encoding="utf-8")
        self.identity.chmod(0o600)

    def kept(self):
        values = {}
        for line in self.identity.read_text(encoding="utf-8").splitlines():
            key, _, value = line.partition("=")
            if key.startswith("DAVET_"):
                values[key[len("DAVET_"):]] = LABELS.get(value, "<unknown value>")
        return values

    def run_listener(self, building, timeout=40, overrides=None):
        url = building.origin.replace("http://", "ws://") + (
            "/ws?room=&name=worker&type=agent"
        )
        out = self.home / "inbox.jsonl"
        env = {
            "HOME": str(self.home),
            "PATH": os.environ.get("PATH", ""),
            "PYTHONIOENCODING": "utf-8",
        }
        # Run the real listener with a shorter deferred seat check; the
        # production value is a module constant, not an environment knob.
        runner = (
            "import sys; sys.path.insert(0, sys.argv[1]); import listen; "
            f"listen.SEAT_REPORT_GRACE_SECONDS = {SEAT_REPORT_GRACE_SECONDS}; "
            + "".join(f"listen.{k} = {v!r}; " for k, v in (overrides or {}).items())
            +             "sys.argv = sys.argv[1:]; sys.argv[0] = 'listen.py'; "
            "raise SystemExit(listen.main())"
        )
        result = subprocess.run(
            [sys.executable, "-c", runner, str(SKILL_DIR), url, str(out)],
            env=env,
            cwd=str(self.home),
            text=True,
            capture_output=True,
            timeout=timeout,
            check=False,
        )
        self.outputs.append(result.stdout)
        self.outputs.append(result.stderr)
        if out.exists():
            self.outputs.append(out.read_text(encoding="utf-8", errors="replace"))
        return result

    def assert_no_secret_leaked(self):
        texts = list(self.outputs)
        if self.status_file.exists():
            texts.append(self.status_file.read_text(encoding="utf-8"))
        for text in texts:
            for secret in SECRETS:
                # Never echo the secret in the failure message either.
                self.assertFalse(
                    secret in text,
                    "a synthetic membership/invite value reached listener output",
                )

    def needs_fresh_rooms(self):
        if not self.status_file.exists():
            return []
        return json.loads(self.status_file.read_text(encoding="utf-8")).get(
            "needs_fresh_davet", []
        )

    # -- the current server: `invites` in lobby_ready (compatibility guards) --

    def test_old_server_live_call_stores_and_starts_the_loca(self):
        script = [[
            {"t": "lobby_ready", "name": "worker", "invites": []},
            {"t": "called", "room": "sb-dev", "token": DAVET_DEV},
        ]]
        with FakeBuilding(script, live_invites={"sb-dev": DAVET_DEV}) as building:
            self.write_identity(building.origin)
            result = self.run_listener(building)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(self.kept(), {"sb_dev": "DAVET_DEV"})
        self.assertEqual(building.rooms_entered("sb-dev"), 1)
        self.assertIn("[lobby] called into sb-dev", result.stderr)
        self.assertNotIn("/whoami", building.paths)
        self.assert_no_secret_leaked()

    def test_old_server_invites_snapshot_starts_listed_once_and_drops_the_rest(self):
        # Today's server: the snapshot, then the same invite replayed as
        # `called`.  One listener, the absent loca's line dropped, no /whoami.
        snapshot = [{"room": "sb-dev", "token": DAVET_DEV}]
        script = [[
            {"t": "lobby_ready", "name": "worker", "invites": snapshot},
            {"t": "called", "room": "sb-dev", "token": DAVET_DEV},
            {"t": "called", "room": "sb-dev", "token": DAVET_DEV},
        ]]
        with FakeBuilding(script, live_invites={"sb-dev": DAVET_DEV}) as building:
            self.write_identity(building.origin, sb_dev=DAVET_DEV, gone=DAVET_GONE)
            result = self.run_listener(building)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(self.kept(), {"sb_dev": "DAVET_DEV"})
        self.assertEqual(building.rooms_entered("sb-dev"), 1)
        self.assertEqual(building.rooms_attempted(), ["sb-dev"])
        self.assertIn("removed stale local invite cache: gone", result.stderr)
        self.assertNotIn("/whoami", building.paths)
        self.assertNotIn(NEEDS_FRESH, result.stderr)
        self.assert_no_secret_leaked()

    def test_old_server_snapshot_replaces_a_reissued_invite(self):
        snapshot = [{"room": "sb-dev", "token": DAVET_DEV_REISSUED}]
        script = [[
            {"t": "lobby_ready", "name": "worker", "invites": snapshot},
            {"t": "called", "room": "sb-dev", "token": DAVET_DEV_REISSUED},
        ]]
        live = {"sb-dev": DAVET_DEV_REISSUED}
        with FakeBuilding(script, live_invites=live) as building:
            self.write_identity(building.origin, sb_dev=DAVET_DEV)
            result = self.run_listener(building)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(self.kept(), {"sb_dev": "DAVET_DEV_REISSUED"})
        self.assertEqual(building.rooms_entered("sb-dev"), 1)
        self.assert_no_secret_leaked()

    def test_old_server_empty_invites_list_drops_every_kept_invite(self):
        # Design §0.2: an old server's empty list is authoritative.  Pinned so
        # the new code cannot quietly change what an empty `invites` means.
        script = [[{"t": "lobby_ready", "name": "worker", "invites": []}]]
        with FakeBuilding(script) as building:
            self.write_identity(building.origin, sb_dev=DAVET_DEV, gone=DAVET_GONE)
            result = self.run_listener(building)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(self.kept(), {})
        self.assertEqual(building.rooms_attempted(), [])
        self.assertNotIn("/whoami", building.paths)
        self.assert_no_secret_leaked()

    # -- the coming server: `seats`, no `invites` key ------------------------

    def test_new_server_seat_starts_the_loca_from_the_kept_invite(self):
        seats = [{"room": "sb-dev", "invite_id": invite_id(DAVET_DEV)}]
        script = [[{"t": "lobby_ready", "name": "worker", "seats": seats}]]
        with FakeBuilding(script, live_invites={"sb-dev": DAVET_DEV}) as building:
            self.write_identity(building.origin, sb_dev=DAVET_DEV)
            result = self.run_listener(building)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(building.rooms_entered("sb-dev"), 1)
        self.assertEqual(self.kept(), {"sb_dev": "DAVET_DEV"})
        self.assertNotIn(NEEDS_FRESH, result.stderr)
        self.assertEqual(self.needs_fresh_rooms(), [])
        self.assert_no_secret_leaked()

    def test_listener_restart_while_seated_re_enters_from_the_kept_invite(self):
        # Design §0.1: the invite arrives live once; after a restart the coming
        # server sends only the seat.  Both runs must reach the loca.
        dev_id = invite_id(DAVET_DEV)
        first = [[
            {"t": "lobby_ready", "name": "worker", "member_id": "mbid_x", "seats": []},
            {"t": "called", "room": "sb-dev", "token": DAVET_DEV, "invite_id": dev_id},
        ]]
        second = [[
            {
                "t": "lobby_ready",
                "name": "worker",
                "member_id": "mbid_x",
                "seats": [{"room": "sb-dev", "invite_id": dev_id}],
            },
        ]]
        live = {"sb-dev": DAVET_DEV}
        with FakeBuilding(first, live_invites=live) as building:
            self.write_identity(building.origin)
            result = self.run_listener(building)
            self.assertEqual(result.returncode, 0)
            self.assertEqual(building.rooms_entered("sb-dev"), 1)
            self.assertEqual(self.kept(), {"sb_dev": "DAVET_DEV"})
        with FakeBuilding(second, live_invites=live) as building:
            # Same identity file, new port: only ROOM_SERVER_URL changes.
            lines = self.identity.read_text(encoding="utf-8").splitlines()
            lines = [
                f"ROOM_SERVER_URL={building.origin}"
                if line.startswith("ROOM_SERVER_URL=") else line
                for line in lines
            ]
            self.identity.write_text("\n".join(lines) + "\n", encoding="utf-8")
            result = self.run_listener(building)
            self.assertEqual(result.returncode, 0)
            self.assertEqual(building.rooms_entered("sb-dev"), 1)
        self.assertEqual(self.kept(), {"sb_dev": "DAVET_DEV"})
        self.assert_no_secret_leaked()

    def test_seat_with_a_stale_invite_id_is_reported_not_started_not_deleted(self):
        seats = [{"room": "sb-dev", "invite_id": invite_id(DAVET_DEV_REISSUED)}]
        script = [[{"t": "lobby_ready", "name": "worker", "seats": seats}]]
        with FakeBuilding(script, live_invites={"sb-dev": DAVET_DEV_REISSUED}) as building:
            self.write_identity(building.origin, sb_dev=DAVET_DEV)
            result = self.run_listener(building)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(building.rooms_attempted(), [])
        self.assertEqual(self.kept(), {"sb_dev": "DAVET_DEV"})
        self.assertEqual(result.stderr.count(NEEDS_FRESH), 1, redact(result.stderr))
        self.assertIn("sb-dev", result.stderr)
        self.assertEqual(self.needs_fresh_rooms(), ["sb-dev"])
        self.assert_no_secret_leaked()

    def test_seat_without_a_kept_invite_is_reported_once_across_reconnects(self):
        seats = [{"room": "sb-dev", "invite_id": invite_id(DAVET_DEV)}]
        ready = {"t": "lobby_ready", "name": "worker", "seats": seats}
        with FakeBuilding([[ready], [ready]]) as building:
            self.write_identity(building.origin)
            result = self.run_listener(building)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(building.lobby_connections, 3)
        self.assertEqual(building.rooms_attempted(), [])
        self.assertEqual(result.stderr.count(NEEDS_FRESH), 1, redact(result.stderr))
        self.assertEqual(self.needs_fresh_rooms(), ["sb-dev"])
        self.assert_no_secret_leaked()

    def test_a_live_call_resolves_a_seat_that_needed_a_fresh_invite(self):
        # The deferred check waits for replays (ruling 5a): a `called` that
        # follows `lobby_ready` must not be reported as missing.
        reissued_id = invite_id(DAVET_DEV_REISSUED)
        script = [[
            {"t": "lobby_ready", "name": "worker",
             "seats": [{"room": "sb-dev", "invite_id": reissued_id}]},
            {"t": "called", "room": "sb-dev", "token": DAVET_DEV_REISSUED,
             "invite_id": reissued_id},
        ]]
        live = {"sb-dev": DAVET_DEV_REISSUED}
        with FakeBuilding(script, live_invites=live) as building:
            self.write_identity(building.origin, sb_dev=DAVET_DEV)
            result = self.run_listener(building)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(building.rooms_entered("sb-dev"), 1)
        self.assertEqual(building.rooms_attempted(), ["sb-dev"])
        self.assertEqual(self.kept(), {"sb_dev": "DAVET_DEV_REISSUED"})
        self.assertNotIn(NEEDS_FRESH, result.stderr)
        self.assertEqual(self.needs_fresh_rooms(), [])
        self.assert_no_secret_leaked()

    def test_two_reissues_delivered_in_order_leave_the_last_invite_kept(self):
        # Two re-issues of one seat reach the Lobby in the order their invites
        # became live (the server holds the invite lock from the swap through
        # the delivery). The first is still accepted when its listener
        # connects; the kept invite must be the second, the one the server
        # holds afterwards.
        script = [[
            {"t": "lobby_ready", "name": "worker", "invites": []},
            {"t": "called", "room": "sb-dev", "token": DAVET_DEV},
            {"t": "called", "room": "sb-dev", "token": DAVET_DEV_REISSUED},
        ]]
        live = {"sb-dev": (DAVET_DEV, DAVET_DEV_REISSUED)}
        with FakeBuilding(script, live_invites=live) as building:
            self.write_identity(building.origin)
            result = self.run_listener(building)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(self.kept(), {"sb_dev": "DAVET_DEV_REISSUED"})
        self.assertEqual(result.stderr.count("[lobby] called into sb-dev"), 2)
        self.assert_no_secret_leaked()

    def test_seat_list_drops_lines_for_rooms_no_longer_listed(self):
        seats = [{"room": "sb-dev", "invite_id": invite_id(DAVET_DEV)}]
        script = [[{"t": "lobby_ready", "name": "worker", "seats": seats}]]
        with FakeBuilding(script, live_invites={"sb-dev": DAVET_DEV}) as building:
            self.write_identity(building.origin, sb_dev=DAVET_DEV, gone=DAVET_GONE)
            result = self.run_listener(building)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(self.kept(), {"sb_dev": "DAVET_DEV"})
        self.assertIn("removed stale local invite cache: gone", result.stderr)
        self.assertEqual(building.rooms_entered("sb-dev"), 1)
        self.assertNotIn("/whoami", building.paths)
        self.assert_no_secret_leaked()

    def test_an_empty_seat_list_keeps_no_invite(self):
        script = [[{"t": "lobby_ready", "name": "worker", "seats": []}]]
        with FakeBuilding(script) as building:
            self.write_identity(building.origin, sb_dev=DAVET_DEV, gone=DAVET_GONE)
            result = self.run_listener(building)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(self.kept(), {})
        self.assertEqual(building.rooms_attempted(), [])
        self.assert_no_secret_leaked()

    def test_malformed_seats_fall_back_to_whoami_and_never_delete(self):
        script = [[{"t": "lobby_ready", "name": "worker",
                    "seats": [{"room": "sb-dev"}]}]]
        with FakeBuilding(script) as building:
            self.write_identity(building.origin, sb_dev=DAVET_DEV, gone=DAVET_GONE)
            result = self.run_listener(building)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(self.kept(), {"sb_dev": "DAVET_DEV", "gone": "DAVET_GONE"})
        self.assertEqual(building.rooms_attempted(), [])
        self.assertIn("/whoami", building.paths)
        self.assert_no_secret_leaked()

    def test_no_seat_information_and_failing_whoami_delete_nothing(self):
        # No `invites`, no `seats`; /whoami fails in every way the network
        # can fail.  None of them may cost a kept invite.
        failures = {
            "5xx": (503, b"unavailable"),
            "unparsable": (200, b"<html>proxy error</html>"),
            "partial": (200, json.dumps({"kind": "member", "name": "worker"}).encode()),
            "dropped": None,
        }
        for label, whoami in failures.items():
            with self.subTest(whoami=label):
                self.outputs = []
                script = [[{"t": "lobby_ready", "name": "worker"}]]
                with FakeBuilding(script, whoami=whoami) as building:
                    self.write_identity(
                        building.origin, sb_dev=DAVET_DEV, gone=DAVET_GONE
                    )
                    result = self.run_listener(building)
                self.assertEqual(result.returncode, 0)
                self.assertEqual(
                    self.kept(), {"sb_dev": "DAVET_DEV", "gone": "DAVET_GONE"}
                )
                self.assertIn("/whoami", building.paths)
                self.assertEqual(building.rooms_attempted(), [])
                self.assert_no_secret_leaked()

    def test_whoami_fallback_keeps_listed_invites_and_drops_the_rest(self):
        body = json.dumps(
            {"kind": "member", "name": "worker", "locas": ["sb-dev"]}
        ).encode()
        script = [[{"t": "lobby_ready", "name": "worker"}]]
        with FakeBuilding(
            script, whoami=(200, body), live_invites={"sb-dev": DAVET_DEV}
        ) as building:
            self.write_identity(building.origin, sb_dev=DAVET_DEV, gone=DAVET_GONE)
            result = self.run_listener(building)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(self.kept(), {"sb_dev": "DAVET_DEV"})
        self.assertIn("removed stale local invite cache: gone", result.stderr)
        self.assert_no_secret_leaked()

    def test_whoami_fallback_starts_listed_locas_from_kept_invites(self):
        body = json.dumps(
            {"kind": "member", "name": "worker", "locas": ["sb-dev", "other"]}
        ).encode()
        script = [[{"t": "lobby_ready", "name": "worker"}]]
        with FakeBuilding(
            script, whoami=(200, body), live_invites={"sb-dev": DAVET_DEV}
        ) as building:
            self.write_identity(building.origin, sb_dev=DAVET_DEV)
            result = self.run_listener(building)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(building.rooms_entered("sb-dev"), 1)
        self.assertEqual(building.rooms_attempted(), ["sb-dev"])
        self.assertEqual(result.stderr.count(NEEDS_FRESH), 1, redact(result.stderr))
        self.assertEqual(self.needs_fresh_rooms(), ["other"])
        self.assert_no_secret_leaked()

    # -- `called` with an invite the kit already keeps --------------------------

    def test_same_token_call_with_the_listener_running_keeps_one_listener(self):
        snapshot = [{"room": "sb-dev", "token": DAVET_DEV}]
        call = {"t": "called", "room": "sb-dev", "token": DAVET_DEV}
        script = [[
            {"t": "lobby_ready", "name": "worker", "invites": snapshot},
            call, call, call,
        ]]
        with FakeBuilding(script, live_invites={"sb-dev": DAVET_DEV}) as building:
            self.write_identity(building.origin, sb_dev=DAVET_DEV)
            result = self.run_listener(building)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(building.rooms_attempted(), ["sb-dev"])
        self.assertNotIn("Traceback", result.stderr)
        self.assert_no_secret_leaked()

    def test_same_token_call_with_no_listener_running_starts_one(self):
        call = {"t": "called", "room": "sb-dev", "token": DAVET_DEV,
                "invite_id": invite_id(DAVET_DEV)}
        script = [[{"t": "lobby_ready", "name": "worker"}, call, call]]
        with FakeBuilding(script, live_invites={"sb-dev": DAVET_DEV}) as building:
            self.write_identity(building.origin, sb_dev=DAVET_DEV)
            result = self.run_listener(building)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(building.rooms_attempted(), ["sb-dev"])
        self.assertEqual(building.rooms_entered("sb-dev"), 1)
        self.assertEqual(self.kept(), {"sb_dev": "DAVET_DEV"})
        self.assert_no_secret_leaked()

    def test_two_lobby_connections_delivering_the_same_invite_start_one_listener(self):
        dev_id = invite_id(DAVET_DEV)
        connection = [
            {"t": "lobby_ready", "name": "worker",
             "seats": [{"room": "sb-dev", "invite_id": dev_id}]},
            {"t": "called", "room": "sb-dev", "token": DAVET_DEV, "invite_id": dev_id},
        ]
        with FakeBuilding(
            [connection, connection], live_invites={"sb-dev": DAVET_DEV}
        ) as building:
            self.write_identity(building.origin)
            result = self.run_listener(building)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(building.lobby_connections, 3)
        self.assertEqual(building.rooms_attempted(), ["sb-dev"])
        self.assertEqual(self.kept(), {"sb_dev": "DAVET_DEV"})
        self.assertNotIn(NEEDS_FRESH, result.stderr)
        self.assert_no_secret_leaked()

    def test_two_lobby_connections_with_a_kept_invite_start_one_listener(self):
        dev_id = invite_id(DAVET_DEV)
        connection = [
            {"t": "lobby_ready", "name": "worker",
             "seats": [{"room": "sb-dev", "invite_id": dev_id}]},
            {"t": "called", "room": "sb-dev", "token": DAVET_DEV, "invite_id": dev_id},
        ]
        with FakeBuilding(
            [connection, connection], live_invites={"sb-dev": DAVET_DEV}
        ) as building:
            self.write_identity(building.origin, sb_dev=DAVET_DEV)
            result = self.run_listener(building)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(building.lobby_connections, 3)
        self.assertEqual(building.rooms_attempted(), ["sb-dev"])
        self.assert_no_secret_leaked()

    # -- door B: the membership key itself was revoked ------------------------

    def test_credential_revoked_stops_the_lobby_without_retrying(self):
        script = [[
            {"t": "lobby_ready", "name": "worker",
             "seats": [{"room": "sb-dev", "invite_id": invite_id(DAVET_DEV)}]},
            {"t": "credential_revoked", "credential_id": "cr_0123456789abcdef",
             "reason": "revoked from the profile page", "future_field": [1, 2]},
        ]]
        with FakeBuilding(script, live_invites={"sb-dev": DAVET_DEV}) as building:
            self.write_identity(building.origin, sb_dev=DAVET_DEV)
            result = self.run_listener(building)
            finished = time.monotonic()
        self.assertEqual(result.returncode, 0)
        # No retry: a second Lobby handshake would have been counted.
        self.assertEqual(building.lobby_connections, 1)
        # It stopped on the frame, before the server ended the connection
        # (the server holds it LOBBY_HOLD_SECONDS after its last frame).
        # Counted from that frame, not from the process start, so a slow
        # interpreter start on a busy host does not count against it.
        self.assertLess(finished - building.last_frame_at, LOBBY_HOLD_SECONDS)
        self.assertIn("membership key was revoked", result.stderr)
        self.assertIn("not retrying", result.stderr)
        self.assertNotIn("Traceback", result.stderr)
        # The revoked key is not replaced behind the operator's back, and the
        # invites (which survive door B) stay kept.
        saved = self.identity.read_text(encoding="utf-8")
        self.assertEqual(saved.count("LOCA_MEMBERSHIP="), 1)
        self.assertEqual(self.kept(), {"sb_dev": "DAVET_DEV"})
        self.assertNotIn("/membership/claim", building.paths)
        self.assert_no_secret_leaked()

    # -- membership suspension: reversible, so the listener waits for Resume --

    SUSPENDED_WINDOW_SECONDS = 3.0
    SUSPENSION_OVERRIDES = {
        "SUSPENDED_RETRY_BASE_SECONDS": 0.25,
        "SUSPENDED_RETRY_MAX_SECONDS": 1.0,
    }

    def suspended_building(self, after_resume, **kwargs):
        """Suspend on the first Lobby connection, refuse with 401 (as the
        server does for a disabled principal) for a window, then resume."""
        window = {"resume_at": None}

        def gate(accepted):
            if accepted == 0:
                return True
            if window["resume_at"] is None:
                window["resume_at"] = time.monotonic() + self.SUSPENDED_WINDOW_SECONDS
            return time.monotonic() >= window["resume_at"]

        script = [
            [
                {"t": "lobby_ready", "name": "worker",
                 "invites": [{"room": "sb-dev", "token": DAVET_DEV}]},
                {"t": "called", "room": "sb-dev", "token": DAVET_DEV},
                {"t": "membership_suspended"},
            ],
            after_resume,
        ]
        # The loca socket stays open (the fake sends no `kicked`), so only the
        # Lobby path can touch the kept invite.
        return FakeBuilding(
            script, lobby_gate=gate, live_invites={"sb-dev": DAVET_DEV}, **kwargs
        )

    def test_suspension_is_said_and_the_listener_returns_after_resume(self):
        after_resume = [{"t": "lobby_ready", "name": "worker", "invites": []}]
        with self.suspended_building(after_resume) as building:
            self.write_identity(building.origin)
            result = self.run_listener(building, overrides=self.SUSPENSION_OVERRIDES)
            refusals = list(building.lobby_refusals)
        stderr = result.stderr
        self.assertIn("membership was suspended by the Master", stderr)
        self.assertIn("resumes when the Master resumes it", stderr)
        self.assertIn("a new call is needed", stderr)
        self.assertNotIn("membership rejected: ", stderr.split("resumed")[0])
        # Waited without hammering: a bounded, growing backoff, not one
        # handshake per frame-loop turn, and none after the Lobby came back.
        self.assertGreaterEqual(len(refusals), 2)
        self.assertLessEqual(len(refusals), 6, f"{len(refusals)} handshakes in the window")
        gaps = [b - a for a, b in zip(refusals, refusals[1:])]
        self.assertTrue(all(gap >= 0.2 for gap in gaps), gaps)
        # Back in the Lobby without a restart: the second scripted connection
        # was used by the same process.
        # Two scripted connections, then the end of the script (401).
        self.assertEqual(building.lobby_connections, 3)
        self.assertEqual(stderr.count("[lobby] connected"), 2)
        self.assertIn("membership resumed", stderr)
        # The key is kept; the dead invite is dropped only by the
        # authoritative snapshot after Resume, never by the suspension.
        saved = self.identity.read_text(encoding="utf-8")
        self.assertIn("LOCA_MEMBERSHIP=" + MEMBERSHIP, saved)
        self.assertEqual(self.kept(), {})
        after = stderr.split("membership resumed", 1)[1]
        self.assertIn("removed stale local invite cache: sb_dev", after)
        self.assertNotIn("Traceback", stderr)
        self.assertEqual(result.returncode, 0)
        self.assert_no_secret_leaked()

    def test_suspension_keeps_kept_invites_when_the_answer_after_resume_is_not_authoritative(self):
        # After Resume a seat-shaped server sends no seat list and /whoami
        # fails: nothing may be deleted on that answer.
        after_resume = [{"t": "lobby_ready", "name": "worker"}]
        with self.suspended_building(after_resume) as building:
            self.write_identity(building.origin)
            result = self.run_listener(building, overrides=self.SUSPENSION_OVERRIDES)
        self.assertIn("membership resumed", result.stderr)
        self.assertEqual(self.kept(), {"sb_dev": "DAVET_DEV"})
        self.assertIn("LOCA_MEMBERSHIP=" + MEMBERSHIP, self.identity.read_text(encoding="utf-8"))
        self.assert_no_secret_leaked()

    def test_suspension_status_is_written_for_connect_status_and_cleared_on_resume(self):
        observed = {}

        def gate(accepted):
            if accepted == 0:
                return True
            if "resume_at" not in observed:
                observed["resume_at"] = time.monotonic() + self.SUSPENDED_WINDOW_SECONDS
            if self.status_file.exists():
                observed["while_suspended"] = json.loads(
                    self.status_file.read_text(encoding="utf-8")
                )
            return time.monotonic() >= observed["resume_at"]

        script = [
            [{"t": "lobby_ready", "name": "worker", "invites": []},
             {"t": "membership_suspended"}],
            [{"t": "lobby_ready", "name": "worker", "invites": []}],
        ]
        with FakeBuilding(script, lobby_gate=gate) as building:
            self.write_identity(building.origin)
            self.run_listener(building, overrides=self.SUSPENSION_OVERRIDES)
        self.assertEqual(observed.get("while_suspended"), {"membership_suspended": True})
        self.assertFalse(self.status_file.exists())

    SUSPENDED_AT_DOOR = (403, b"membership suspended")

    def test_a_listener_started_while_suspended_waits_and_returns_after_resume(self):
        # The current server says "suspended" at the handshake, so a listener
        # that never saw the frame waits for Resume instead of stopping.
        observed = {}

        def gate(accepted):
            if "resume_at" not in observed:
                observed["resume_at"] = time.monotonic() + self.SUSPENDED_WINDOW_SECONDS
            if self.status_file.exists():
                observed["while_suspended"] = json.loads(
                    self.status_file.read_text(encoding="utf-8")
                )
            if time.monotonic() >= observed["resume_at"]:
                return True
            return self.SUSPENDED_AT_DOOR

        script = [[{"t": "lobby_ready", "name": "worker", "invites": []}]]
        with FakeBuilding(script, lobby_gate=gate) as building:
            self.write_identity(building.origin, sb_dev=DAVET_DEV)
            result = self.run_listener(
                building, timeout=20, overrides=self.SUSPENSION_OVERRIDES
            )
            refusals = list(building.lobby_refusals)
        stderr = result.stderr
        before_resume = stderr.split("membership resumed")[0]
        self.assertIn("membership was suspended by the Master", before_resume)
        self.assertIn("resumes when the Master resumes it", before_resume)
        self.assertNotIn("membership rejected", before_resume)
        self.assertNotIn("reconnect after error", before_resume)
        self.assertEqual(observed.get("while_suspended"), {"membership_suspended": True})
        # The slow, bounded backoff, not a reconnect every two seconds.
        self.assertGreaterEqual(len(refusals), 2)
        self.assertLessEqual(len(refusals), 6, f"{len(refusals)} handshakes in the window")
        gaps = [b - a for a, b in zip(refusals, refusals[1:])]
        self.assertTrue(all(gap >= 0.2 for gap in gaps), gaps)
        # Back in the Lobby in the same process; the key is kept. One
        # scripted connection, then the end of the script (401).
        self.assertIn("membership resumed", stderr)
        self.assertEqual(building.lobby_connections, 2)
        self.assertEqual(stderr.count("[lobby] connected"), 1)
        self.assertIn("LOCA_MEMBERSHIP=" + MEMBERSHIP, self.identity.read_text(encoding="utf-8"))
        self.assertFalse(self.status_file.exists())
        self.assertEqual(self.kept(), {})
        self.assertNotIn("Traceback", stderr)
        self.assertEqual(result.returncode, 0)
        self.assert_no_secret_leaked()

    def test_a_membership_revoked_while_waiting_for_resume_stops_as_rejected(self):
        # Suspended (frame, then "suspended" at the door), then revoked: the
        # door's 401 now means revoked, and the listener stops as a listener
        # that was never suspended does, instead of waiting forever.
        def gate(accepted):
            if accepted == 0:
                return True
            if len(building.lobby_refusals) < 2:
                return self.SUSPENDED_AT_DOOR
            return False

        script = [[
            {"t": "lobby_ready", "name": "worker", "invites": []},
            {"t": "membership_suspended"},
        ]]
        with FakeBuilding(script, lobby_gate=gate) as building:
            self.write_identity(building.origin)
            started = time.monotonic()
            result = self.run_listener(
                building, timeout=15, overrides=self.SUSPENSION_OVERRIDES
            )
            elapsed = time.monotonic() - started
            refusals = len(building.lobby_refusals)
        stderr = result.stderr
        self.assertIn("membership was suspended by the Master", stderr)
        self.assertIn("[lobby] membership rejected", stderr)
        self.assertEqual(refusals, 3)
        self.assertLess(elapsed, 10)
        self.assertEqual(result.returncode, 0)
        # The note for `connect.sh status` must not keep saying "suspended".
        self.assertFalse(self.status_file.exists())
        self.assert_no_secret_leaked()

    def test_a_403_that_does_not_say_suspended_is_not_a_suspension(self):
        # Guard: a 403 from something else (a proxy) is not read as the
        # Master's suspension; a following 401 still stops the listener.
        def gate(accepted):
            if not building.lobby_refusals:
                return (403, b"forbidden")
            return False

        with FakeBuilding([], lobby_gate=gate) as building:
            self.write_identity(building.origin)
            result = self.run_listener(
                building, timeout=15, overrides=self.SUSPENSION_OVERRIDES
            )
        self.assertNotIn("suspended", result.stderr)
        self.assertIn("[lobby] membership rejected", result.stderr)
        self.assertFalse(self.status_file.exists())
        self.assertEqual(result.returncode, 0)
        self.assert_no_secret_leaked()

    def test_membership_revoked_still_stops_for_good(self):
        # Guard: a revoked (not suspended) membership erases the key and the
        # listener stops without reconnecting, exactly as before.
        script = [[
            {"t": "lobby_ready", "name": "worker", "invites": []},
            {"t": "membership_revoked"},
        ]]
        with FakeBuilding(script, lobby_gate=lambda accepted: True) as building:
            self.write_identity(building.origin)
            result = self.run_listener(building, overrides=self.SUSPENSION_OVERRIDES)
            finished = time.monotonic()
        self.assertEqual(result.returncode, 0)
        self.assertEqual(building.lobby_connections, 1)
        self.assertLess(finished - building.last_frame_at, LOBBY_HOLD_SECONDS)
        self.assertIn("building membership revoked", result.stderr)
        self.assertNotIn("suspended", result.stderr)
        self.assertNotIn(MEMBERSHIP, self.identity.read_text(encoding="utf-8"))
        self.assert_no_secret_leaked()

    def test_a_refused_lobby_without_a_suspension_frame_stops_as_before(self):
        # Guard: a 401 the listener was not told is a suspension (revoked
        # while the listener was away, or a wrong key) still stops at once.
        with FakeBuilding([], lobby_gate=lambda accepted: False) as building:
            self.write_identity(building.origin)
            result = self.run_listener(building, overrides=self.SUSPENSION_OVERRIDES)
            refusals = len(building.lobby_refusals)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(refusals, 1)
        self.assertIn("[lobby] membership rejected", result.stderr)
        self.assert_no_secret_leaked()


if __name__ == "__main__":
    unittest.main()
