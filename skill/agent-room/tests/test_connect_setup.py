import json
import os
import shutil
import subprocess
import tempfile
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


SKILL_DIR = Path(__file__).resolve().parents[1]


class MembershipHandler(BaseHTTPRequestHandler):
    session_requests = 0
    last_post_path = ""
    whoami_raw = None

    def log_message(self, *_):
        pass

    def _json(self, body, status=200):
        payload = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def do_GET(self):
        token = self.headers.get("x-room-token")
        session = self.headers.get("x-session-token")
        if self.path == "/health":
            self._json({"ok": True})
        elif self.path == "/whoami" and session == "st_live":
            self._json(
                {
                    "kind": "session",
                    "name": "debug",
                    "member_kind": "agent",
                    "loca": "sb-mobile",
                    "admin": False,
                }
            )
        elif self.path == "/rooms" and token == "mb_worker":
            self._json([])
        elif self.path == "/rooms" and token == "dv_invited":
            self._json([{"room": "sb-mobile", "members": 0}])
        elif self.path == "/whoami" and token == "mb_worker":
            self._json(
                {
                    "kind": "member",
                    "name": "worker",
                    "member": "mb_worker",
                    "locas": [],
                }
            )
        elif (
            self.path == "/whoami"
            and token == "mb_invited"
            and type(self).whoami_raw is not None
        ):
            # A member answer exactly as given, for incomplete-answer tests.
            payload = type(self).whoami_raw
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
        elif self.path == "/whoami" and token == "mb_invited":
            self._json(
                {
                    "kind": "member",
                    "name": "debug",
                    "member_kind": "agent",
                    "locas": ["sb-mobile"],
                }
            )
        elif self.path == "/whoami" and token in ("mb_suspended_now", "mb_forbidden"):
            # What the server answers a suspended member's own key, and a 403
            # that is not that answer (a proxy, say).
            payload = (
                b"membership suspended" if token == "mb_suspended_now" else b"forbidden"
            )
            self.send_response(403)
            self.send_header("content-type", "text/plain")
            self.send_header("content-length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
        elif self.path == "/whoami" and token == "dv_live":
            self._json({"kind": "davet", "loca": "sb-mobile"})
        elif self.path == "/whoami" and token == "dv_invited":
            self._json({"kind": "davet", "loca": "sb-mobile"})
        else:
            self._json({"error": "unauthorized"}, 401)

    def do_POST(self):
        type(self).last_post_path = self.path
        if (
            self.path == "/membership/claim"
            and self.headers.get("x-room-token") == "dv_invited"
        ):
            self._json(
                {
                    "membership_token": "mb_invited",
                    "name": "debug",
                    "kind": "agent",
                }
            )
        elif self.path == "/sessions":
            type(self).session_requests += 1
            length = int(self.headers.get("content-length", "0"))
            self.rfile.read(length)
            # Match a closed real server: a Building membership is not a loca
            # credential, and this response is intentionally plain text.
            payload = b"davet required"
            self.send_response(401)
            self.send_header("content-type", "text/plain")
            self.send_header("content-length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
        else:
            self._json({"error": "not found"}, 404)


class MembershipOnlySetupTests(unittest.TestCase):
    def test_attention_resolve_is_an_owner_command_without_internal_flag(self):
        server = ThreadingHTTPServer(("127.0.0.1", 0), MembershipHandler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            with tempfile.TemporaryDirectory() as tmp:
                env = dict(os.environ)
                env.update({
                    "HOME": tmp,
                    "LOCA_NAME": "debug",
                    "DAVET_sb_mobile": "dv_live",
                    "LOCA_INTERNAL_ATTENTION": "0",
                })
                origin = f"http://127.0.0.1:{server.server_port}"
                result = subprocess.run(
                    [
                        str(SKILL_DIR / "connect.sh"),
                        "attention-resolve",
                        origin,
                        "sb-mobile",
                        "debug",
                        "attention:sb-mobile:silence:123",
                    ],
                    env=env,
                    text=True,
                    capture_output=True,
                    timeout=10,
                    check=False,
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertNotIn("internal attention lifecycle", result.stderr)
                self.assertEqual(
                    MembershipHandler.last_post_path,
                    "/rooms/sb-mobile/attentions/attention%3Asb-mobile%3Asilence%3A123/resolve",
                )
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)

    def test_status_without_identity_gives_one_actionable_onboarding_path(self):
        with tempfile.TemporaryDirectory() as tmp:
            # Hermetic: strip any loca identity the caller's shell already holds,
            # or a real operator running the suite would see their own membership
            # here instead of the no-identity path.
            env = {
                k: v for k, v in os.environ.items()
                if not (k.startswith("LOCA_") or k.startswith("ROOM_")
                        or k.startswith("DAVET_"))
            }
            env["HOME"] = tmp
            result = subprocess.run(
                [
                    str(SKILL_DIR / "connect.sh"),
                    "status",
                    "https://loca.example",
                    "mihenk",
                ],
                env=env,
                text=True,
                capture_output=True,
                check=False,
            )

            self.assertEqual(result.returncode, 3)
            self.assertIn("no Building identity for 'mihenk'", result.stderr)
            self.assertIn("membership (Lobby) or invite (one loca)", result.stderr)
            self.assertIn("do not reuse another agent's env", result.stderr)

    def _run_invite_diagnostic(
        self,
        command,
        invite,
        *,
        stale_extra=False,
        session="",
        hide_processes=False,
        crlf_jq=False,
        needs_fresh=None,
        extra_args=(),
        whoami_raw=None,
        membership="mb_invited",
        suspension_noted=False,
    ):
        handler = (
            MembershipHandler
            if whoami_raw is None
            else type("IncompleteWhoami", (MembershipHandler,), {"whoami_raw": whoami_raw})
        )
        server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            with tempfile.TemporaryDirectory() as tmp:
                loca_dir = Path(tmp) / ".loca"
                loca_dir.mkdir()
                origin = f"http://127.0.0.1:{server.server_port}"
                env_file = loca_dir / "debug.env"
                env_file.write_text(
                    f"ROOM_SERVER_URL={origin}\n"
                    "LOCA_NAME=debug\n"
                    f"LOCA_MEMBERSHIP={membership}\n"
                    f"DAVET_sb_mobile={invite}\n"
                    + (f"LOCA_SESSION={session}\n" if session else "")
                    + ("DAVET_reviewer=dv_stale\n" if stale_extra else ""),
                    encoding="utf-8",
                )
                if suspension_noted:
                    # What listen.py writes on `membership_suspended`.
                    Path(str(env_file) + ".davet-status.json").write_text(
                        json.dumps({"membership_suspended": True}),
                        encoding="utf-8",
                    )
                if needs_fresh is not None:
                    Path(str(env_file) + ".davet-status.json").write_text(
                        json.dumps({"needs_fresh_davet": needs_fresh}),
                        encoding="utf-8",
                    )
                env = dict(os.environ)
                env.update({"HOME": tmp, "LOCA_ENV": str(env_file)})
                if hide_processes:
                    bin_dir = Path(tmp) / "bin"
                    bin_dir.mkdir()
                    fake_pgrep = bin_dir / "pgrep"
                    fake_pgrep.write_text("#!/bin/sh\nexit 1\n", encoding="utf-8")
                    fake_pgrep.chmod(0o700)
                    env["PATH"] = f"{bin_dir}:{env['PATH']}"
                if crlf_jq:
                    real_jq = shutil.which("jq", path=env["PATH"])
                    self.assertIsNotNone(real_jq)
                    bin_dir = Path(tmp) / "crlf-bin"
                    bin_dir.mkdir()
                    fake_jq = bin_dir / "jq"
                    fake_jq.write_text(
                        "#!/bin/sh\n"
                        f"'{real_jq}' \"$@\" | sed 's/$/\\r/'\n",
                        encoding="utf-8",
                    )
                    fake_jq.chmod(0o700)
                    env["PATH"] = f"{bin_dir}:{env['PATH']}"
                args = [str(SKILL_DIR / "connect.sh"), command, origin]
                if command in ("status", "reconnect"):
                    args.append("debug")
                args.extend(extra_args)
                result = subprocess.run(
                    args,
                    env=env,
                    text=True,
                    capture_output=True,
                    timeout=10,
                    check=False,
                )
                # Inspected by key only; never print the file (it holds keys).
                result.saved_keys = {
                    line.split("=", 1)[0]
                    for line in env_file.read_text(encoding="utf-8").splitlines()
                    if "=" in line
                }
                return result
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)

    def test_status_verifies_the_exact_local_invite(self):
        result = self._run_invite_diagnostic("status", "dv_live")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("INVITED (davet verified)", result.stdout)
        self.assertIn("sb-mobile", result.stdout)
        self.assertIn("POSTING SESSION: renewal required", result.stdout)

    def test_status_normalizes_crlf_from_windows_jq(self):
        result = self._run_invite_diagnostic(
            "status", "dv_live", crlf_jq=True
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("INVITED (davet verified)", result.stdout)
        self.assertIn("sb-mobile", result.stdout)
        self.assertNotIn("membership rejected", result.stderr)
        self.assertNotIn("\r", result.stdout + result.stderr)

    def test_status_separately_proves_a_live_posting_session(self):
        result = self._run_invite_diagnostic(
            "status", "dv_live", session="st_live"
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("INVITED (davet verified)", result.stdout)
        self.assertIn("POSTING SESSION: ready for sb-mobile", result.stdout)

    def test_status_reclassifies_a_server_seated_loca_as_pending_not_stale(self):
        # The server's whoami lists debug in sb-mobile (an active seat), but the
        # local invite cache is stale. This is the case that once misdirected a
        # called-in agent to "ask the master for a new invite": the seat is valid,
        # the fresh invite arrives over the Lobby socket, so the right guidance is
        # to START THE LISTENER. Membership is valid, so it is not an error.
        result = self._run_invite_diagnostic("status", "dv_stale")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotIn("INVITED (davet verified)", result.stdout)
        self.assertIn("the Master has CALLED", result.stdout)
        self.assertIn("start your listener", result.stdout)
        # It must NOT send a member the server has seated to ask for a new invite.
        self.assertNotIn("ask the master for a new", result.stdout)

    def test_status_says_a_seat_without_a_fitting_invite_needs_a_fresh_one(self):
        # The listener recorded that sb-mobile is seated but no kept invite
        # fits it. A server that keeps only invite hashes cannot re-send it,
        # so "start your listener" would be wrong advice here.
        result = self._run_invite_diagnostic(
            "status", "dv_stale", needs_fresh=["sb-mobile"]
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("NEEDS FRESH DAVET", result.stdout)
        self.assertIn("sb-mobile", result.stdout)
        self.assertIn(
            "needs a fresh invite — ask the Master to re-send", result.stdout
        )
        self.assertNotIn("the Master has CALLED", result.stdout)
        self.assertNotIn("dv_stale", result.stdout + result.stderr)

    def test_status_prefers_a_verified_invite_over_an_old_fresh_invite_note(self):
        result = self._run_invite_diagnostic(
            "status", "dv_live", needs_fresh=["sb-mobile"]
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("INVITED (davet verified)", result.stdout)
        self.assertNotIn("NEEDS FRESH DAVET", result.stdout)

    def test_status_says_suspended_when_the_listener_was_told_so(self):
        # The server answers a suspended membership with 401, like a revoked
        # one; the listener's note of `membership_suspended` tells them apart.
        result = self._run_invite_diagnostic(
            "status", "dv_stale", membership="mb_suspended", suspension_noted=True
        )
        self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
        self.assertIn("membership suspended", result.stderr)
        self.assertIn("resumes when the Master resumes it", result.stderr)
        self.assertNotIn("membership rejected", result.stderr)
        self.assertNotIn("mb_suspended", result.stdout + result.stderr)

    def test_status_without_a_suspension_note_still_says_rejected(self):
        result = self._run_invite_diagnostic(
            "status", "dv_stale", membership="mb_suspended"
        )
        self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
        self.assertIn("membership rejected", result.stderr)
        self.assertNotIn("suspended", result.stderr)

    def test_status_says_suspended_when_the_server_says_so(self):
        # The current server answers `/whoami` 403 `membership suspended`, so
        # no listener note is needed (a listener may never have run).
        result = self._run_invite_diagnostic(
            "status", "dv_stale", membership="mb_suspended_now"
        )
        self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
        self.assertIn("membership suspended — the server says", result.stderr)
        self.assertIn("resumes when the Master resumes it", result.stderr)
        self.assertNotIn("membership rejected", result.stderr)
        self.assertNotIn("mb_suspended_now", result.stdout + result.stderr)

    def test_status_reads_only_the_suspended_answer_of_a_403(self):
        # Guard: any other 403 is not a suspension.
        result = self._run_invite_diagnostic(
            "status", "dv_stale", membership="mb_forbidden"
        )
        self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
        self.assertIn("membership rejected", result.stderr)
        self.assertNotIn("suspended", result.stderr)

    def test_doctor_says_suspended_when_the_server_says_so(self):
        result = self._run_invite_diagnostic(
            "doctor", "dv_stale", membership="mb_suspended_now", hide_processes=True
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("SUSPENDED: 'debug' — the server says", result.stdout)
        self.assertIn("resumes when the Master resumes it", result.stdout)
        self.assertNotIn("session stale", result.stdout)
        self.assertNotIn("mb_suspended_now", result.stdout + result.stderr)

    def test_doctor_says_suspended_when_the_listener_was_told_so(self):
        result = self._run_invite_diagnostic(
            "doctor",
            "dv_stale",
            membership="mb_suspended",
            suspension_noted=True,
            hide_processes=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("SUSPENDED: 'debug'", result.stdout)
        self.assertIn("resumes when the Master resumes it", result.stdout)
        self.assertNotIn("session stale", result.stdout)
        self.assertNotIn("mb_suspended", result.stdout + result.stderr)

    def test_doctor_says_a_seat_without_a_fitting_invite_needs_a_fresh_one(self):
        result = self._run_invite_diagnostic(
            "doctor", "dv_stale", needs_fresh=["sb-mobile"]
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("NEEDS FRESH DAVET: 'debug'", result.stdout)
        self.assertIn(
            "needs a fresh invite — ask the Master to re-send", result.stdout
        )
        self.assertNotIn("STALE: 'debug'", result.stdout)
        self.assertNotIn("dv_stale", result.stdout + result.stderr)

    # The --fix runs below hide every process from doctor: --fix also signals
    # the older of two listeners with the same room and name anywhere on the
    # machine, and these tests are about the identity file. A real `doctor
    # --fix` here takes the listener of a test running in another checkout
    # on the same host (two test_lobby_seats runs: `returncode -15`).

    def test_doctor_fix_keeps_a_seated_locas_invite_and_drops_unlisted_ones(self):
        # sb-mobile is still seated (the server lists it) but its invite did not
        # verify; that can be a network fault, and the coming server cannot
        # re-send it. Only reviewer, which the server no longer lists, goes.
        result = self._run_invite_diagnostic(
            "doctor", "dv_stale", stale_extra=True, extra_args=("--fix",),
            hide_processes=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("DAVET_sb_mobile", result.saved_keys)
        self.assertNotIn("DAVET_reviewer", result.saved_keys)
        self.assertIn("invites for seated locas are kept", result.stdout)

    def test_doctor_fix_removes_nothing_on_an_incomplete_loca_list(self):
        # `locas` missing, null, an object, holding something that is not a
        # loca name, or cut off mid-answer is not "seated nowhere". --fix must
        # keep every invite and the session.
        member = {"kind": "member", "name": "debug", "member_kind": "agent"}
        cases = {
            "missing": json.dumps(member).encode(),
            "null": json.dumps({**member, "locas": None}).encode(),
            "object": json.dumps({**member, "locas": {"sb-mobile": True}}).encode(),
            "null entry": json.dumps({**member, "locas": ["sb-mobile", None]}).encode(),
            "number entry": json.dumps({**member, "locas": [7]}).encode(),
            "empty name": json.dumps({**member, "locas": [""]}).encode(),
            "object entry": json.dumps({**member, "locas": [{"room": "sb-mobile"}]}).encode(),
            "truncated": json.dumps({**member, "locas": ["sb-mobile"]}).encode()[:-9],
        }
        for label, raw in cases.items():
            with self.subTest(label):
                result = self._run_invite_diagnostic(
                    "doctor",
                    "dv_stale",
                    stale_extra=True,
                    session="st_kept",
                    extra_args=("--fix",),
                    whoami_raw=raw,
                    hide_processes=True,
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn("DAVET_sb_mobile", result.saved_keys)
                self.assertIn("DAVET_reviewer", result.saved_keys)
                self.assertIn("LOCA_SESSION", result.saved_keys)
                self.assertNotIn("fixed:", result.stdout)
                self.assertNotIn("dv_stale", result.stdout + result.stderr)

    def test_doctor_fix_clears_kept_invites_on_a_valid_empty_loca_list(self):
        # A complete answer that lists no loca is authoritative: nothing is
        # seated, so the kept invites and the session go.
        raw = json.dumps(
            {"kind": "member", "name": "debug", "member_kind": "agent", "locas": []}
        ).encode()
        result = self._run_invite_diagnostic(
            "doctor",
            "dv_stale",
            stale_extra=True,
            session="st_kept",
            extra_args=("--fix",),
            whoami_raw=raw,
            hide_processes=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("DAVET_sb_mobile", result.saved_keys)
        self.assertNotIn("DAVET_reviewer", result.saved_keys)
        self.assertNotIn("LOCA_SESSION", result.saved_keys)
        self.assertIn("fixed:", result.stdout)

    def test_doctor_reports_a_stale_local_invite(self):
        result = self._run_invite_diagnostic("doctor", "dv_stale")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("STALE: 'debug'", result.stdout)
        self.assertNotIn("INVITED (davet verified): 'debug'", result.stdout)

    def test_status_keeps_verified_room_usable_when_an_old_room_is_stale(self):
        result = self._run_invite_diagnostic(
            "status", "dv_live", stale_extra=True
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("INVITED (davet verified)", result.stdout)
        self.assertIn("sb-mobile", result.stdout)
        self.assertIn("STALE LOCAL CACHE (ignored)", result.stdout)
        self.assertIn("reviewer", result.stdout)

    def test_doctor_separates_verified_room_from_stale_local_cache(self):
        result = self._run_invite_diagnostic(
            "doctor", "dv_live", stale_extra=True
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("INVITED (davet verified): 'debug'", result.stdout)
        self.assertIn("STALE LOCAL CACHE (ignored): 'debug'", result.stdout)

    def test_doctor_flags_verified_identity_without_a_listener(self):
        result = self._run_invite_diagnostic(
            "doctor", "dv_live", hide_processes=True
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("MISSING LISTENER: 'debug'", result.stdout)

    @staticmethod
    def _fake_lobby_listener(name):
        """A process whose argv reads like the Lobby listener the runtime
        reference starts (empty room=); it only sleeps."""
        argv = (
            "python3 -u /tmp/listen.py "
            f"ws://127.0.0.1:9/ws?room=&name={name}&type=agent&filter=mentions -"
        )
        process = subprocess.Popen(["bash", "-c", 'exec -a "$1" sleep 30', "bash", argv])
        # Until bash has exec'd, the process still reads as a shell, which
        # doctor deliberately ignores.
        for _ in range(100):
            probe = subprocess.run(["ps", "-o", "args=", "-p", str(process.pid)],
                                   capture_output=True, text=True, check=False)
            if probe.stdout.startswith("python3 -u /tmp/listen.py"):
                break
            time.sleep(0.02)
        return process

    def test_doctor_counts_a_lobby_listener_for_a_seated_identity(self):
        # One Lobby listener serves every loca it is called into; its URL
        # names no loca. Doctor used to see no listener for the seat.
        listener = self._fake_lobby_listener("debug")
        try:
            result = self._run_invite_diagnostic("doctor", "dv_live")
        finally:
            listener.terminate()
            listener.wait(timeout=3)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("OK: 'debug' has a live listener", result.stdout)
        self.assertNotIn("MISSING LISTENER: 'debug'", result.stdout)

    # A name no other test or listener on the machine uses, so a listener
    # left running elsewhere cannot answer for it.
    LOBBY_ONLY = "lobby-only-probe"

    def _doctor_for_lobby_only_identity(self, name=None):
        name = name or self.LOBBY_ONLY

        class LobbyOnly(MembershipHandler):
            def do_GET(self):
                if self.path == "/whoami" and self.headers.get("x-room-token") == "mb_lobby_only":
                    self._json({"kind": "member", "name": name, "locas": []})
                else:
                    super().do_GET()

        server = ThreadingHTTPServer(("127.0.0.1", 0), LobbyOnly)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            with tempfile.TemporaryDirectory() as tmp:
                loca_dir = Path(tmp) / ".loca"
                loca_dir.mkdir()
                origin = f"http://127.0.0.1:{server.server_port}"
                (loca_dir / f"{name}.env").write_text(
                    f"ROOM_SERVER_URL={origin}\nLOCA_NAME={name}\nLOCA_MEMBERSHIP=mb_lobby_only\n",
                    encoding="utf-8",
                )
                env = dict(os.environ)
                env["HOME"] = tmp
                env.pop("LOCA_ENV", None)
                return subprocess.run(
                    [str(SKILL_DIR / "connect.sh"), "doctor", origin],
                    env=env,
                    text=True,
                    capture_output=True,
                    timeout=10,
                    check=False,
                )
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)

    def test_doctor_checks_the_listener_of_a_lobby_only_identity(self):
        # A fresh request-join identity sits in the Lobby with no loca; the
        # onboarding output tells it to wait for doctor's OK line.
        name = self.LOBBY_ONLY
        listener = self._fake_lobby_listener(name)
        try:
            result = self._doctor_for_lobby_only_identity()
        finally:
            listener.terminate()
            listener.wait(timeout=3)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(f"LOBBY: '{name}'", result.stdout)
        self.assertIn(f"OK: '{name}' has a live listener", result.stdout)

        result = self._doctor_for_lobby_only_identity()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(f"MISSING LISTENER: '{name}'", result.stdout)

    def test_doctor_sees_the_listener_of_a_name_that_contains_doctor(self):
        # Doctor once dropped every process line holding the word "doctor",
        # so an agent named like this was always reported without a listener.
        name = "doctor-bot-probe"
        listener = self._fake_lobby_listener(name)
        try:
            result = self._doctor_for_lobby_only_identity(name)
        finally:
            listener.terminate()
            listener.wait(timeout=3)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(f"OK: '{name}' has a live listener", result.stdout)
        self.assertNotIn(f"MISSING LISTENER: '{name}'", result.stdout)

    def test_doctor_does_not_list_its_own_process(self):
        # An argument naming listen.py makes doctor's own command line match
        # the listener pattern; doctor still leaves itself out.
        origin = "http://127.0.0.1:9"
        with tempfile.TemporaryDirectory() as tmp:
            env = {**os.environ, "HOME": tmp}
            env.pop("LOCA_ENV", None)
            result = subprocess.run(
                [str(SKILL_DIR / "connect.sh"), "doctor", origin, "listen.py"],
                env=env, text=True, capture_output=True, timeout=20, check=False,
            )
        self.assertEqual(result.returncode, 0, result.stderr)
        own = [line for line in result.stdout.splitlines()
               if line.startswith("  pid=") and "connect.sh doctor" in line]
        self.assertEqual(own, [])

    def _doctor_with_two_identities(self, pinned):
        """Two identities share one home. Run doctor, with LOCA_ENV pinned to
        the first one's file or not at all; return its result and every key
        the server was shown."""
        presented = []

        class TwoMembers(MembershipHandler):
            def do_GET(self):
                key = self.headers.get("x-room-token")
                if key:
                    presented.append(key)
                names = {"mb_first_identity": "first-probe", "mb_second_identity": "second-probe"}
                if self.path == "/whoami" and key in names:
                    self._json({"kind": "member", "name": names[key], "locas": []})
                else:
                    super().do_GET()

        server = ThreadingHTTPServer(("127.0.0.1", 0), TwoMembers)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            with tempfile.TemporaryDirectory() as tmp:
                loca_dir = Path(tmp) / ".loca"
                loca_dir.mkdir()
                origin = f"http://127.0.0.1:{server.server_port}"
                for name, key in (
                    ("first-probe", "mb_first_identity"),
                    ("second-probe", "mb_second_identity"),
                ):
                    (loca_dir / f"{name}.env").write_text(
                        f"ROOM_SERVER_URL={origin}\nLOCA_NAME={name}\nLOCA_MEMBERSHIP={key}\n",
                        encoding="utf-8",
                    )
                env = dict(os.environ)
                env["HOME"] = tmp
                env.pop("LOCA_ENV", None)
                if pinned:
                    env["LOCA_ENV"] = str(loca_dir / "first-probe.env")
                result = subprocess.run(
                    [str(SKILL_DIR / "connect.sh"), "doctor", origin],
                    env=env,
                    text=True,
                    capture_output=True,
                    timeout=10,
                    check=False,
                )
                return result, presented
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)

    def test_doctor_with_a_pinned_identity_checks_only_that_identity(self):
        # The skill tells an agent to pin LOCA_ENV and to run doctor. With
        # two identities in one home, A's check must not open B's file and
        # must not ask the server anything as B.
        result, presented = self._doctor_with_two_identities(pinned=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("'first-probe'", result.stdout)
        self.assertIn("mb_first_identity", presented)
        self.assertNotIn("mb_second_identity", presented)
        self.assertNotIn("second-probe", result.stdout + result.stderr)

    def test_doctor_without_a_pinned_identity_checks_every_identity_of_the_machine(self):
        # An operator looking at the machine, with nothing pinned, still sees
        # every identity kept on it.
        result, presented = self._doctor_with_two_identities(pinned=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("'first-probe'", result.stdout)
        self.assertIn("'second-probe'", result.stdout)
        self.assertEqual(
            sorted(set(presented)), ["mb_first_identity", "mb_second_identity"]
        )

    def test_reconnect_refuses_to_replace_an_unmanaged_native_monitor(self):
        result = self._run_invite_diagnostic("reconnect", "dv_live")
        self.assertEqual(result.returncode, 5, result.stdout + result.stderr)
        self.assertIn("UNMANAGED RUNTIME", result.stderr)
        self.assertIn("do not launch a bare listen.py", result.stderr)

    def test_doctor_does_not_count_monitor_supervisor_as_a_listener(self):
        server = ThreadingHTTPServer(("127.0.0.1", 0), MembershipHandler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        fake_argv = (
            "python3 -u /tmp/monitor_listener.py --name fake-worker -- "
            "python3 -u /tmp/listen.py "
            "wss://loca.example/ws?room=fake-room&name=fake-worker"
        )
        supervisor = subprocess.Popen(
            ["bash", "-c", 'exec -a "$1" sleep 30', "bash", fake_argv]
        )
        try:
            with tempfile.TemporaryDirectory() as tmp:
                env = dict(os.environ)
                env["HOME"] = tmp
                env.pop("LOCA_ENV", None)
                origin = f"http://127.0.0.1:{server.server_port}"
                result = subprocess.run(
                    [str(SKILL_DIR / "connect.sh"), "doctor", origin],
                    env=env,
                    text=True,
                    capture_output=True,
                    timeout=10,
                    check=False,
                )

                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertNotIn(f"pid={supervisor.pid}", result.stdout)
                self.assertNotIn(
                    "fake-room&name=fake-worker -> 2 processes",
                    result.stdout,
                )
        finally:
            supervisor.terminate()
            supervisor.wait(timeout=3)
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)

    def test_doctor_reports_one_invalid_env_and_continues_with_other_identities(self):
        server = ThreadingHTTPServer(("127.0.0.1", 0), MembershipHandler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            with tempfile.TemporaryDirectory() as tmp:
                loca_dir = Path(tmp) / ".loca"
                loca_dir.mkdir()
                origin = f"http://127.0.0.1:{server.server_port}"
                (loca_dir / "env").write_text(
                    f"ROOM_SERVER_URL={origin}\nLOCA_NAME=legacy\nM_TOKEN=bad\n",
                    encoding="utf-8",
                )
                (loca_dir / "worker.env").write_text(
                    f"ROOM_SERVER_URL={origin}\n"
                    "LOCA_NAME=worker\n"
                    "LOCA_MEMBERSHIP=mb_worker\n",
                    encoding="utf-8",
                )
                env = dict(os.environ)
                env["HOME"] = tmp
                env.pop("LOCA_ENV", None)
                result = subprocess.run(
                    [str(SKILL_DIR / "connect.sh"), "doctor", origin],
                    env=env,
                    text=True,
                    capture_output=True,
                    check=False,
                )

                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn("INVALID: invalid credential key 'M_TOKEN'", result.stdout)
                self.assertIn("LOBBY: 'worker'", result.stdout)
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)

    def test_membership_is_not_stored_as_a_legacy_room_token(self):
        MembershipHandler.session_requests = 0
        server = ThreadingHTTPServer(("127.0.0.1", 0), MembershipHandler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            with tempfile.TemporaryDirectory() as tmp:
                env_file = Path(tmp) / ".loca" / "worker.env"
                env = dict(os.environ)
                env.update({"HOME": tmp, "LOCA_ENV": str(env_file)})
                result = subprocess.run(
                    [
                        str(SKILL_DIR / "connect.sh"),
                        "setup",
                        f"http://127.0.0.1:{server.server_port}",
                        "worker",
                    ],
                    env=env,
                    input="mb_worker\n",
                    text=True,
                    capture_output=True,
                    check=False,
                )

                self.assertEqual(result.returncode, 0, result.stderr)
                saved = env_file.read_text(encoding="utf-8")
                self.assertIn("LOCA_MEMBERSHIP=mb_worker\n", saved)
                self.assertNotIn("ROOM_TOKEN=", saved)
                self.assertNotIn("DAVET_", saved)
                self.assertEqual(env_file.stat().st_mode & 0o777, 0o600)
                self.assertEqual(MembershipHandler.session_requests, 0)
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)

    def test_setup_rejects_membership_issued_to_another_identity(self):
        server = ThreadingHTTPServer(("127.0.0.1", 0), MembershipHandler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            with tempfile.TemporaryDirectory() as tmp:
                env_file = Path(tmp) / ".loca" / "mihenk.env"
                env = dict(os.environ)
                env.update({"HOME": tmp, "LOCA_ENV": str(env_file)})
                result = subprocess.run(
                    [
                        str(SKILL_DIR / "connect.sh"),
                        "setup",
                        f"http://127.0.0.1:{server.server_port}",
                        "mihenk",
                    ],
                    env=env,
                    input="mb_worker\n",
                    text=True,
                    capture_output=True,
                    check=False,
                )

                self.assertEqual(result.returncode, 1)
                self.assertIn(
                    "credential belongs to 'worker', not requested identity 'mihenk'",
                    result.stderr,
                )
                self.assertIn("exact name 'mihenk'", result.stderr)
                self.assertFalse(env_file.exists())
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)

    def test_setup_rejects_invite_issued_to_another_identity(self):
        server = ThreadingHTTPServer(("127.0.0.1", 0), MembershipHandler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            with tempfile.TemporaryDirectory() as tmp:
                env_file = Path(tmp) / ".loca" / "mihenk.env"
                env = dict(os.environ)
                env.update({"HOME": tmp, "LOCA_ENV": str(env_file)})
                result = subprocess.run(
                    [
                        str(SKILL_DIR / "connect.sh"),
                        "setup",
                        f"http://127.0.0.1:{server.server_port}",
                        "mihenk",
                    ],
                    env=env,
                    input="dv_invited\n",
                    text=True,
                    capture_output=True,
                    check=False,
                )

                self.assertEqual(result.returncode, 1)
                # The invite's member is named by the claim helper, which
                # prints no data: the refusal names the requested identity.
                self.assertIn(
                    "credential belongs to another identity, not requested identity 'mihenk'",
                    result.stderr,
                )
                self.assertFalse(env_file.exists())
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)


if __name__ == "__main__":
    unittest.main()
