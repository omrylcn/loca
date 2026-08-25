import importlib.util
import io
import json
import os
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

    def test_revoked_davet_is_distinct_from_a_transient_session_failure(self):
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
                DAVET_sb_mobile="davet-feature",
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
                credentials.token_for_room("sb-mobile"), "davet-feature"
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
                DAVET_sb_mobile="davet-feature",
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
                DAVET_sb_mobile="davet-feature",
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
                    "loca.room.davet-feature",
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

    def test_listener_rest_helpers_use_local_davet_after_url_auth_removal(self):
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

    def test_listener_claims_and_persists_the_lobby_membership(self):
        with tempfile.TemporaryDirectory() as tmp, isolated_environment(tmp):
            identity = write_env(
                tmp,
                "worker.env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="worker",
                DAVET_project_x="davet-worker",
            )
            credentials.load_local_env("worker")

            class Response(io.BytesIO):
                def __enter__(self):
                    return self

                def __exit__(self, *_):
                    return False

            response = Response(
                json.dumps(
                    {
                        "membership_token": "mb_worker",
                        "name": "worker",
                        "kind": "agent",
                    }
                ).encode()
            )
            with mock.patch.object(
                listen.urllib.request, "urlopen", return_value=response
            ) as opened:
                claimed = listen.claim_membership(
                    "wss://loca.example/ws?room=project-x&name=worker"
                )

            self.assertEqual(claimed, "mb_worker")
            self.assertEqual(os.environ["LOCA_MEMBERSHIP"], "mb_worker")
            self.assertIn(
                "LOCA_MEMBERSHIP=mb_worker",
                identity.read_text(encoding="utf-8"),
            )
            request = opened.call_args.args[0]
            self.assertEqual(
                request.full_url, "https://loca.example:443/membership/claim"
            )
            self.assertEqual(request.get_header("X-room-token"), "davet-worker")

    def test_listener_skips_a_stale_davet_when_claiming_membership(self):
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

            class Response(io.BytesIO):
                def __enter__(self):
                    return self

                def __exit__(self, *_):
                    return False

            live = Response(
                json.dumps(
                    {
                        "membership_token": "mb_feature",
                        "name": "sb-feature",
                        "kind": "agent",
                    }
                ).encode()
            )
            with mock.patch.object(
                listen.urllib.request,
                "urlopen",
                side_effect=[RuntimeError("HTTP 401"), live],
            ) as opened:
                claimed = listen.claim_membership(
                    "wss://loca.example/ws?room=sb-mobile&name=sb-feature"
                )

            self.assertEqual(claimed, "mb_feature")
            self.assertEqual(opened.call_count, 2)
            attempted = [
                call.args[0].get_header("X-room-token")
                for call in opened.call_args_list
            ]
            self.assertEqual(attempted, ["dv_stale", "dv_live"])
            self.assertIn(
                "LOCA_MEMBERSHIP=mb_feature",
                identity.read_text(encoding="utf-8"),
            )

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

    def test_lobby_call_replaces_davet_and_discards_old_scoped_session(self):
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

    def test_lobby_snapshot_removes_only_server_absent_local_davets(self):
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

    def test_bot_defaults_to_per_loca_davet(self):
        with tempfile.TemporaryDirectory() as tmp, isolated_environment(tmp):
            write_env(
                tmp,
                "sb-feature.env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="sb-feature",
                DAVET_sb_mobile="davet-feature",
            )
            bot_args = bot.parse_args(
                ["--name", "sb-feature", "--room", "sb-mobile"]
            )
            self.assertEqual(bot_args.server, "https://loca.example")
            self.assertEqual(bot_args.token, "davet-feature")

    @unittest.skipIf(
        AGENTD is None,
        "generic-command adapter is outside the standalone skill installation",
    )
    def test_generic_adapter_defaults_to_per_loca_davet(self):
        with tempfile.TemporaryDirectory() as tmp, isolated_environment(tmp):
            write_env(
                tmp,
                "sb-feature.env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="sb-feature",
                DAVET_sb_mobile="davet-feature",
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
            self.assertEqual(adapter_args.token, "davet-feature")

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
                DAVET_sb_mobile="TOP-SECRET-DAVET",
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
            self.assertNotIn("TOP-SECRET-DAVET", output)

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
                DAVET_reviewer="davet-worker",
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
printf '{}\\n200'
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

    def test_release_uses_named_davet_and_clears_only_local_seat_credentials(self):
        with tempfile.TemporaryDirectory() as tmp:
            identity = write_env(
                tmp,
                "worker.env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="worker",
                LOCA_SESSION="stale-session",
                DAVET_project_x="TOP-SECRET-DAVET",
                DAVET_other="keep-this-davet",
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
            self.assertNotIn("TOP-SECRET-DAVET", result.stdout + result.stderr)
            saved = identity.read_text(encoding="utf-8")
            self.assertNotIn("DAVET_project_x=", saved)
            self.assertNotIn("LOCA_SESSION=", saved)
            self.assertIn("DAVET_other=keep-this-davet", saved)

    def test_setup_rejects_bad_davet_before_writing_named_identity(self):
        with tempfile.TemporaryDirectory() as tmp:
            write_env(
                tmp,
                "env",
                ROOM_SERVER_URL="https://loca.example",
                LOCA_NAME="cyber",
                DAVET_general="cyber-davet",
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
                input="bad-davet\n",
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
