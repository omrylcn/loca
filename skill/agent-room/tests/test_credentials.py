import importlib.util
import io
import json
import os
import socket
import subprocess
import sys
import tempfile
import unittest
from contextlib import contextmanager
from pathlib import Path
from unittest import mock
from urllib.parse import parse_qs, urlparse


SKILL_DIR = Path(__file__).resolve().parents[1]
REPO_DIR = SKILL_DIR.parents[1]
sys.path.insert(0, str(SKILL_DIR))

import bot  # noqa: E402
import credentials  # noqa: E402
import listen  # noqa: E402


def load_agentd():
    path = REPO_DIR / "adapters" / "generic-command" / "agentd.py"
    if not path.is_file():
        return None
    spec = importlib.util.spec_from_file_location("loca_agentd_test", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


AGENTD = load_agentd()


@contextmanager
def isolated_environment(home):
    old = dict(os.environ)
    os.environ.clear()
    os.environ.update({"HOME": str(home), "PATH": old.get("PATH", "")})
    try:
        yield
    finally:
        os.environ.clear()
        os.environ.update(old)


def write_env(home, filename, **values):
    loca = Path(home) / ".loca"
    loca.mkdir(parents=True, exist_ok=True)
    path = loca / filename
    path.write_text(
        "".join(f"{key}={value}\n" for key, value in values.items()),
        encoding="utf-8",
    )
    return path


class CredentialSelectionTests(unittest.TestCase):
    def test_atomic_updates_do_not_lose_concurrent_room_credentials(self):
        with tempfile.TemporaryDirectory() as tmp:
            identity = write_env(
                tmp,
                "worker.env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="worker",
            )
            processes = []
            for index in range(16):
                payload = f"DAVET_room_{index}\0dv_{index}\0".encode()
                processes.append(
                    subprocess.Popen(
                        [
                            sys.executable,
                            str(SKILL_DIR / "credentials.py"),
                            "update",
                            str(identity),
                        ],
                        stdin=subprocess.PIPE,
                        stdout=subprocess.DEVNULL,
                        stderr=subprocess.PIPE,
                    )
                )
                processes[-1].stdin.write(payload)
                processes[-1].stdin.close()

            for process in processes:
                process.wait(timeout=5)
                error = process.stderr.read().decode()
                process.stderr.close()
                self.assertEqual(
                    process.returncode,
                    0,
                    error,
                )

            saved = identity.read_text(encoding="utf-8")
            self.assertIn("ROOM_SERVER_URL=https://loca.example", saved)
            self.assertIn("LOCA_NAME=worker", saved)
            for index in range(16):
                self.assertIn(f"DAVET_room_{index}=dv_{index}\n", saved)

    def test_listener_without_required_arguments_exits_with_usage(self):
        result = subprocess.run(
            [sys.executable, str(SKILL_DIR / "listen.py")],
            text=True,
            capture_output=True,
            timeout=5,
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("usage: listen.py", result.stderr)
        self.assertNotIn("Traceback", result.stderr)

    def test_listener_reports_bad_credentials_without_a_traceback(self):
        with tempfile.TemporaryDirectory() as tmp:
            identity = write_env(
                tmp,
                "worker.env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="another-agent",
            )
            result = subprocess.run(
                [
                    sys.executable,
                    str(SKILL_DIR / "listen.py"),
                    "wss://loca.example/ws?room=project&name=worker",
                    "-",
                ],
                env={
                    "HOME": tmp,
                    "LOCA_ENV": str(identity),
                    "PATH": os.environ.get("PATH", ""),
                },
                text=True,
                capture_output=True,
                timeout=5,
            )
            self.assertEqual(result.returncode, 2)
            self.assertIn("credential configuration error", result.stderr)
            self.assertNotIn("Traceback", result.stderr)

    def test_revoked_invite_is_distinct_from_a_transient_session_failure(self):
        with tempfile.TemporaryDirectory() as tmp, isolated_environment(tmp):
            write_env(
                tmp,
                "worker.env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="worker",
                DAVET_project="dv_revoked",
            )
            credentials.load_local_env("worker")
            error = listen.HTTPError(
                "https://loca.example/sessions", 401, "Unauthorized", {}, None
            )
            with mock.patch.object(
                listen.urllib.request, "urlopen", side_effect=error
            ):
                with self.assertRaises(listen.RoomAccessRevoked):
                    listen.renew_session(
                        "wss://loca.example/ws?room=project&name=worker"
                    )

    def test_each_loca_presents_the_session_minted_for_it(self):
        # One listener process, two locas. A session minted from a loca's
        # invite opens that loca only, and the env file has one LOCA_SESSION
        # slot. When both renew, each must still present its own session:
        # presenting the other's is refused, which makes it renew again and
        # overwrite the slot for the other, for as long as the two stay in
        # step (seen as a 60 s livelock in test_session_expiry_e2e).
        with tempfile.TemporaryDirectory() as tmp, isolated_environment(tmp):
            write_env(
                tmp,
                "worker.env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="worker",
                DAVET_alpha="dv_alpha",
                DAVET_beta="dv_beta",
                LOCA_SESSION="sess_from_file",
            )
            credentials.load_local_env("worker")
            self.addCleanup(listen.ROOM_SESSIONS.clear)
            listen.ROOM_SESSIONS.clear()

            def mint(request, timeout=None):
                loca = json.loads(request.data.decode())["loca"]
                self.assertEqual(request.get_header("X-room-token"), f"dv_{loca}")
                return io.BytesIO(json.dumps({"session_token": f"sess_{loca}"}).encode())

            # Before either has renewed, both present what the file held.
            self.assertIn("loca.session.sess_from_file", listen.room_protocols("alpha"))
            with mock.patch.object(listen.urllib.request, "urlopen", side_effect=mint), \
                    mock.patch.object(listen.sys, "stderr", io.StringIO()):
                listen.renew_session("wss://loca.example/ws?room=alpha&name=worker")
                listen.renew_session("wss://loca.example/ws?room=beta&name=worker")
            self.assertIn("loca.session.sess_alpha", listen.room_protocols("alpha"))
            self.assertIn("loca.session.sess_beta", listen.room_protocols("beta"))
            self.assertNotIn("loca.session.sess_beta", listen.room_protocols("alpha"))

            # A fresh invite for a loca discards that loca's session only.
            with mock.patch.object(listen.sys, "stderr", io.StringIO()):
                self.assertTrue(listen.store_room_invite("alpha", "dv_alpha_new"))
            self.assertFalse(
                any(p.startswith("loca.session.") for p in listen.room_protocols("alpha"))
            )
            self.assertIn("loca.session.sess_beta", listen.room_protocols("beta"))
            # A seat the server no longer lists takes its session with it.
            with mock.patch.object(listen.sys, "stderr", io.StringIO()):
                listen.drop_unlisted_room_invites(["alpha"])
            self.assertFalse(
                any(p.startswith("loca.session.") for p in listen.room_protocols("beta"))
            )

    def test_a_renewal_answered_after_the_invite_changed_is_discarded(self):
        # The invite is replaced, or the seat removed, while POST /sessions is
        # out. The answer is a session of the seat that is gone: it must not
        # be kept for the loca nor written to the shared slot or the env file.
        for change in ("replaced", "removed"):
            with self.subTest(change=change), tempfile.TemporaryDirectory() as tmp, \
                    isolated_environment(tmp):
                identity = write_env(
                    tmp,
                    "worker.env",
                    ROOM_SERVER_URL="https://loca.example",
                    LOCA_NAME="worker",
                    DAVET_alpha="dv_alpha",
                    DAVET_beta="dv_beta",
                )
                credentials.load_local_env("worker")
                self.addCleanup(listen.ROOM_SESSIONS.clear)
                listen.ROOM_SESSIONS.clear()
                stderr = io.StringIO()

                def mint(request, timeout=None):
                    loca = json.loads(request.data.decode())["loca"]
                    if loca == "alpha":
                        if change == "replaced":
                            self.assertTrue(listen.store_room_invite("alpha", "dv_alpha_new"))
                        else:
                            listen.clear_room_invite("alpha", "dv_alpha")
                    return io.BytesIO(
                        json.dumps({"session_token": f"sess_{loca}_old_seat"}).encode()
                    )

                with mock.patch.object(listen.urllib.request, "urlopen", side_effect=mint), \
                        mock.patch.object(listen.sys, "stderr", stderr):
                    renewed = listen.renew_session(
                        "wss://loca.example/ws?room=alpha&name=worker"
                    )
                self.assertIsNone(renewed)
                self.assertIn("discarded", stderr.getvalue())
                self.assertNotIn(listen.room_env_key("alpha"), listen.ROOM_SESSIONS)
                self.assertNotIn("LOCA_SESSION", os.environ)
                self.assertNotIn("sess_alpha_old_seat", identity.read_text(encoding="utf-8"))
                self.assertFalse(
                    any(p.startswith("loca.session.") for p in listen.room_protocols("alpha"))
                )
                # The other loca is untouched and still renews normally.
                with mock.patch.object(listen.urllib.request, "urlopen", side_effect=mint), \
                        mock.patch.object(listen.sys, "stderr", io.StringIO()):
                    self.assertIsNotNone(
                        listen.renew_session("wss://loca.example/ws?room=beta&name=worker")
                    )
                self.assertIn("loca.session.sess_beta_old_seat", listen.room_protocols("beta"))

    def test_shell_metacharacters_in_env_are_literal_and_never_execute(self):
        with tempfile.TemporaryDirectory() as tmp:
            marker = Path(tmp) / "executed"
            identity = write_env(
                tmp,
                "worker.env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="worker",
                LOCA_MEMBERSHIP=f"$(touch {marker})",
            )
            fake_bin = Path(tmp) / "bin"
            fake_bin.mkdir()
            curl = fake_bin / "curl"
            curl.write_text(
                """#!/bin/sh
printf '{"kind":"member","name":"worker","locas":[]}\n200'
""",
                encoding="utf-8",
            )
            curl.chmod(0o755)
            result = subprocess.run(
                [
                    str(SKILL_DIR / "connect.sh"),
                    "status",
                    "https://loca.example",
                    "worker",
                ],
                env={
                    "HOME": tmp,
                    "LOCA_ENV": str(identity),
                    "PATH": f"{fake_bin}:{os.environ.get('PATH', '')}",
                },
                text=True,
                capture_output=True,
                timeout=5,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertFalse(marker.exists(), "credential data executed as shell")

    def test_python_loader_rejects_non_credential_environment_keys(self):
        with tempfile.TemporaryDirectory() as tmp, isolated_environment(tmp):
            write_env(
                tmp,
                "worker.env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="worker",
                PATH="/attacker-controlled",
            )
            with self.assertRaises(credentials.CredentialError):
                credentials.load_local_env("worker")

    def test_named_identity_replaces_default_and_stale_shell_values(self):
        with tempfile.TemporaryDirectory() as tmp, isolated_environment(tmp):
            write_env(
                tmp,
                "env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="loca-dev",
                DAVET_general="wrong",
            )
            named = write_env(
                tmp,
                "sb-feature.env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="sb-feature",
                LOCA_SESSION="session-feature",
                DAVET_sb_mobile="invite-feature",
            )
            os.environ["LOCA_NAME"] = "inherited-other-agent"
            os.environ["DAVET_general"] = "inherited-wrong"
            os.environ["LOCA_MEMBERSHIP"] = "mb_wrong"

            selected = credentials.load_local_env("sb-feature")

            self.assertEqual(selected, str(named))
            self.assertEqual(os.environ["LOCA_NAME"], "sb-feature")
            self.assertNotIn("DAVET_general", os.environ)
            self.assertNotIn("LOCA_MEMBERSHIP", os.environ)
            self.assertEqual(
                credentials.token_for_room("sb-mobile"), "invite-feature"
            )

    def test_default_identity_mismatch_fails_instead_of_impersonating(self):
        with tempfile.TemporaryDirectory() as tmp, isolated_environment(tmp):
            write_env(
                tmp,
                "env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="loca-dev",
                DAVET_general="wrong",
            )
            with self.assertRaises(credentials.CredentialError):
                credentials.load_local_env("sb-feature")

    def test_server_origin_is_enforced_for_http_and_websocket(self):
        with tempfile.TemporaryDirectory() as tmp, isolated_environment(tmp):
            write_env(
                tmp,
                "sb-feature.env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="sb-feature",
                DAVET_sb_mobile="invite-feature",
            )
            credentials.load_local_env("sb-feature")
            credentials.validate_server_scope("wss://loca.example/ws")
            with self.assertRaises(credentials.CredentialError):
                credentials.validate_server_scope("http://127.0.0.1:8787")

    def test_listener_keeps_bearers_out_of_url_and_uses_protocol_header(self):
        with tempfile.TemporaryDirectory() as tmp, isolated_environment(tmp):
            write_env(
                tmp,
                "sb-feature.env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="sb-feature",
                LOCA_SESSION="session-feature",
                DAVET_sb_mobile="invite-feature",
            )
            clean = (
                "wss://loca.example/ws?"
                "room=sb-mobile&name=sb-feature&type=agent&filter=msg"
            )
            credentials.load_local_env("sb-feature")
            built = listen.authenticated_room_url(clean, "sb-mobile")
            query = parse_qs(urlparse(built).query)
            self.assertNotIn("token", query)
            self.assertNotIn("session", query)
            self.assertEqual(
                listen.room_protocols("sb-mobile"),
                [
                    "loca.v1",
                    "loca.room.invite-feature",
                    "loca.session.session-feature",
                ],
            )

    def test_listener_requests_three_message_four_second_turn_queue(self):
        with tempfile.TemporaryDirectory() as tmp, isolated_environment(tmp):
            write_env(
                tmp,
                "worker.env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="worker",
                DAVET_project_x="dv_worker",
            )
            credentials.load_local_env("worker")
            built = listen.authenticated_room_url(
                "wss://loca.example/ws?"
                "room=project-x&name=worker&type=agent&filter=mentions",
                "project-x",
            )
            query = parse_qs(urlparse(built).query)
            self.assertNotIn("turn_max", query)
            self.assertNotIn("turn_wait_ms", query)

    def test_listener_rest_helpers_use_local_invite_after_url_auth_removal(self):
        with tempfile.TemporaryDirectory() as tmp, isolated_environment(tmp):
            write_env(
                tmp,
                "lead-engineer.env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="lead-engineer",
                DAVET_sb_dev="dv_sb_dev",
            )
            credentials.load_local_env("lead-engineer")

            class Response(io.BytesIO):
                status = 200

                def __enter__(self):
                    return self

                def __exit__(self, *_):
                    return False

            settings = Response(json.dumps({"lead": "lead-engineer"}).encode())
            history = Response(b"[]")
            ack = Response(b"")
            ack.status = 204
            url = (
                "wss://loca.example/ws?"
                "room=sb-dev&name=lead-engineer&type=agent&filter=mentions"
            )
            with mock.patch.object(
                listen.urllib.request,
                "urlopen",
                side_effect=[settings, history, ack],
            ) as opened:
                self.assertEqual(listen.fetch_room_lead(url), "lead-engineer")
                self.assertEqual(
                    list(
                        listen.backfill_pages(
                            url, 42, "lead-engineer", is_lead=True
                        )
                    ),
                    [],
                )
                self.assertTrue(listen.acknowledge_care(url, 7, "lead-engineer"))

            self.assertEqual(opened.call_count, 3)
            for call in opened.call_args_list:
                request = call.args[0]
                self.assertEqual(request.get_header("X-room-token"), "dv_sb_dev")
                self.assertIsNone(request.get_header("X-session-token"))

    def test_listener_preserves_server_turn_as_one_durable_delivery(self):
        event = {
            "t": "turn",
            "room": "reviewer",
            "messages": [
                {"id": 101, "room": "reviewer", "text": "first"},
                {"id": 103, "room": "reviewer", "text": "third"},
            ],
        }
        delivery = listen.make_delivery("reviewer", event)
        self.assertEqual(delivery["delivery_id"], "reviewer:103")
        self.assertEqual(delivery["last_id"], 103)
        self.assertIs(delivery["event"], event)
        self.assertEqual(len(delivery["event"]["messages"]), 2)

    @staticmethod
    def _fake_server(answers, calls):
        """A stand-in for the server's claim, ACK and /whoami, by URL path.
        `answers` maps an invite to its claim answer; every call is recorded."""
        keys = {}

        def http(method, url, headers=None, body=None):
            headers = headers or {}
            path = url.split("443", 1)[1]
            calls.append((method, url, dict(headers)))
            token = headers.get("x-room-token")
            if path == "/membership/claim":
                code, answer = answers[token]
                if code in (200, 201):
                    keys[answer["membership_token"]] = answer["name"]
                return code, json.dumps(answer)
            if path == "/membership/claim/ack":
                return 200, json.dumps({"acknowledged": True})
            if path == "/whoami" and token in keys:
                return 200, json.dumps({"kind": "member", "name": keys[token]})
            return 401, "{}"

        return http

    def test_listener_claims_and_persists_the_lobby_membership(self):
        with tempfile.TemporaryDirectory() as tmp, isolated_environment(tmp):
            identity = write_env(
                tmp,
                "worker.env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="worker",
                DAVET_project_x="invite-worker",
            )
            credentials.load_local_env("worker")
            calls = []
            answers = {
                "invite-worker": (
                    201,
                    {"membership_token": "mb_worker", "name": "worker",
                     "kind": "agent", "ack_required": True},
                ),
            }
            import join_request

            with mock.patch.object(
                join_request, "http", self._fake_server(answers, calls)
            ):
                claimed = listen.claim_membership(
                    "wss://loca.example/ws?room=project-x&name=worker"
                )

            self.assertEqual(claimed, "mb_worker")
            self.assertEqual(os.environ["LOCA_MEMBERSHIP"], "mb_worker")
            self.assertIn(
                "LOCA_MEMBERSHIP=mb_worker",
                identity.read_text(encoding="utf-8"),
            )
            claim = [c for c in calls if c[1].endswith("/membership/claim")]
            ack = [c for c in calls if c[1].endswith("/membership/claim/ack")]
            self.assertEqual(
                claim[0][1], "https://loca.example:443/membership/claim"
            )
            self.assertEqual(claim[0][2]["x-room-token"], "invite-worker")
            claim_id = claim[0][2]["x-claim-id"]
            self.assertRegex(claim_id, r"^cl_[A-Za-z0-9_-]{43}$")
            self.assertEqual([c[2]["x-claim-id"] for c in ack], [claim_id])
            self.assertFalse((Path(tmp) / ".loca" / "worker.claim").exists())

    def test_listener_skips_a_stale_invite_when_claiming_membership(self):
        with tempfile.TemporaryDirectory() as tmp, isolated_environment(tmp):
            identity = write_env(
                tmp,
                "sb-feature.env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="sb-feature",
                DAVET_sb_dev="dv_stale",
                DAVET_sb_mobile="dv_live",
            )
            credentials.load_local_env("sb-feature")
            calls = []
            answers = {
                "dv_stale": (401, {}),
                "dv_live": (
                    201,
                    {"membership_token": "mb_feature", "name": "sb-feature",
                     "kind": "agent", "ack_required": True},
                ),
            }
            import join_request

            with mock.patch.object(
                join_request, "http", self._fake_server(answers, calls)
            ):
                claimed = listen.claim_membership(
                    "wss://loca.example/ws?room=sb-mobile&name=sb-feature"
                )

            self.assertEqual(claimed, "mb_feature")
            attempted = [
                c[2]["x-room-token"]
                for c in calls
                if c[1].endswith("/membership/claim")
            ]
            self.assertEqual(attempted, ["dv_stale", "dv_live"])
            self.assertIn(
                "LOCA_MEMBERSHIP=mb_feature",
                identity.read_text(encoding="utf-8"),
            )

    def test_a_listener_restarted_after_a_lost_answer_asks_for_the_same_key(self):
        with tempfile.TemporaryDirectory() as tmp, isolated_environment(tmp):
            write_env(
                tmp,
                "worker.env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="worker",
                DAVET_project_x="invite-worker",
            )
            credentials.load_local_env("worker")
            calls = []
            answers = {"invite-worker": (502, {})}
            import join_request

            with mock.patch.object(
                join_request, "http", self._fake_server(answers, calls)
            ):
                self.assertIsNone(
                    listen.claim_membership(
                        "wss://loca.example/ws?room=project-x&name=worker"
                    )
                )
                answers["invite-worker"] = (
                    200,
                    {"membership_token": "mb_worker", "name": "worker",
                     "kind": "agent", "ack_required": True},
                )
                self.assertEqual(
                    listen.claim_membership(
                        "wss://loca.example/ws?room=project-x&name=worker"
                    ),
                    "mb_worker",
                )
            claim_ids = [
                c[2]["x-claim-id"] for c in calls if c[1].endswith("/membership/claim")
            ]
            self.assertEqual(len(claim_ids), 2)
            self.assertEqual(len(set(claim_ids)), 1, "the restart resumed the set-up")

    def test_a_stale_release_cannot_erase_a_newer_lobby_call(self):
        with tempfile.TemporaryDirectory() as tmp, isolated_environment(tmp):
            identity = write_env(
                tmp,
                "worker.env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="worker",
                LOCA_MEMBERSHIP="mb_worker",
                DAVET_project_x="dv_old",
            )
            credentials.load_local_env("worker")
            self.assertTrue(listen.store_room_invite("project-x", "dv_new"))

            listen.clear_room_invite("project-x", "dv_old")

            self.assertEqual(
                os.environ["DAVET_project_x"],
                "dv_new",
                "the old room thread must not erase a raced-in call",
            )
            self.assertIn(
                "DAVET_project_x=dv_new",
                identity.read_text(encoding="utf-8"),
            )

    def test_a_release_landing_inside_a_lobby_call_cannot_erase_the_fresh_invite(self):
        """The room thread forgets the ended invite while the Lobby thread is
        between writing the fresh one to the file and taking it into
        ``os.environ``.  The file must still hold the fresh invite: the running
        process would otherwise go on working and lose its seat on restart."""
        import threading

        with tempfile.TemporaryDirectory() as tmp, isolated_environment(tmp):
            identity = write_env(
                tmp,
                "worker.env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="worker",
                LOCA_MEMBERSHIP="mb_worker",
                DAVET_project_x="dv_old",
            )
            credentials.load_local_env("worker")

            real_update = listen.update_env_values
            release = []

            def update_then_let_the_release_in(path, updates):
                real_update(path, updates)
                if updates.get("DAVET_project_x") == "dv_new" and not release:
                    # The fresh invite is in the file and not yet in os.environ.
                    thread = threading.Thread(
                        target=listen.clear_room_invite,
                        args=("project-x", "dv_old"),
                    )
                    release.append(thread)
                    thread.start()
                    # Unguarded code finishes its clear well inside this wait;
                    # guarded code is still waiting for the Lobby thread.
                    thread.join(2)

            with mock.patch.object(
                listen, "update_env_values", update_then_let_the_release_in
            ):
                self.assertTrue(listen.store_room_invite("project-x", "dv_new"))
                self.assertEqual(len(release), 1, "the release never ran")
                release[0].join(10)
                self.assertFalse(release[0].is_alive(), "the release never finished")

            self.assertEqual(os.environ.get("DAVET_project_x"), "dv_new")
            self.assertIn(
                "DAVET_project_x=dv_new",
                identity.read_text(encoding="utf-8"),
                "the release erased the fresh invite from the file",
            )

    def test_lobby_call_replaces_invite_and_discards_old_scoped_session(self):
        with tempfile.TemporaryDirectory() as tmp, isolated_environment(tmp):
            identity = write_env(
                tmp,
                "worker.env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="worker",
                LOCA_MEMBERSHIP="mb_worker",
                LOCA_SESSION="stale-reviewer-session",
                DAVET_project_x="dv_old",
            )
            credentials.load_local_env("worker")

            self.assertTrue(listen.store_room_invite("project-x", "dv_new"))

            self.assertEqual(os.environ["DAVET_project_x"], "dv_new")
            self.assertNotIn("LOCA_SESSION", os.environ)
            saved = identity.read_text(encoding="utf-8")
            self.assertIn("DAVET_project_x=dv_new", saved)
            self.assertNotIn("LOCA_SESSION=", saved)

    def test_lobby_snapshot_removes_only_server_absent_local_invites(self):
        with tempfile.TemporaryDirectory() as tmp, isolated_environment(tmp):
            identity = write_env(
                tmp,
                "lead-engineer.env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="lead-engineer",
                LOCA_MEMBERSHIP="mb_lead",
                LOCA_SESSION="stale-reviewer-session",
                DAVET_sb_dev="dv_live",
                DAVET_reviewer="dv_revoked",
            )
            credentials.load_local_env("lead-engineer")

            class Response(io.BytesIO):
                def __enter__(self):
                    return self

                def __exit__(self, *_):
                    return False

            response = Response(
                json.dumps(
                    {
                        "kind": "member",
                        "name": "lead-engineer",
                        "locas": ["sb-dev"],
                    }
                ).encode()
            )
            with mock.patch.object(
                listen.urllib.request, "urlopen", return_value=response
            ) as opened:
                removed = listen.reconcile_room_invites(
                    "wss://loca.example/lobby/ws", "mb_lead"
                )

            self.assertEqual(removed, ["reviewer"])
            self.assertEqual(os.environ["DAVET_sb_dev"], "dv_live")
            self.assertNotIn("DAVET_reviewer", os.environ)
            self.assertNotIn("LOCA_SESSION", os.environ)
            saved = identity.read_text(encoding="utf-8")
            self.assertIn("DAVET_sb_dev=dv_live", saved)
            self.assertNotIn("DAVET_reviewer=", saved)
            self.assertNotIn("LOCA_SESSION=", saved)
            request = opened.call_args.args[0]
            self.assertEqual(request.full_url, "https://loca.example:443/whoami")
            self.assertEqual(request.get_header("X-room-token"), "mb_lead")

    def test_lobby_snapshot_failure_never_deletes_local_credentials(self):
        with tempfile.TemporaryDirectory() as tmp, isolated_environment(tmp):
            identity = write_env(
                tmp,
                "worker.env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="worker",
                LOCA_MEMBERSHIP="mb_worker",
                DAVET_project_x="dv_keep",
            )
            credentials.load_local_env("worker")
            with mock.patch.object(
                listen.urllib.request,
                "urlopen",
                side_effect=RuntimeError("network down"),
            ):
                removed = listen.reconcile_room_invites(
                    "wss://loca.example/lobby/ws", "mb_worker"
                )

            self.assertEqual(removed, [])
            self.assertEqual(os.environ["DAVET_project_x"], "dv_keep")
            self.assertIn(
                "DAVET_project_x=dv_keep",
                identity.read_text(encoding="utf-8"),
            )

    def _reconcile_with_whoami(self, tmp, outcome):
        """Run /whoami reconciliation with one canned outcome.

        Returns ``(removed, kept_intact)``; never exposes an invite value, so
        a failure message cannot print one.
        """
        identity = write_env(
            tmp,
            "worker.env",
            ROOM_SERVER_URL="https://loca.example",
            LOCA_NAME="worker",
            LOCA_MEMBERSHIP="mb_synthetic_worker",
            DAVET_project_x="dv_synthetic_keep_x",
            DAVET_sb_dev="dv_synthetic_keep_dev",
        )
        credentials.load_local_env("worker")

        class Response(io.BytesIO):
            def __enter__(self):
                return self

            def __exit__(self, *_):
                return False

        if isinstance(outcome, BaseException):
            patch = {"side_effect": outcome}
        else:
            patch = {"return_value": Response(outcome)}
        stderr = io.StringIO()
        with mock.patch.object(listen.urllib.request, "urlopen", **patch), \
                mock.patch.object(listen.sys, "stderr", stderr):
            removed = listen.reconcile_room_invites(
                "wss://loca.example/lobby/ws", "mb_synthetic_worker"
            )
        saved = identity.read_text(encoding="utf-8")
        kept_intact = (
            os.environ.get("DAVET_project_x") == "dv_synthetic_keep_x"
            and os.environ.get("DAVET_sb_dev") == "dv_synthetic_keep_dev"
            and "DAVET_project_x=dv_synthetic_keep_x" in saved
            and "DAVET_sb_dev=dv_synthetic_keep_dev" in saved
        )
        self.assertFalse(
            any(secret in stderr.getvalue() for secret in (
                "mb_synthetic_worker", "dv_synthetic_keep_x", "dv_synthetic_keep_dev"
            )),
            "reconciliation diagnostics must not print a membership or invite",
        )
        return removed, kept_intact

    def test_whoami_reconciliation_keeps_invites_through_transport_failures(self):
        failures = {
            "timeout": socket.timeout("timed out"),
            "connection reset": ConnectionResetError("reset by peer"),
            "5xx": listen.HTTPError(
                "https://loca.example:443/whoami", 503, "unavailable", {}, None
            ),
            "unparsable body": b"<html>bad gateway</html>",
            "truncated body": b'{"kind": "member", "locas": ["sb-d',
        }
        for label, outcome in failures.items():
            with self.subTest(failure=label), tempfile.TemporaryDirectory() as tmp, \
                    isolated_environment(tmp):
                removed, kept_intact = self._reconcile_with_whoami(tmp, outcome)
                self.assertEqual(removed, [])
                self.assertTrue(kept_intact, f"{label} deleted a kept invite")

    def test_whoami_reconciliation_keeps_invites_on_incomplete_member_answers(self):
        answers = {
            "no locas": {"kind": "member", "name": "worker"},
            "locas null": {"kind": "member", "name": "worker", "locas": None},
            "locas object": {"kind": "member", "locas": {"sb-dev": True}},
            "not a member": {"kind": "session", "locas": []},
            "no kind": {"locas": []},
        }
        for label, answer in answers.items():
            with self.subTest(answer=label), tempfile.TemporaryDirectory() as tmp, \
                    isolated_environment(tmp):
                removed, kept_intact = self._reconcile_with_whoami(
                    tmp, json.dumps(answer).encode()
                )
                self.assertEqual(removed, [])
                self.assertTrue(kept_intact, f"{label} deleted a kept invite")

    def test_whoami_reconciliation_rejects_a_body_that_is_not_an_object(self):
        for label, body in {"list": [], "string": "member", "null": None}.items():
            with self.subTest(body=label), tempfile.TemporaryDirectory() as tmp, \
                    isolated_environment(tmp):
                removed, kept_intact = self._reconcile_with_whoami(
                    tmp, json.dumps(body).encode()
                )
                self.assertEqual(removed, [])
                self.assertTrue(kept_intact, f"{label} body deleted a kept invite")

    def test_whoami_reconciliation_rejects_loca_entries_that_are_not_names(self):
        answers = {
            "null entry": [None],
            "number entry": [7, "sb-dev"],
            "empty name": ["", "sb-dev"],
            "object entry": [{"room": "project-x"}, "sb-dev"],
        }
        for label, locas in answers.items():
            with self.subTest(locas=label), tempfile.TemporaryDirectory() as tmp, \
                    isolated_environment(tmp):
                removed, kept_intact = self._reconcile_with_whoami(
                    tmp,
                    json.dumps({"kind": "member", "locas": locas}).encode(),
                )
                self.assertEqual(removed, [])
                self.assertTrue(kept_intact, f"{label} deleted a kept invite")

    def test_invite_id_matches_the_server_derivation(self):
        # Expected value computed independently of Python's hashlib:
        #   printf '%s' 'invite:dv_synthetic_vector_0001' | sha256sum | cut -c1-32
        # The derivation is the server's `invite_management_id`
        # (crates/server/src/hub.rs) over `store::hashed_id` (store.rs).
        self.assertEqual(
            listen.invite_management_id("dv_synthetic_vector_0001"),
            "ivid_8f9b089836861ab5924a97e66a895fef",
        )

    def test_seat_snapshot_accepts_extra_fields_and_rejects_incomplete_lists(self):
        self.assertEqual(
            listen.parse_seat_snapshot(
                [{"room": "sb-dev", "invite_id": "ivid_a", "since": 1}]
            ),
            {"sb-dev": "ivid_a"},
        )
        self.assertEqual(listen.parse_seat_snapshot([]), {})
        secret = "dv_synthetic_must_not_echo"
        for seats in (
            None,
            {"sb-dev": "ivid_a"},
            [secret],
            [{"room": "sb-dev"}],
            [{"invite_id": "ivid_a"}],
            [{"room": "", "invite_id": "ivid_a"}],
            [{"room": "sb-dev", "invite_id": 7}],
            [{"room": secret, "invite_id": "ivid_a"},
             {"room": secret, "invite_id": "ivid_b"}],
        ):
            with self.assertRaises(ValueError) as caught:
                listen.parse_seat_snapshot(seats)
            self.assertNotIn(secret, str(caught.exception))

    def test_lobby_full_snapshot_replaces_same_room_token_and_stale_rooms_atomically(self):
        with tempfile.TemporaryDirectory() as tmp, isolated_environment(tmp):
            identity = write_env(
                tmp,
                "lead.env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="lead",
                LOCA_MEMBERSHIP="mb_lead",
                LOCA_SESSION="stale-session",
                DAVET_sb_dev="dv_old",
                DAVET_reviewer="dv_stale",
            )
            credentials.load_local_env("lead")

            removed = listen.reconcile_room_invite_snapshot(
                [{"room": "sb-dev", "token": "dv_fresh"}]
            )

            self.assertEqual(removed, ["reviewer"])
            self.assertEqual(os.environ["DAVET_sb_dev"], "dv_fresh")
            self.assertNotIn("DAVET_reviewer", os.environ)
            self.assertNotIn("LOCA_SESSION", os.environ)
            saved = identity.read_text(encoding="utf-8")
            self.assertIn("DAVET_sb_dev=dv_fresh", saved)
            self.assertNotIn("DAVET_reviewer=", saved)
            self.assertNotIn("LOCA_SESSION=", saved)

    def test_replayed_identical_room_invite_does_not_drop_session(self):
        with tempfile.TemporaryDirectory() as tmp, isolated_environment(tmp):
            identity = write_env(
                tmp,
                "worker.env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="worker",
                LOCA_SESSION="live-session",
                DAVET_sb_dev="dv_same",
            )
            credentials.load_local_env("worker")
            self.assertFalse(listen.store_room_invite("sb-dev", "dv_same"))
            self.assertEqual(os.environ["LOCA_SESSION"], "live-session")
            self.assertIn("LOCA_SESSION=live-session", identity.read_text())

    def test_bot_defaults_to_per_loca_invite(self):
        with tempfile.TemporaryDirectory() as tmp, isolated_environment(tmp):
            write_env(
                tmp,
                "sb-feature.env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="sb-feature",
                DAVET_sb_mobile="invite-feature",
            )
            bot_args = bot.parse_args(
                ["--name", "sb-feature", "--room", "sb-mobile"]
            )
            self.assertEqual(bot_args.server, "https://loca.example")
            self.assertEqual(bot_args.token, "invite-feature")

    @unittest.skipIf(
        AGENTD is None,
        "generic-command adapter is outside the standalone skill installation",
    )
    def test_generic_adapter_defaults_to_per_loca_invite(self):
        with tempfile.TemporaryDirectory() as tmp, isolated_environment(tmp):
            write_env(
                tmp,
                "sb-feature.env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="sb-feature",
                DAVET_sb_mobile="invite-feature",
            )
            adapter_args = AGENTD.parse_args(
                [
                    "--name",
                    "sb-feature",
                    "--room",
                    "sb-mobile",
                    "--cmd",
                    "true",
                ]
            )
            self.assertEqual(adapter_args.server, "https://loca.example")
            self.assertEqual(adapter_args.token, "invite-feature")

    def test_connect_sh_uses_named_file_and_never_prints_its_secret(self):
        with tempfile.TemporaryDirectory() as tmp:
            write_env(
                tmp,
                "env",
                ROOM_SERVER_URL="http://127.0.0.1:9",
                LOCA_NAME="loca-dev",
                DAVET_sb_mobile="wrong-default",
            )
            write_env(
                tmp,
                "sb-feature.env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="sb-feature",
                DAVET_sb_mobile="TOP-SECRET-INVITE",
            )
            env = {
                "HOME": tmp,
                "PATH": os.environ.get("PATH", ""),
                "ROOM_LOG": str(Path(tmp) / "listener.jsonl"),
            }
            result = subprocess.run(
                [
                    str(SKILL_DIR / "connect.sh"),
                    "listen",
                    "http://127.0.0.1:9",
                    "sb-mobile",
                    "sb-feature",
                ],
                env=env,
                text=True,
                capture_output=True,
                timeout=5,
            )
            output = result.stdout + result.stderr
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("credential server mismatch", output)
            self.assertNotIn("TOP-SECRET-INVITE", output)

    def test_connect_sh_refuses_default_file_for_another_named_agent(self):
        with tempfile.TemporaryDirectory() as tmp:
            write_env(
                tmp,
                "env",
                ROOM_SERVER_URL="http://127.0.0.1:9",
                LOCA_NAME="loca-dev",
                DAVET_sb_mobile="wrong-default",
            )
            result = subprocess.run(
                [
                    str(SKILL_DIR / "connect.sh"),
                    "send",
                    "http://127.0.0.1:9",
                    "sb-mobile",
                    "sb-feature",
                    "-",
                    "hello",
                ],
                env={"HOME": tmp, "PATH": os.environ.get("PATH", "")},
                text=True,
                capture_output=True,
                timeout=5,
            )
            self.assertEqual(result.returncode, 6)
            self.assertIn("refusing to impersonate", result.stderr)

    def test_connect_sh_reuses_delivery_operation_id_for_a_reply(self):
        with tempfile.TemporaryDirectory() as tmp:
            write_env(
                tmp,
                "worker.env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="worker",
                LOCA_SESSION="session-worker",
                DAVET_reviewer="invite-worker",
            )
            fake_bin = Path(tmp) / "bin"
            fake_bin.mkdir()
            capture = Path(tmp) / "body.json"
            curl = fake_bin / "curl"
            curl.write_text(
                """#!/bin/sh
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-d" ]; then
    shift
    printf '%s' "$1" > "$CAPTURE"
  fi
  shift
done
printf '{"id":1,"room":"reviewer","sender":"worker"}\\n201'
""",
                encoding="utf-8",
            )
            curl.chmod(0o755)
            result = subprocess.run(
                [
                    str(SKILL_DIR / "connect.sh"),
                    "send",
                    "https://loca.example",
                    "reviewer",
                    "worker",
                    "operator",
                    "done",
                ],
                env={
                    "HOME": tmp,
                    "PATH": f"{fake_bin}:{os.environ.get('PATH', '')}",
                    "CAPTURE": str(capture),
                    "LOCA_OP_ID": "loca-reviewer:103",
                    "LOCA_REPLY_TO": "103",
                },
                text=True,
                capture_output=True,
                timeout=5,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            body = json.loads(capture.read_text(encoding="utf-8"))
            self.assertEqual(body["op_id"], "loca-reviewer:103")
            self.assertEqual(body["reply_to"], 103)
            self.assertEqual(body["text"], "done")

    def test_release_uses_named_invite_and_clears_only_local_seat_credentials(self):
        with tempfile.TemporaryDirectory() as tmp:
            identity = write_env(
                tmp,
                "worker.env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="worker",
                LOCA_SESSION="stale-session",
                DAVET_project_x="TOP-SECRET-INVITE",
                DAVET_other="keep-this-invite",
            )
            fake_bin = Path(tmp) / "bin"
            fake_bin.mkdir()
            curl = fake_bin / "curl"
            curl.write_text(
                """#!/bin/sh
case "$*" in
  *"/rooms/project-x/release"*) printf '204'; exit 0 ;;
esac
printf '500'
""",
                encoding="utf-8",
            )
            curl.chmod(0o755)
            result = subprocess.run(
                [
                    str(SKILL_DIR / "connect.sh"),
                    "release",
                    "https://loca.example",
                    "project-x",
                    "worker",
                ],
                env={
                    "HOME": tmp,
                    "PATH": f"{fake_bin}:{os.environ.get('PATH', '')}",
                },
                text=True,
                capture_output=True,
                timeout=5,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("membership stays in lobby", result.stdout)
            self.assertNotIn("TOP-SECRET-INVITE", result.stdout + result.stderr)
            saved = identity.read_text(encoding="utf-8")
            self.assertNotIn("DAVET_project_x=", saved)
            self.assertNotIn("LOCA_SESSION=", saved)
            self.assertIn("DAVET_other=keep-this-invite", saved)

    def test_setup_rejects_bad_invite_before_writing_named_identity(self):
        with tempfile.TemporaryDirectory() as tmp:
            write_env(
                tmp,
                "env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="cyber",
                DAVET_general="cyber-invite",
            )
            fake_bin = Path(tmp) / "bin"
            fake_bin.mkdir()
            curl = fake_bin / "curl"
            curl.write_text(
                """#!/bin/sh
case "$*" in
  *"/health"*) exit 0 ;;
  *"%{http_code}"*) printf '401'; exit 0 ;;
esac
exit 1
""",
                encoding="utf-8",
            )
            curl.chmod(0o755)
            result = subprocess.run(
                [
                    str(SKILL_DIR / "connect.sh"),
                    "setup",
                    "https://loca.example",
                    "debug",
                ],
                env={
                    "HOME": tmp,
                    "PATH": f"{fake_bin}:{os.environ.get('PATH', '')}",
                },
                input="bad-invite\n",
                text=True,
                capture_output=True,
                timeout=5,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("using separate file", result.stdout)
            self.assertIn("no identity file was changed", result.stderr)
            self.assertFalse((Path(tmp) / ".loca" / "debug.env").exists())

    def test_setup_rejects_bootstrap_credential_in_process_arguments(self):
        with tempfile.TemporaryDirectory() as tmp:
            result = subprocess.run(
                [
                    str(SKILL_DIR / "connect.sh"),
                    "setup",
                    "https://loca.example",
                    "worker",
                    "dv_must_not_enter_ps",
                ],
                env={"HOME": tmp, "PATH": os.environ.get("PATH", "")},
                text=True,
                capture_output=True,
                timeout=5,
            )
            self.assertEqual(result.returncode, 2)
            self.assertIn("must not be passed as an argument", result.stderr)
            self.assertNotIn("dv_must_not_enter_ps", result.stdout + result.stderr)

    def test_setup_requires_an_explicit_server_origin(self):
        with tempfile.TemporaryDirectory() as tmp:
            result = subprocess.run(
                [str(SKILL_DIR / "connect.sh"), "setup"],
                env={"HOME": tmp, "PATH": os.environ.get("PATH", "")},
                input="dv_private\n",
                text=True,
                capture_output=True,
                timeout=5,
            )
            self.assertEqual(result.returncode, 2)
            self.assertIn("server is required", result.stderr)
            self.assertNotIn("loca.speakbetter.tech", result.stdout + result.stderr)

    def test_update_json_uses_the_atomic_credential_writer(self):
        with tempfile.TemporaryDirectory() as tmp:
            identity = Path(tmp) / "worker.env"
            result = subprocess.run(
                [
                    sys.executable,
                    str(SKILL_DIR / "credentials.py"),
                    "update-json",
                    str(identity),
                ],
                input=json.dumps(
                    {
                        "ROOM_SERVER_URL": "https://loca.example",
                        "LOCA_NAME": "worker",
                        "LOCA_MEMBERSHIP": "mb_private",
                    }
                ),
                text=True,
                capture_output=True,
                timeout=5,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            saved = identity.read_text(encoding="utf-8")
            self.assertIn("LOCA_NAME=worker", saved)
            self.assertIn("LOCA_MEMBERSHIP=mb_private", saved)


if __name__ == "__main__":
    unittest.main()
