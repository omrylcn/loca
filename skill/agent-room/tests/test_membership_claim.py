"""The kit's invite claim (`join_request.py claim`, `connect.sh setup` with an
invite, the listener's claim): one set-up, one claim id, one key.

A stdlib ThreadingHTTPServer stands in for room-server and keeps what the
real one keeps: `(invite, claim id) -> key`, a delivery copy until the ACK,
five set-ups per invite. Each test builds a hermetic environment. Keys and
claim ids are asserted on in the env file, the state file and the mock's
records, never read from the helper's output, which must not carry them.
"""

import json
import os
import re
import subprocess
import sys
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from unittest import mock

SKILL_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SKILL_DIR))

import join_request  # noqa: E402

INVITE = "dv_" + "a1" * 32
NAME = "worker"
ORIGINAL = "mb_" + "0f" * 32


class ClaimMock(BaseHTTPRequestHandler):
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
        st = self.server.state
        token = self.headers.get("x-room-token")
        if self.path == "/health":
            self._json({"ok": True})
        elif self.path == "/whoami" and st["whoami_failures"] > 0:
            # The server cannot be reached for this one check.
            st["whoami_failures"] -= 1
            self._json({"error": "gateway"}, 502)
        elif self.path == "/whoami" and token in st["keys"]:
            self._json({"kind": "member", "name": st["keys"][token], "locas": ["parlor"]})
        elif self.path == "/whoami" and token == INVITE:
            self._json({"kind": "davet", "loca": "parlor"})
        elif self.path == "/rooms" and token == INVITE:
            self._json([{"room": "parlor", "members": 0}])
        elif self.path == "/rooms/parlor/messages" and token == INVITE:
            self._json([])
        else:
            self._json({"error": "unauthorized"}, 401)

    def do_POST(self):
        st = self.server.state
        length = int(self.headers.get("content-length", 0) or 0)
        if length:
            self.rfile.read(length)
        invite = self.headers.get("x-room-token")
        claim_id = self.headers.get("x-claim-id")
        if self.path == "/membership/claim":
            st["claims"].append(claim_id)
            if invite != INVITE:
                self._json({"error": "no davet"}, 401)
                return
            if st["old_server"]:
                # An older server ignores the claim id and hands out the
                # member's own key.
                st["keys"][ORIGINAL] = st["member_name"]
                self._json({"membership_token": ORIGINAL, "name": st["member_name"],
                            "kind": "agent"})
                return
            setup = st["setups"].get(claim_id) if claim_id else None
            if setup:
                if setup["acked"]:
                    self._json({"error": "already delivered"}, 409)
                    return
                if st["expired"]:
                    self._json({"error": "delivery ended"}, 410)
                    return
                self._json({"membership_token": setup["key"], "name": st["member_name"],
                            "kind": "agent", "ack_required": True}, 200)
                return
            if st["minted"] >= 5:
                self._json({"error": "cap"}, 429)
                return
            st["minted"] += 1
            key = "mb_claimed%02d" % st["minted"] + "c" * 54
            st["keys"][key] = st["member_name"]
            if claim_id:
                st["setups"][claim_id] = {"key": key, "acked": False}
            if st["lose_answers"] > 0:
                # The server minted the key, but its answer never arrived.
                st["lose_answers"] -= 1
                self._json({"error": "gateway"}, 502)
                return
            self._json({"membership_token": key, "name": st["member_name"], "kind": "agent",
                        "ack_required": bool(claim_id)}, 201)
            return
        if self.path == "/membership/claim/ack" and not st["old_server"]:
            st["acks"].append(claim_id)
            if st["env_path"]:
                # What the identity file held at the moment of the ACK.
                st["held_at_ack"].append(
                    env_values(Path(st["env_path"])).get("LOCA_MEMBERSHIP"))
            if st["ack_failures"] > 0:
                st["ack_failures"] -= 1
                self._json({"error": "storage"}, 503)
                return
            setup = st["setups"].get(claim_id)
            if invite != INVITE or not setup:
                self._json({"error": "unknown"}, 404)
                return
            setup["acked"] = True
            self._json({"acknowledged": True})
            return
        if self.path == "/sessions":
            self._json({"session_token": "st_" + "5" * 64, "name": NAME}, 201)
            return
        self._json({"error": "not found"}, 404)


def start(**flags):
    server = ThreadingHTTPServer(("127.0.0.1", 0), ClaimMock)
    server.state = {"keys": {}, "setups": {}, "claims": [], "acks": [], "minted": 0,
                    "member_name": NAME, "old_server": False, "expired": False,
                    "lose_answers": 0, "ack_failures": 0, "whoami_failures": 0,
                    "env_path": None, "held_at_ack": []}
    server.state.update(flags)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, "http://127.0.0.1:%d" % server.server_address[1]


def env_values(path):
    if not path.exists():
        return {}
    return dict(line.split("=", 1) for line in path.read_text().splitlines() if "=" in line)


def no_secrets(test, text):
    test.assertNotRegex(text, r"mb_[0-9a-z]{8}|dv_[0-9a-f]{8}|cl_[A-Za-z0-9_-]{8}")


class ClaimHelperTest(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(self.enterContext(tempfile.TemporaryDirectory()))
        (self.tmp / ".loca").mkdir()
        self.state = self.tmp / ".loca" / "worker.claim"
        self.env = self.tmp / ".loca" / "worker.env"

    def serve(self, **flags):
        server, base = start(**flags)
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        return server, base

    def run_claim(self, base, invite=INVITE, name=NAME):
        env = {k: v for k, v in os.environ.items()
               if not k.startswith(("LOCA_", "ROOM_", "DAVET_"))}
        env["HOME"] = str(self.tmp)
        return subprocess.run(
            [sys.executable, str(SKILL_DIR / "join_request.py"), "claim", base, name,
             str(self.state), str(self.env)],
            input=invite + "\n", env=env, capture_output=True, text=True, timeout=30,
        )

    def test_a_claim_files_verifies_and_acknowledges_one_key(self):
        server, base = self.serve()
        result = self.run_claim(base)
        self.assertEqual(result.returncode, 0, result.stderr)
        st = server.state
        self.assertEqual(st["minted"], 1)
        (claim_id,) = set(st["claims"])
        self.assertRegex(claim_id, r"^cl_[A-Za-z0-9_-]{43}$")
        self.assertEqual(st["acks"], [claim_id])
        key = env_values(self.env)["LOCA_MEMBERSHIP"]
        self.assertEqual(st["setups"][claim_id], {"key": key, "acked": True})
        self.assertEqual(self.env.stat().st_mode & 0o777, 0o600)
        self.assertFalse(self.state.exists(), "a finished set-up leaves no state file")
        no_secrets(self, result.stdout + result.stderr)

    def test_the_state_file_is_written_before_the_claim_and_is_private(self):
        server, base = self.serve()
        seen = {}
        real_http = join_request.http

        def spy(method, url, headers=None, body=None):
            if url.endswith("/membership/claim"):
                seen["mode"] = self.state.stat().st_mode & 0o777
                seen["state"] = join_request.read_claim_state(str(self.state))
            return real_http(method, url, headers, body)

        with mock.patch.object(join_request, "http", spy):
            rc = join_request.claim_key(base, NAME, INVITE, str(self.state), str(self.env))
        self.assertEqual(rc, 0)
        self.assertEqual(seen["mode"], 0o600)
        self.assertEqual(seen["state"]["server"], base)
        self.assertEqual(seen["state"]["invite_id"], join_request.invite_management_id(INVITE))
        self.assertEqual([seen["state"]["claim_id"]], server.state["acks"])
        self.assertNotIn(INVITE, self.state.read_text() if self.state.exists() else "")

    def test_a_lost_answer_is_retried_with_the_same_claim_id_and_mints_nothing_more(self):
        server, base = self.serve(lose_answers=1)
        first = self.run_claim(base)
        self.assertEqual(first.returncode, 1, first.stderr)
        self.assertTrue(self.state.exists(), "the set-up is kept for the re-run")
        self.assertNotIn("LOCA_MEMBERSHIP", env_values(self.env))
        second = self.run_claim(base)
        self.assertEqual(second.returncode, 0, second.stderr)
        st = server.state
        self.assertEqual(len(set(st["claims"])), 1, "the re-run resumed the same set-up")
        self.assertEqual(st["minted"], 1)
        self.assertEqual(env_values(self.env)["LOCA_MEMBERSHIP"],
                         st["setups"][st["claims"][0]]["key"])
        self.assertFalse(self.state.exists())

    def test_a_crash_between_receiving_and_storing_resumes_the_same_key(self):
        server, base = self.serve()
        with mock.patch.object(join_request.credentials, "update_env_values",
                               side_effect=OSError("disk full")):
            rc = join_request.claim_key(base, NAME, INVITE, str(self.state), str(self.env))
        self.assertEqual(rc, 1)
        self.assertTrue(self.state.exists())
        self.assertEqual(server.state["acks"], [], "nothing is acknowledged before it is stored")
        rc = join_request.claim_key(base, NAME, INVITE, str(self.state), str(self.env))
        self.assertEqual(rc, 0)
        st = server.state
        self.assertEqual(st["minted"], 1)
        self.assertEqual(len(set(st["claims"])), 1)
        self.assertEqual(env_values(self.env)["LOCA_MEMBERSHIP"],
                         st["setups"][st["claims"][0]]["key"])

    def test_a_crash_between_storing_and_the_ack_finishes_on_the_re_run(self):
        server, base = self.serve(ack_failures=1)
        first = self.run_claim(base)
        self.assertEqual(first.returncode, 6, first.stderr)
        key = env_values(self.env)["LOCA_MEMBERSHIP"]
        self.assertTrue(self.state.exists())
        second = self.run_claim(base)
        self.assertEqual(second.returncode, 0, second.stderr)
        st = server.state
        (claim_id,) = set(st["claims"])
        self.assertEqual(st["acks"], [claim_id, claim_id])
        self.assertEqual(st["minted"], 1)
        self.assertEqual(len(st["claims"]), 1, "the re-run claimed nothing new")
        self.assertEqual(env_values(self.env)["LOCA_MEMBERSHIP"], key)
        self.assertFalse(self.state.exists())

    def test_an_open_claim_is_never_acknowledged_while_another_key_is_held(self):
        """The identity file holds a working key K0. One check of K0 fails on
        the network, so the set-up claims; the server mints K1 and its answer
        is lost. On the re-run K0 verifies again. The ACK clears the server's
        copy of K1, so it may only go out once K1 is the key in the file."""
        server, base = self.serve(lose_answers=1, whoami_failures=1,
                                  env_path=str(self.env))
        st = server.state
        st["keys"][ORIGINAL] = NAME
        self.env.write_text(
            "ROOM_SERVER_URL=%s\nLOCA_NAME=%s\nLOCA_MEMBERSHIP=%s\n" % (base, NAME, ORIGINAL))
        os.chmod(self.env, 0o600)

        first = self.run_claim(base)
        self.assertEqual(first.returncode, 1, first.stderr)
        self.assertEqual(st["minted"], 1, "the server minted this set-up's key")
        self.assertEqual(st["acks"], [])
        self.assertEqual(env_values(self.env)["LOCA_MEMBERSHIP"], ORIGINAL)

        second = self.run_claim(base)
        self.assertEqual(second.returncode, 0, second.stderr)
        (claim_id,) = set(st["claims"])
        claimed = st["setups"][claim_id]["key"]
        self.assertEqual(
            st["held_at_ack"], [claimed],
            "the claim was acknowledged while the identity file held another key",
        )
        self.assertEqual(env_values(self.env)["LOCA_MEMBERSHIP"], claimed)
        self.assertEqual(st["minted"], 1, "nothing more was minted")
        self.assertFalse(self.state.exists())
        self.assertIn("Profile -> Keys", second.stderr)
        no_secrets(self, first.stdout + first.stderr + second.stdout + second.stderr)

    def test_the_stored_mark_is_not_the_key(self):
        server, base = self.serve(ack_failures=1)
        self.assertEqual(self.run_claim(base).returncode, 6)
        key = env_values(self.env)["LOCA_MEMBERSHIP"]
        text = self.state.read_text()
        self.assertIn("stored=", text)
        self.assertNotIn(key, text)
        self.assertEqual(self.state.stat().st_mode & 0o777, 0o600)

    def test_an_identity_that_already_holds_a_key_claims_nothing(self):
        server, base = self.serve()
        self.assertEqual(self.run_claim(base).returncode, 0)
        again = self.run_claim(base)
        self.assertEqual(again.returncode, 0, again.stderr)
        self.assertEqual(server.state["minted"], 1)
        self.assertEqual(len(server.state["claims"]), 1)

    def test_already_delivered_delivery_ended_and_the_cap_say_what_to_do(self):
        cases = (
            ({"expired": True}, 7, "delivery ended"),
            ({}, 8, "already delivered"),
            ({"minted": 5}, 9, "five keys"),
        )
        for flags, code, words in cases:
            with self.subTest(code=code):
                for leftover in (self.state, self.env):
                    leftover.unlink(missing_ok=True)
                server, base = self.serve(**flags)
                if code in (7, 8):
                    # The set-up claimed earlier: its state file names it.
                    claim_id = "cl_" + "x" * 43
                    join_request.write_claim_state(
                        str(self.state), base, join_request.invite_management_id(INVITE), claim_id)
                    server.state["setups"][claim_id] = {
                        "key": "mb_earlier" + "e" * 54, "acked": code == 8}
                result = self.run_claim(base)
                self.assertEqual(result.returncode, code, result.stderr)
                self.assertIn(words, result.stderr)
                self.assertIn("Master", result.stderr)
                self.assertFalse(self.state.exists())
                self.assertNotIn("LOCA_MEMBERSHIP", env_values(self.env))
                self.assertEqual(server.state["minted"], flags.get("minted", 0))

    def test_an_older_server_that_ignores_the_claim_id_still_sets_up(self):
        server, base = self.serve(old_server=True)
        result = self.run_claim(base)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(env_values(self.env)["LOCA_MEMBERSHIP"], ORIGINAL)
        self.assertFalse(self.state.exists(), "no ACK route is not an error")
        no_secrets(self, result.stdout + result.stderr)

    def test_an_invite_of_another_identity_saves_nothing(self):
        server, base = self.serve(member_name="debug")
        result = self.run_claim(base)
        self.assertEqual(result.returncode, 4, result.stderr)
        self.assertFalse(self.env.exists())
        self.assertEqual(server.state["acks"], [])


class ConnectSetupClaimTest(unittest.TestCase):
    def test_setup_with_an_invite_files_the_claimed_key_and_the_invite(self):
        server, base = start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        with tempfile.TemporaryDirectory() as tmp:
            env_file = Path(tmp) / ".loca" / "worker.env"
            env = {k: v for k, v in os.environ.items()
                   if not k.startswith(("LOCA_", "ROOM_", "DAVET_"))}
            env.update({"HOME": tmp, "LOCA_ENV": str(env_file)})
            result = subprocess.run(
                [str(SKILL_DIR / "connect.sh"), "setup", base, NAME],
                env=env, input=INVITE + "\n", text=True, capture_output=True, timeout=60,
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            values = env_values(env_file)
            st = server.state
            (claim_id,) = set(st["claims"])
            self.assertEqual(values["LOCA_MEMBERSHIP"], st["setups"][claim_id]["key"])
            self.assertEqual(values["DAVET_parlor"], INVITE)
            self.assertEqual(values["LOCA_NAME"], NAME)
            self.assertEqual(st["acks"], [claim_id])
            self.assertFalse((Path(tmp) / ".loca" / "worker.claim").exists())
            self.assertEqual(env_file.stat().st_mode & 0o777, 0o600)
            self.assertIn("lobby membership bound", result.stdout)
            self.assertNotIn(values["LOCA_MEMBERSHIP"], result.stdout + result.stderr)
            self.assertNotIn(claim_id, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
