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
            rid = self.path.rsplit("/", 1)[-1]
            r = st["reqs"].get(rid)
            if not r or r["secret"] != self.headers.get("x-join-secret"):
                self._json({"error": "unknown"}, 404)
                return
            ready = r["status"] == "approved" and r["mb"] is not None and not r["acked"]
            self._json({"status": r["status"], "bootstrap_ready": ready})
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
            if action == "bootstrap":
                if not (good and r["status"] == "approved" and r["mb"] and not r["acked"]):
                    self._json({"error": "closed"}, 404)
                    return
                if st["bootstrap_malformed"]:
                    # A 201 that carries the mb_ credential under the WRONG key:
                    # join_request sees no "davet" and must log ONLY the status,
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
                 bootstrap_malformed=False):
    srv = ThreadingHTTPServer(("127.0.0.1", 0), LocaMock)
    srv.state = {"reqs": {}, "members": {}, "created": 0, "auto_approve": auto_approve,
                 "ack_forbidden": ack_forbidden, "deny": deny, "wrong_name": wrong_name,
                 "bootstrap_malformed": bootstrap_malformed}
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
            self.assertIn("re-admit", r.stderr)
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
            self.assertIn("belongs to 'impostor'", r.stderr)
            self.assertFalse((home / ".loca" / "env").exists())  # never persisted
        finally:
            srv.shutdown(); srv.server_close()

    def test_no_secret_in_stderr_when_bootstrap_response_is_malformed(self):
        # A malformed bootstrap 201 carries the mb_ credential under the wrong
        # key, so join_request finds no "davet" and takes the error path. It must
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


if __name__ == "__main__":
    unittest.main()
