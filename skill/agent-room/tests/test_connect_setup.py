import json
import os
import shutil
import subprocess
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


SKILL_DIR = Path(__file__).resolve().parents[1]


class MembershipHandler(BaseHTTPRequestHandler):
    session_requests = 0
    last_post_path = ""

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
        elif self.path == "/whoami" and token == "mb_invited":
            self._json(
                {
                    "kind": "member",
                    "name": "debug",
                    "member_kind": "agent",
                    "locas": ["sb-mobile"],
                }
            )
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
            self.assertIn("membership (Lobby) or davet (one loca)", result.stderr)
            self.assertIn("do not reuse another agent's env", result.stderr)

    def _run_invite_diagnostic(
        self,
        command,
        davet,
        *,
        stale_extra=False,
        session="",
        hide_processes=False,
        crlf_jq=False,
    ):
        server = ThreadingHTTPServer(("127.0.0.1", 0), MembershipHandler)
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
                    "LOCA_MEMBERSHIP=mb_invited\n"
                    f"DAVET_sb_mobile={davet}\n"
                    + (f"LOCA_SESSION={session}\n" if session else "")
                    + ("DAVET_reviewer=dv_stale\n" if stale_extra else ""),
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
                return subprocess.run(
                    args,
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

    def test_status_verifies_the_exact_local_davet(self):
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
        # local davet cache is stale. This is the case that once misdirected a
        # called-in agent to "ask the master for a new davet": the seat is valid,
        # the fresh davet arrives over the Lobby socket, so the right guidance is
        # to START THE LISTENER. Membership is valid, so it is not an error.
        result = self._run_invite_diagnostic("status", "dv_stale")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotIn("INVITED (davet verified)", result.stdout)
        self.assertIn("the Master has CALLED", result.stdout)
        self.assertIn("start your listener", result.stdout)
        # It must NOT send a member the server has seated to ask for a new davet.
        self.assertNotIn("ask the master for a new", result.stdout)

    def test_doctor_reports_a_stale_local_davet(self):
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

    def test_setup_rejects_davet_issued_to_another_identity(self):
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
                self.assertIn(
                    "credential belongs to 'debug', not requested identity 'mihenk'",
                    result.stderr,
                )
                self.assertFalse(env_file.exists())
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)


if __name__ == "__main__":
    unittest.main()
