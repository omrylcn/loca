"""An agent whose session ended carries on by itself, against the real server.

Every session now ends: after twelve hours, or earlier when its credential
takes a seventeenth (the oldest is ended). The client sees the same refusal
either way, so these tests end sessions through the cap and through sign-out
instead of waiting twelve hours, and prove the agent kit renews from the
invite it holds without anyone doing anything:

- ``connect.sh send`` renews and retries once;
- the loca listener, seated in several locas that share one ``LOCA_SESSION``,
  renews on a handshake 401 and keeps delivering;
- the Lobby listener, which never uses a session, still takes a new call.
"""

import json
import os
import subprocess
import tempfile
import time
import unittest
import urllib.error
import urllib.request
from pathlib import Path

from test_listener_backfill_e2e import SERVER_BIN, free_port

SKILL_DIR = Path(__file__).resolve().parents[1]
REPO_ROOT = SKILL_DIR.parents[1]
MASTER = "expiry-e2e-master"
# SESSIONS_PER_CREDENTIAL in the server.
CAP = 16


def call(base, path, *, method="GET", headers=None, body=None):
    encoded = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(f"{base}{path}", data=encoded, method=method)
    if encoded is not None:
        request.add_header("content-type", "application/json")
    for key, value in (headers or {}).items():
        request.add_header(key, value)
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            payload = response.read()
            return response.status, json.loads(payload.decode()) if payload else None
    except urllib.error.HTTPError as error:
        return error.code, None


def master(base, path, body=None, method="POST"):
    status, payload = call(
        base, path, method=method, headers={"x-admin-token": MASTER}, body=body
    )
    if status >= 300:
        raise AssertionError(f"{method} {path} -> {status}")
    return payload


def take_session(base, invite):
    status, payload = call(
        base,
        "/sessions",
        method="POST",
        headers={"x-room-token": invite},
        body={"name": "worker", "kind": "agent"},
    )
    assert status == 201, status
    return payload["session_token"]


def session_works(base, room, session):
    status, _ = call(
        base, f"/rooms/{room}/messages", headers={"x-session-token": session}
    )
    return status == 200


def read_env(path):
    values = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        if "=" in line:
            key, value = line.split("=", 1)
            values[key] = value
    return values


class SessionExpiryEndToEndTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not SERVER_BIN.is_file():
            subprocess.run(
                ["cargo", "build", "-q", "-p", "server"],
                cwd=REPO_ROOT,
                check=True,
                timeout=600,
            )

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.port = free_port()
        self.base = f"http://127.0.0.1:{self.port}"
        env = dict(os.environ)
        env.update(
            {
                "PORT": str(self.port),
                "BIND_ADDR": "127.0.0.1",
                "DB_PATH": str(self.root / "loca.sqlite3"),
                "ADMIN_TOKEN": MASTER,
                "REQUIRE_INVITE": "1",
                "REQUIRE_SESSIONS": "1",
                "LEGACY_WS_QUERY_AUTH": "0",
                "RUST_LOG": "warn",
            }
        )
        env.pop("ROOM_TOKEN", None)
        self.server_log = (self.root / "server.log").open("ab")
        self.server = subprocess.Popen(
            [str(SERVER_BIN)],
            cwd=REPO_ROOT,
            env=env,
            stdout=self.server_log,
            stderr=subprocess.STDOUT,
        )
        self.addCleanup(self.stop, self.server)
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            try:
                if call(self.base, "/health")[0] == 200:
                    break
            except OSError:
                pass
            time.sleep(0.05)
        else:
            self.fail("room-server did not become healthy")
        self.member = master(self.base, "/members", {"name": "worker", "kind": "agent"})
        self.home = self.root / "home"
        (self.home / ".loca").mkdir(parents=True)
        self.env_file = self.home / ".loca" / "worker.env"

    def tearDown(self):
        self.server_log.close()
        self.tmp.cleanup()

    @staticmethod
    def stop(process):
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)

    def call_into(self, room):
        master(self.base, "/rooms", {"name": room})
        return master(self.base, f"/rooms/{room}/invites", {"name": "worker"})["token"]

    def write_env(self, lines):
        self.env_file.write_text("\n".join(lines) + "\n", encoding="utf-8")
        self.env_file.chmod(0o600)

    def kit_env(self):
        env = dict(os.environ)
        for key in list(env):
            if key.startswith("DAVET_") or key in (
                "ROOM_TOKEN", "LOCA_SESSION", "LOCA_MEMBERSHIP", "LOCA_NAME",
            ):
                env.pop(key)
        env.update({"HOME": str(self.home), "LOCA_ENV": str(self.env_file)})
        return env

    def end_by_cap(self, invite):
        """Take a full set of new sessions with this invite: every older
        session taken with it is ended."""
        for _ in range(CAP):
            take_session(self.base, invite)

    def test_a_post_after_the_session_ended_renews_from_the_invite_and_retries_once(self):
        invite = self.call_into("alpha")
        stale = take_session(self.base, invite)
        self.end_by_cap(invite)
        self.assertFalse(session_works(self.base, "alpha", stale))
        self.write_env(
            [
                f"ROOM_SERVER_URL={self.base}",
                "LOCA_NAME=worker",
                f"DAVET_alpha={invite}",
                f"LOCA_SESSION={stale}",
            ]
        )
        result = subprocess.run(
            [
                str(SKILL_DIR / "connect.sh"), "send", self.base, "alpha", "worker",
                "-", "said after the session ended",
            ],
            env=self.kit_env(),
            text=True,
            capture_output=True,
            timeout=30,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stderr.count("session renewed"), 1, result.stderr)
        renewed = read_env(self.env_file)["LOCA_SESSION"]
        self.assertNotEqual(renewed, stale)
        self.assertTrue(session_works(self.base, "alpha", renewed))
        _, history = call(
            self.base, "/rooms/alpha/messages", headers={"x-admin-token": MASTER}
        )
        texts = [m["text"] for m in history]
        self.assertEqual(texts.count("said after the session ended"), 1)

    def test_listeners_in_several_locas_sharing_one_session_renew_and_the_lobby_still_calls(self):
        invite_a = self.call_into("alpha")
        invite_b = self.call_into("beta")
        # One LOCA_SESSION slot, scoped to alpha: beta must renew at once.
        shared = take_session(self.base, invite_a)
        self.write_env(
            [
                f"ROOM_SERVER_URL={self.base}",
                "LOCA_NAME=worker",
                f"LOCA_MEMBERSHIP={self.member['token']}",
                f"DAVET_alpha={invite_a}",
                f"DAVET_beta={invite_b}",
                f"LOCA_SESSION={shared}",
            ]
        )
        history = self.root / "history.jsonl"
        listener_log = self.root / "listener.log"
        log = listener_log.open("ab")
        self.addCleanup(log.close)
        listener = subprocess.Popen(
            [
                "python3", "-u", str(SKILL_DIR / "listen.py"),
                f"ws://127.0.0.1:{self.port}/ws?room=alpha&name=worker&type=agent&filter=mentions",
                str(history),
                "--skip-own", "worker",
            ],
            env=self.kit_env(),
            stdout=subprocess.DEVNULL,
            stderr=log,
        )
        self.addCleanup(self.stop, listener)

        def delivered(text, timeout):
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                if listener.poll() is not None:
                    self.fail("listener exited:\n" + listener_log.read_text(errors="replace"))
                if history.exists() and text in history.read_text(errors="replace"):
                    return True
                time.sleep(0.2)
            self.fail(
                f"{text!r} was not delivered:\n" + listener_log.read_text(errors="replace")
            )

        # REQUIRE_SESSIONS: the operator speaks through a session of its own
        # (the root credential's, so the worker's count is not touched).
        speaker = master(self.base, "/sessions", {"name": "operator", "kind": "user"})

        def say(room, text):
            status, _ = call(
                self.base,
                f"/rooms/{room}/messages",
                method="POST",
                headers={
                    "x-admin-token": MASTER,
                    "x-session-token": speaker["session_token"],
                },
                body={"sender": "operator", "sender_type": "user", "text": text},
            )
            self.assertLess(status, 300, f"post to {room}")

        def listening(room, times, timeout=30):
            """Wait for the listener to have (re)connected `room` `times` times."""
            marker = f"[{room}] listening"
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                if listener_log.read_text(errors="replace").count(marker) >= times:
                    return
                if listener.poll() is not None:
                    break
                time.sleep(0.2)
            self.fail(f"{room} not listening:\n" + listener_log.read_text(errors="replace"))

        listening("alpha", 1)
        listening("beta", 1)
        say("alpha", "@worker before-a")
        say("beta", "@worker before-b")
        delivered("before-a", 30)
        delivered("before-b", 30)

        # Every session this identity holds ends; open sockets close at the
        # next ping and must come back with renewed sessions on their own.
        # An invite's sessions are counted by that invite, so each loca's are
        # ended through its own.
        self.end_by_cap(invite_a)
        self.end_by_cap(invite_b)
        listening("alpha", 2, 60)
        listening("beta", 2, 60)
        say("alpha", "@worker after-a")
        say("beta", "@worker after-b")
        delivered("after-a", 60)
        delivered("after-b", 60)

        # The Lobby connection never held a session: a call still arrives and
        # the new loca's listener renews from its fresh invite.
        self.call_into("gamma")
        listening("gamma", 1, 60)
        say("gamma", "@worker after-call")
        delivered("after-call", 60)
        self.assertIsNone(listener.poll())


if __name__ == "__main__":
    unittest.main()
