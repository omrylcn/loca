"""Committed security matrix for self-service request-join onboarding.

Every test builds a HERMETIC subprocess environment (all LOCA_*/ROOM_*/DAVET_*
variables stripped) so results do not depend on the caller's shell — a real
operator running this suite already has a live identity in the environment.

A stdlib ThreadingHTTPServer stands in for room-server; join_request.py and
connect.sh are driven as subprocesses exactly as an agent would. The membership
(mb_) is never returned to the caller; it is written straight into the atomic
600 identity env, so the tests assert on the env file and on process argv, never
on stdout.
"""

import glob
import importlib.util
import json
import os
import re
import subprocess
import sys
import threading
import time
import unittest
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

SKILL_DIR = Path(__file__).resolve().parents[1]

_BOUND_PREFIXES = ("LOCA_", "ROOM_", "DAVET_")


def hermetic_env(home, **extra):
    """os.environ minus every loca credential var, with HOME pinned to a temp."""
    env = {k: v for k, v in os.environ.items()
           if not any(k.startswith(p) for p in _BOUND_PREFIXES)}
    env["HOME"] = str(home)
    env.update(extra)
    return env


def _seed_request_state(path, rid, secret):
    """Write a request-state fixture the SAME secure way production does
    (join_request.write_state): create it 0600 via os.open — so the per-request
    secret is never world-readable, not even briefly, and never goes through a
    high-level clear-text-storage sink like Path.write_text()."""
    fd = os.open(str(path), os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as f:
        f.write("request_id=%s\nrequest_secret=%s\n" % (rid, secret))


def reap(proc):
    """Terminate (if alive), wait (reap the PID), and close every pipe, so the
    suite leaves behind no ghost process or open file descriptor."""
    try:
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=5)
        else:
            proc.wait(timeout=5)
    except Exception:  # noqa: BLE001
        pass
    finally:
        for pipe in (proc.stdout, proc.stderr, proc.stdin):
            if pipe is not None:
                try:
                    pipe.close()
                except Exception:  # noqa: BLE001
                    pass


class LocaMock(BaseHTTPRequestHandler):
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
        if self.path == "/health":
            self._json({"ok": True})
            return
        if self.path == "/whoami":
            name = st["members"].get(self.headers.get("x-room-token"))
            if name:
                self._json({"kind": "member", "name": name})
            else:
                self._json({"error": "unauthorized"}, 401)
            return
        if self.path.startswith("/join-requests/"):
            if "poll" in st["unavailable"]:
                self._json({"error": "storage"}, 503)
                return
            rid = self.path.rsplit("/", 1)[-1]
            r = st["reqs"].get(rid)
            if not r or r["secret"] != self.headers.get("x-join-secret"):
                self._json({"error": "unknown"}, 404)
                return
            if st["expire"] and r["status"] == "approved" and not r["acked"]:
                # The server's delivery window closed: the copy is gone.
                r["mb"] = None
                r["expired"] = True
            ready = r["status"] == "approved" and r["mb"] is not None and not r["acked"]
            self._json({"status": r["status"], "bootstrap_ready": ready,
                        "bootstrap_expired": bool(r.get("expired")) and not r["acked"]})
            return
        self._json({"error": "not found"}, 404)

    def _approve(self, rec, name):
        rec["status"] = "approved"
        who = "impostor" if self.server.state["wrong_name"] else name
        rec["mb"] = "mb_" + name
        self.server.state["members"]["mb_" + name] = who

    def do_POST(self):
        st = self.server.state
        length = int(self.headers.get("content-length", 0) or 0)
        raw = self.rfile.read(length) if length else b""
        if self.path == "/join-requests":
            body = json.loads(raw or b"{}")
            st["created"] += 1
            rid, sec = "jr_%d" % st["created"], "jrs_%d" % st["created"]
            rec = {"name": body["name"], "secret": sec, "status": "pending",
                   "mb": None, "acked": False}
            if st["deny"]:
                rec["status"] = "denied"
            elif st["auto_approve"]:
                self._approve(rec, body["name"])
            st["reqs"][rid] = rec
            self._json({"request_id": rid, "request_secret": sec}, 201)
            return
        parts = self.path.strip("/").split("/")
        if len(parts) == 3 and parts[0] == "join-requests":
            rid, action = parts[1], parts[2]
            r = st["reqs"].get(rid)
            good = r and r["secret"] == self.headers.get("x-join-secret")
            if action in st["unavailable"]:
                self._json({"error": "storage"}, 503)
                return
            if action == "bootstrap":
                if good and r["status"] == "approved" and not r["acked"] and (
                        r.get("expired") or st["expire_on_bootstrap"]):
                    r["mb"] = None
                    r["expired"] = True
                    self._json({"error": "gone"}, 410)
                    return
                if not (good and r["status"] == "approved" and r["mb"] and not r["acked"]):
                    self._json({"error": "closed"}, 404)
                    return
                if st["bootstrap_malformed"]:
                    # A 201 that carries the mb_ credential under the WRONG key:
                    # join_request sees no "invite" and must log ONLY the status,
                    # never this body (which holds the credential).
                    self._json({"unexpected": r["mb"]}, 201)
                    return
                self._json({"davet": r["mb"]}, 201)
                return
            if action == "ack":
                if not (good and r["status"] == "approved"):
                    self._json({"error": "unknown"}, 404)
                    return
                if st["ack_forbidden"]:
                    self._json({"error": "forced"}, 404)
                    return
                r["acked"] = True
                r["mb"] = None
                self._json({"acknowledged": True}, 200)
                return
        self._json({"error": "not found"}, 404)


def start_server(auto_approve=True, ack_forbidden=False, deny=False, wrong_name=False,
                 bootstrap_malformed=False, expire=False, expire_on_bootstrap=False,
                 unavailable=()):
    srv = ThreadingHTTPServer(("127.0.0.1", 0), LocaMock)
    srv.state = {"reqs": {}, "members": {}, "created": 0, "auto_approve": auto_approve,
                 "ack_forbidden": ack_forbidden, "deny": deny, "wrong_name": wrong_name,
                 "bootstrap_malformed": bootstrap_malformed, "expire": expire,
                 "expire_on_bootstrap": expire_on_bootstrap,
                 "unavailable": set(unavailable)}
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv, "http://127.0.0.1:%d" % srv.server_address[1]


def onboard_cmd(base, name, home):
    os.makedirs(os.path.join(home, ".loca"), exist_ok=True)
    return [sys.executable, str(SKILL_DIR / "join_request.py"), "onboard", base, name,
            os.path.join(home, ".loca", "%s.request" % name),
            os.path.join(home, ".loca", "env")]


class OnboardTest(unittest.TestCase):
    def _onboard(self, base, name, home, timeout="8"):
        return subprocess.run(
            onboard_cmd(base, name, home),
            env=hermetic_env(home, LOCA_JOIN_POLL_TIMEOUT=timeout),
            capture_output=True, text=True, timeout=40,
        )

    def _tmp(self):
        d = Path(self.enterContext(_TmpDir()))
        (d / ".loca").mkdir()
        return d

    def test_happy_path_writes_env_without_mb_on_stdout(self):
        srv, base = start_server()
        try:
            home = self._tmp()
            r = self._onboard(base, "alice", home)
            self.assertEqual(r.returncode, 0, r.stderr)
            self.assertNotIn("mb_", r.stdout)  # blocker 1: no membership on stdout
            envp = home / ".loca" / "env"
            self.assertIn("LOCA_MEMBERSHIP=mb_alice", envp.read_text())
            self.assertEqual(oct(envp.stat().st_mode & 0o777), "0o600")
            self.assertFalse((home / ".loca" / "alice.request").exists())
        finally:
            srv.shutdown(); srv.server_close()

    def test_ack_404_keeps_state_and_does_not_claim_finalized(self):
        srv, base = start_server(ack_forbidden=True)
        try:
            home = self._tmp()
            r = self._onboard(base, "bob", home)
            self.assertEqual(r.returncode, 6, r.stderr)  # onboarded, finalize pending
            self.assertIn("LOCA_MEMBERSHIP=mb_bob", (home / ".loca" / "env").read_text())
            self.assertTrue((home / ".loca" / "bob.request").exists())  # state kept on 404
        finally:
            srv.shutdown(); srv.server_close()

    def test_ack_drop_then_resume_completes(self):
        srv, base = start_server(ack_forbidden=True)
        try:
            home = self._tmp()
            first = self._onboard(base, "carol", home)
            self.assertEqual(first.returncode, 6, first.stderr)
            self.assertTrue((home / ".loca" / "carol.request").exists())
            srv.state["ack_forbidden"] = False  # the ack endpoint recovers
            again = self._onboard(base, "carol", home)
            self.assertEqual(again.returncode, 0, again.stderr)
            self.assertIn("already onboarded", again.stderr)
            self.assertFalse((home / ".loca" / "carol.request").exists())  # now finalized
        finally:
            srv.shutdown(); srv.server_close()

    def test_already_onboarded_verifies_local_env(self):
        srv, base = start_server()
        try:
            home = self._tmp()
            self.assertEqual(self._onboard(base, "dave", home).returncode, 0)
            again = self._onboard(base, "dave", home)
            self.assertEqual(again.returncode, 0, again.stderr)
            self.assertIn("already onboarded", again.stderr)
        finally:
            srv.shutdown(); srv.server_close()

    def test_acked_but_no_local_env_is_unrecoverable(self):
        srv, base = start_server()
        try:
            home = self._tmp()

            def post(path, headers=None, body=None):
                data = json.dumps(body).encode() if body is not None else None
                req = urllib.request.Request(base + path, method="POST", data=data)
                if data:
                    req.add_header("content-type", "application/json")
                for k, v in (headers or {}).items():
                    req.add_header(k, v)
                return urllib.request.urlopen(req, timeout=5)

            created = json.load(post("/join-requests", body={"name": "erin", "kind": "agent"}))
            rid, sec = created["request_id"], created["request_secret"]
            post("/join-requests/%s/bootstrap" % rid, headers={"x-join-secret": sec})
            post("/join-requests/%s/ack" % rid, headers={"x-join-secret": sec})
            req_path = home / ".loca" / "erin.request"
            _seed_request_state(req_path, rid, sec)  # 0600 from creation, no cleartext sink

            r = self._onboard(base, "erin", home)
            self.assertEqual(r.returncode, 5, r.stderr)
            # Admitting the name again hands out no key; only an invite does.
            self.assertIn("invite for this same name", r.stderr)
            self.assertNotIn("re-admit", r.stderr)
            self.assertFalse((home / ".loca" / "env").exists())
        finally:
            srv.shutdown(); srv.server_close()

    def test_denied_request_is_reported_and_state_discarded(self):
        srv, base = start_server(deny=True)
        try:
            home = self._tmp()
            r = self._onboard(base, "frank", home)
            self.assertEqual(r.returncode, 3, r.stderr)
            self.assertIn("denied", r.stderr.lower())
            self.assertFalse((home / ".loca" / "frank.request").exists())
            self.assertFalse((home / ".loca" / "env").exists())
        finally:
            srv.shutdown(); srv.server_close()

    def test_pending_re_run_resumes_the_same_request_no_duplicate(self):
        srv, base = start_server(auto_approve=False)  # stays pending
        try:
            home = self._tmp()
            first = self._onboard(base, "grace", home, timeout="1")
            self.assertEqual(first.returncode, 1)  # timed out waiting for approval
            self.assertTrue((home / ".loca" / "grace.request").exists())
            second = self._onboard(base, "grace", home, timeout="1")
            self.assertEqual(second.returncode, 1)
            # Both runs reused ONE request — no duplicate was created.
            self.assertEqual(srv.state["created"], 1)
        finally:
            srv.shutdown(); srv.server_close()

    def test_credential_for_a_different_name_is_rejected(self):
        srv, base = start_server(wrong_name=True)  # mb_ /whoami-s as 'impostor'
        try:
            home = self._tmp()
            r = self._onboard(base, "heidi", home)
            self.assertEqual(r.returncode, 1, r.stderr)
            self.assertIn("belongs to a DIFFERENT identity", r.stderr)
            self.assertFalse((home / ".loca" / "env").exists())  # never persisted
        finally:
            srv.shutdown(); srv.server_close()

    def test_no_secret_in_stderr_when_bootstrap_response_is_malformed(self):
        # A malformed bootstrap 201 carries the mb_ credential under the wrong
        # key, so join_request finds no "invite" and takes the error path. It must
        # log ONLY the status — the credential must never reach stderr.
        srv, base = start_server(bootstrap_malformed=True)
        try:
            home = self._tmp()
            r = self._onboard(base, "mallory", home)
            self.assertEqual(r.returncode, 1, r.stderr)
            self.assertNotIn("mb_mallory", r.stderr)
            self.assertNotIn("mb_", r.stderr)  # no credential of any name leaks
            self.assertFalse((home / ".loca" / "env").exists())  # nothing persisted
        finally:
            srv.shutdown(); srv.server_close()

    def test_no_secret_in_any_process_cmdline_or_environ_during_onboard(self):
        # Live /proc scan: while onboard is polling for approval, the per-request
        # secret must appear in NO process's argv or environment block.
        srv, base = start_server(auto_approve=False)
        try:
            home = self._tmp()
            proc = subprocess.Popen(
                onboard_cmd(base, "ivan", home),
                env=hermetic_env(home, LOCA_JOIN_POLL_TIMEOUT="6"),
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, text=True,
            )
            self.addCleanup(reap, proc)
            try:
                req = home / ".loca" / "ivan.request"
                for _ in range(60):  # wait for the request to be created
                    if req.exists() and "request_secret=" in req.read_text():
                        break
                    time.sleep(0.1)
                secret = next(l.split("=", 1)[1].strip()
                              for l in req.read_text().splitlines()
                              if l.startswith("request_secret="))
                self.assertTrue(secret.startswith("jrs_"))
                # Scan every process's cmdline AND environ for the secret value.
                for kind in ("cmdline", "environ"):
                    for path in glob.glob("/proc/[0-9]*/" + kind):
                        try:
                            with open(path, "rb") as f:
                                blob = f.read()
                        except OSError:
                            continue
                        self.assertNotIn(secret.encode(), blob,
                                         "secret leaked into %s" % path)
                # Approve so onboard can finish cleanly.
                srv.state["reqs"]["jr_1"]["status"] = "approved"
                srv.state["reqs"]["jr_1"]["mb"] = "mb_ivan"
                srv.state["members"]["mb_ivan"] = "ivan"
                proc.wait(timeout=20)
                self.assertEqual(proc.returncode, 0)
            finally:
                if proc.poll() is None:
                    proc.kill()
        finally:
            srv.shutdown(); srv.server_close()


class DeliveryWindowTest(unittest.TestCase):
    """The server keeps an approved key for collection for 24 hours after the
    approval. After that the poll says `bootstrap_expired` and a collection
    answers 410: the helper must stop and say what a person can do, not poll
    for ever, report success, or claim the request was finalized."""

    def _onboard(self, base, name, home, timeout="8"):
        return subprocess.run(
            onboard_cmd(base, name, home),
            env=hermetic_env(home, LOCA_JOIN_POLL_TIMEOUT=timeout),
            capture_output=True, text=True, timeout=40,
        )

    def _tmp(self):
        d = Path(self.enterContext(_TmpDir()))
        (d / ".loca").mkdir()
        return d

    def _assert_expired_message(self, r):
        self.assertEqual(r.returncode, 7, r.stderr)
        self.assertIn("24 hours", r.stderr)
        self.assertIn("ask a Master", r.stderr)
        self.assertIn("membership was kept", r.stderr)
        self.assertNotIn("finalized", r.stderr)
        # A membership for an existing name carries no key; only an invite
        # (and setup with it) leads back.
        self.assertIn("invite for this same name", r.stderr)
        self.assertNotIn("a membership or", r.stderr)

    def test_an_expired_window_seen_by_the_poll_stops_with_its_own_exit(self):
        srv, base = start_server(expire=True)
        try:
            home = self._tmp()
            r = self._onboard(base, "late", home)
            self._assert_expired_message(r)
            self.assertFalse((home / ".loca" / "env").exists())
            # The request is kept, so a re-run says the same instead of filing
            # a new request the server would refuse.
            self.assertTrue((home / ".loca" / "late.request").exists())
            again = self._onboard(base, "late", home)
            self._assert_expired_message(again)
            self.assertEqual(srv.state["created"], 1)
        finally:
            srv.shutdown(); srv.server_close()

    def test_a_collection_answered_410_stops_with_its_own_exit(self):
        # The poll said ready a moment before the deadline; the collection
        # came after it.
        srv, base = start_server(expire_on_bootstrap=True)
        try:
            home = self._tmp()
            r = self._onboard(base, "racer", home)
            self._assert_expired_message(r)
            self.assertFalse((home / ".loca" / "env").exists())
        finally:
            srv.shutdown(); srv.server_close()

    def test_connect_request_join_tells_the_person_what_to_do(self):
        srv, base = start_server(expire=True)
        try:
            home = self._tmp()
            proc = subprocess.run(
                [str(SKILL_DIR / "connect.sh"), "request-join", base, "late"],
                env=hermetic_env(home, LOCA_JOIN_POLL_TIMEOUT="8"),
                capture_output=True, text=True, timeout=40,
            )
            self.assertEqual(proc.returncode, 1, proc.stderr)
            self.assertIn("not collected within", proc.stderr)
            self.assertIn("24 hours", proc.stderr)
            self.assertIn("ask a Master for an invite for", proc.stderr)
            self.assertNotIn("a membership or a davet", proc.stderr)
            self.assertNotIn("RESULT: LOBBY", proc.stdout)
        finally:
            srv.shutdown(); srv.server_close()

    def test_a_kept_key_whose_ack_was_lost_finishes_after_the_window(self):
        # Collected and stored in time, ACK lost; the window then closes. The
        # re-run finds the stored key valid and its late ACK is accepted.
        srv, base = start_server(ack_forbidden=True)
        try:
            home = self._tmp()
            self.assertEqual(self._onboard(base, "kept", home).returncode, 6)
            srv.state["expire"] = True
            srv.state["ack_forbidden"] = False
            again = self._onboard(base, "kept", home)
            self.assertEqual(again.returncode, 0, again.stderr)
            self.assertIn("already onboarded", again.stderr)
            self.assertFalse((home / ".loca" / "kept.request").exists())
        finally:
            srv.shutdown(); srv.server_close()


class StorageUnavailableTest(unittest.TestCase):
    """A 503 from the server means it could not read the request just now.
    The helper keeps its request (no new one is filed) and says re-run."""

    def _onboard(self, base, name, home, timeout="2"):
        return subprocess.run(
            onboard_cmd(base, name, home),
            env=hermetic_env(home, LOCA_JOIN_POLL_TIMEOUT=timeout),
            capture_output=True, text=True, timeout=40,
        )

    def _tmp(self):
        d = Path(self.enterContext(_TmpDir()))
        (d / ".loca").mkdir()
        return d

    def _state(self, home, name):
        return (home / ".loca" / ("%s.request" % name)).read_text()

    def test_a_503_poll_keeps_the_request_and_files_no_new_one(self):
        srv, base = start_server(auto_approve=False)
        try:
            home = self._tmp()
            self.assertEqual(self._onboard(base, "steady", home, timeout="1").returncode, 1)
            before = self._state(home, "steady")
            srv.state["unavailable"] = {"poll"}
            r = self._onboard(base, "steady", home)
            self.assertEqual(r.returncode, 1, r.stderr)
            self.assertEqual(self._state(home, "steady"), before)
            self.assertEqual(srv.state["created"], 1)
            # The server recovers and approves: the same request completes.
            srv.state["unavailable"] = set()
            req = srv.state["reqs"]["jr_1"]
            req["status"], req["mb"] = "approved", "mb_steady"
            srv.state["members"]["mb_steady"] = "steady"
            done = self._onboard(base, "steady", home, timeout="8")
            self.assertEqual(done.returncode, 0, done.stderr)
            self.assertEqual(srv.state["created"], 1)
        finally:
            srv.shutdown(); srv.server_close()

    def test_a_503_collection_says_re_run_and_keeps_the_request(self):
        srv, base = start_server(unavailable={"bootstrap"})
        try:
            home = self._tmp()
            r = self._onboard(base, "patient", home)
            self.assertEqual(r.returncode, 1, r.stderr)
            self.assertIn("re-run to retry", r.stderr)
            self.assertNotIn("invite for this same name", r.stderr)
            self.assertTrue((home / ".loca" / "patient.request").exists())
            srv.state["unavailable"] = set()
            again = self._onboard(base, "patient", home, timeout="8")
            self.assertEqual(again.returncode, 0, again.stderr)
            self.assertEqual(srv.state["created"], 1)
        finally:
            srv.shutdown(); srv.server_close()

    def test_a_503_ack_keeps_the_request_and_a_re_run_finishes_it(self):
        srv, base = start_server(unavailable={"ack"})
        try:
            home = self._tmp()
            r = self._onboard(base, "acker", home)
            self.assertEqual(r.returncode, 6, r.stderr)
            self.assertIn("LOCA_MEMBERSHIP=mb_acker", (home / ".loca" / "env").read_text())
            self.assertTrue((home / ".loca" / "acker.request").exists())
            srv.state["unavailable"] = set()
            again = self._onboard(base, "acker", home)
            self.assertEqual(again.returncode, 0, again.stderr)
            self.assertFalse((home / ".loca" / "acker.request").exists())
        finally:
            srv.shutdown(); srv.server_close()


class ConnectRequestJoinTest(unittest.TestCase):
    """Drive connect.sh request-join so the DETERMINISTIC per-identity env path
    (not the shared default) is exercised, and two agents cannot collide."""

    def _request_join(self, base, name, home):
        proc = subprocess.Popen(
            [str(SKILL_DIR / "connect.sh"), "request-join", base, name],
            env=hermetic_env(home, LOCA_JOIN_POLL_TIMEOUT="10"),
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        )
        self.addCleanup(reap, proc)
        return proc

    def test_two_agents_onboard_into_separate_files_without_collision(self):
        srv, base = start_server(auto_approve=True)
        try:
            home = Path(self.enterContext(_TmpDir()))
            (home / ".loca").mkdir()
            a = self._request_join(base, "one", home)
            b = self._request_join(base, "two", home)
            # communicate() waits AND drains+closes the pipes (no fd leak).
            _, err_a = a.communicate(timeout=40)
            _, err_b = b.communicate(timeout=40)
            self.assertEqual(a.returncode, 0, err_a)
            self.assertEqual(b.returncode, 0, err_b)
            # Each identity got its OWN deterministic file — no shared ~/.loca/env.
            self.assertFalse((home / ".loca" / "env").exists())
            self.assertIn("LOCA_MEMBERSHIP=mb_one", (home / ".loca" / "one.env").read_text())
            self.assertIn("LOCA_MEMBERSHIP=mb_two", (home / ".loca" / "two.env").read_text())
            self.assertIn("LOCA_NAME=one", (home / ".loca" / "one.env").read_text())
            self.assertIn("LOCA_NAME=two", (home / ".loca" / "two.env").read_text())
        finally:
            srv.shutdown(); srv.server_close()


class ConnectRequestJoinExitTest(unittest.TestCase):
    """connect.sh runs under `set -e`; the helper's exit code must reach the
    case that explains it instead of ending the script silently."""

    def test_a_pending_finalize_still_reports_the_lobby_next_step(self):
        srv, base = start_server(ack_forbidden=True)
        try:
            home = Path(self.enterContext(_TmpDir()))
            (home / ".loca").mkdir()
            proc = subprocess.run(
                [str(SKILL_DIR / "connect.sh"), "request-join", base, "pending"],
                env=hermetic_env(home, LOCA_JOIN_POLL_TIMEOUT="8"),
                capture_output=True, text=True, timeout=40,
            )
            self.assertEqual(proc.returncode, 0, proc.stderr)
            self.assertIn("RESULT: LOBBY", proc.stdout)
            self.assertIn("finalize (ack) is still pending", proc.stdout)
        finally:
            srv.shutdown(); srv.server_close()

    def test_success_names_the_identity_file_and_what_an_agent_can_check(self):
        # A Lobby agent can read no roster; it can check doctor's OK line and
        # its listener's log. The identity lives in a side file, so the
        # export line setup prints is printed here too.
        srv, base = start_server()
        try:
            home = Path(self.enterContext(_TmpDir()))
            (home / ".loca").mkdir()
            proc = subprocess.run(
                [str(SKILL_DIR / "connect.sh"), "request-join", base, "fresh"],
                env=hermetic_env(home, LOCA_JOIN_POLL_TIMEOUT="8"),
                capture_output=True, text=True, timeout=40,
            )
            self.assertEqual(proc.returncode, 0, proc.stderr)
            self.assertIn("RESULT: LOBBY", proc.stdout)
            self.assertIn("export LOCA_ENV=%s" % (home / ".loca" / "fresh.env"), proc.stdout)
            self.assertIn("OK: 'fresh' has a live listener", proc.stdout)
            self.assertIn("[lobby] connected", proc.stdout)
            self.assertNotIn("roster ONLINE", proc.stdout)
            self.assertNotIn("roster shows", proc.stdout)
        finally:
            srv.shutdown(); srv.server_close()

    def test_status_without_an_identity_points_at_request_join(self):
        home = Path(self.enterContext(_TmpDir()))
        proc = subprocess.run(
            [str(SKILL_DIR / "connect.sh"), "status", "http://127.0.0.1:9", "newcomer"],
            env=hermetic_env(home), capture_output=True, text=True, timeout=20,
        )
        self.assertEqual(proc.returncode, 3, proc.stderr)
        self.assertIn("not onboarded", proc.stderr)
        self.assertIn("connect.sh request-join http://127.0.0.1:9 newcomer", proc.stderr)


class CredentialsOwnershipTest(unittest.TestCase):
    def setUp(self):
        if str(SKILL_DIR) not in sys.path:
            sys.path.insert(0, str(SKILL_DIR))  # so credentials.py finds portable_lock
        spec = importlib.util.spec_from_file_location(
            "credentials", SKILL_DIR / "credentials.py")
        self.cred = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.cred)

    def test_update_env_values_refuses_to_overwrite_another_identity(self):
        home = Path(self.enterContext(_TmpDir()))
        envp = home / "one.env"
        self.cred.update_env_values(str(envp), {"LOCA_NAME": "one", "LOCA_MEMBERSHIP": "mb_one"})
        with self.assertRaises(self.cred.CredentialError):
            self.cred.update_env_values(str(envp), {"LOCA_NAME": "two", "LOCA_MEMBERSHIP": "mb_two"})
        # The original identity is untouched.
        self.assertIn("LOCA_NAME=one", envp.read_text())
        self.assertIn("mb_one", envp.read_text())
        # Re-writing the SAME identity is allowed (session renewal etc.).
        self.cred.update_env_values(str(envp), {"LOCA_NAME": "one", "LOCA_SESSION": "st_x"})
        self.assertIn("LOCA_SESSION=st_x", envp.read_text())


class RefusedNameTest(unittest.TestCase):
    """Against the real room-server: a name no door admits (`all`) is refused
    with 400 when the request is filed. Re-running cannot change that, so
    request-join must stop as a permanent refusal, not say "re-run"."""

    MASTER = "refused-name-e2e-master"

    def setUp(self):
        from test_listener_backfill_e2e import SERVER_BIN, free_port

        if not SERVER_BIN.is_file():
            subprocess.run(["cargo", "build", "-q", "-p", "server"],
                           cwd=SKILL_DIR.parents[1], check=True, timeout=600)
        root = Path(self.enterContext(_TmpDir()))
        port = free_port()
        self.base = "http://127.0.0.1:%d" % port
        env = {k: v for k, v in os.environ.items() if k != "ROOM_TOKEN"}
        env.update(PORT=str(port), BIND_ADDR="127.0.0.1",
                   DB_PATH=str(root / "loca.sqlite3"), ADMIN_TOKEN=self.MASTER,
                   REQUIRE_INVITE="1", REQUIRE_SESSIONS="1", RUST_LOG="warn")
        log = open(root / "server.log", "ab")
        self.addCleanup(log.close)
        server = subprocess.Popen([str(SERVER_BIN)], cwd=SKILL_DIR.parents[1],
                                  env=env, stdout=log, stderr=subprocess.STDOUT)
        self.addCleanup(reap, server)
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            try:
                with urllib.request.urlopen(self.base + "/health", timeout=2):
                    break
            except OSError:
                time.sleep(0.05)
        else:
            self.fail("room-server did not become healthy")
        self.home = root / "home"
        (self.home / ".loca").mkdir(parents=True)

    def test_request_join_stops_on_a_name_the_server_refuses(self):
        proc = subprocess.run(
            [str(SKILL_DIR / "connect.sh"), "request-join", self.base, "all"],
            env=hermetic_env(self.home, LOCA_JOIN_POLL_TIMEOUT="5"),
            capture_output=True, text=True, timeout=40,
        )
        self.assertEqual(proc.returncode, 3, proc.stderr)
        self.assertIn("Choose another name", proc.stderr)
        self.assertNotIn("re-run to retry", proc.stderr)
        self.assertNotIn("RESULT: LOBBY", proc.stdout)
        self.assertEqual(list((self.home / ".loca").glob("*.request")), [])
        self.assertFalse((self.home / ".loca" / "all.env").exists())


class DocParityTest(unittest.TestCase):
    def test_getting_started_documents_request_join_for_both_runtimes(self):
        doc = (SKILL_DIR.parents[1] / "docs" / "getting-started.md").read_text()
        self.assertIn("~/.claude/skills/loca/connect.sh request-join", doc)
        self.assertIn("~/.codex/skills/loca/connect.sh request-join", doc)
        self.assertIn("LOBBY", doc)
        self.assertIn("monitor setup required", doc)


# --- minimal enterContext(TemporaryDirectory) shim for older runners ---------
import tempfile  # noqa: E402


class _TmpDir:
    def __enter__(self):
        self._d = tempfile.TemporaryDirectory()
        return self._d.name

    def __exit__(self, *exc):
        self._d.cleanup()


if not hasattr(unittest.TestCase, "enterContext"):
    def _enter_context(self, cm):
        value = cm.__enter__()
        self.addCleanup(cm.__exit__, None, None, None)
        return value
    unittest.TestCase.enterContext = _enter_context


class DiagnosticEmitTest(unittest.TestCase):
    """join_request.py logs diagnostics with NO interpolated STRING data: the
    only emitters are say(literal) and say_code(template, int). No identity name,
    request id, response body, or exception value is ever formatted into a line,
    so there is no data-flow path from any request/response value to stderr and a
    credential can never be logged."""

    def setUp(self):
        if str(SKILL_DIR) not in sys.path:
            sys.path.insert(0, str(SKILL_DIR))
        spec = importlib.util.spec_from_file_location(
            "join_request", SKILL_DIR / "join_request.py")
        self.jr = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.jr)

    def test_only_literal_and_integer_emitters_exist(self):
        src = (SKILL_DIR / "join_request.py").read_text(encoding="utf-8")
        # The generic and the string-interpolating emitters are gone.
        for gone in ("def err(", "def say_name(", "def say_pair(", "def _safe("):
            self.assertNotIn(gone, src, "%s must not exist" % gone)
        # Exactly one stderr write, inside the single private sink.
        self.assertEqual(src.count("sys.stderr.write"), 1)
        # No say(...) call interpolates anything: every say() argument is a plain
        # string literal (contains no '%s'/'%d' format placeholder).
        say_calls = re.findall(r"(?<!_)\bsay\(([^\n]*)", src)
        for arg in say_calls:
            self.assertNotIn("%", arg, "say() must take a literal, not a template: %s" % arg)

    def test_say_code_coerces_to_int_and_rejects_strings(self):
        import contextlib
        import io
        jr = self.jr
        buf = io.StringIO()
        with contextlib.redirect_stderr(buf):
            jr.say_code("bootstrap did not return a membership (status %s)", 404)
            jr.say("credential belongs to a DIFFERENT identity than requested — not saving.")
        out = buf.getvalue()
        self.assertIn("status 404", out)
        self.assertNotIn("mb_", out)
        # A non-integer value can never be smuggled through say_code.
        with self.assertRaises((TypeError, ValueError)):
            jr.say_code("x %s", '{"davet": "mb_LEAK"}')


if __name__ == "__main__":
    unittest.main()
