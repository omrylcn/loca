#!/usr/bin/env python3
"""Agent-initiated join onboarding — every credential stays in-process.

Two credentials pass through here and NEITHER may ever reach a command line, an
environment block, stdout, a log, or a room:

* the per-request secret (`jrs_...`) — the requester's only handle on its pending
  request. It lives solely in a mode-600 state file and this process's memory.
* the issued membership (`mb_...`) — a Building credential. This helper writes it
  DIRECTLY into the atomic 600 identity env via the shared `credentials` API; it
  is never printed to stdout or handed back through a shell variable.

connect.sh calls `onboard` with only non-sensitive arguments — SERVER, the
identity NAME, the state-file path, and the resolved identity-env path.

    onboard <server> <name> <state_file> <env_path>

Flow: if the identity env already holds a membership that `/whoami`-verifies as
this name (kind=member), we are already onboarded — finalize the server window if
we still hold the secret, then stop. Otherwise create-or-resume the request, wait
for a Master to approve, re-fetch the issued mb_ (idempotent on the server until
ack), verify it via `/whoami` (kind AND name), persist it atomically, RE-VERIFY
the persisted env, and only then ACK to close the delivery window.

Exit codes (so connect.sh branches without parsing prose):
  0  onboarded — membership filed, verified, and acknowledged (or already so)
  2  usage / validation error
  3  denied, or a permanent refusal — do NOT retry
  5  UNRECOVERABLE: the request was finalized on the server but no valid local
     membership exists (an ACK'd credential with a missing/broken env). The
     credential cannot be re-fetched; the operator must re-admit this name.
  6  onboarded and PERSISTED, but the server finalize (ack) was not confirmed —
     nothing is lost; re-run to complete the finalize
  1  transient / verify failure — safe to retry
"""

import json
import os
import re
import sys
import time
import urllib.error
import urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import credentials  # noqa: E402  (shared atomic 600 identity-env writer)

POLL_INTERVAL = 3.0
HTTP_TIMEOUT = 10.0
MAX_POLL_SECONDS = float(os.environ.get("LOCA_JOIN_POLL_TIMEOUT", "0") or "0")

NAME_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$")
ORIGIN_RE = re.compile(r"^https?://([A-Za-z0-9._-]+|\[[0-9A-Fa-f:]+\])(:[0-9]+)?$")


def _emit(text):
    sys.stderr.write(text + "\n")
    sys.stderr.flush()


# Diagnostics carry NO interpolated STRING data. Only two emitters exist:
# `say` writes a fixed literal message, and `say_code` appends a single INTEGER
# (a status / exit / count). No identity name, request id, response body, or
# exception value is ever formatted into a diagnostic line, so a credential can
# never reach stderr even by a miswired call site — there is no data-flow path
# from any request/response value to the sink at all.
# (CodeQL: py/clear-text-logging-sensitive-data.)
def say(message):
    """Emit a fixed, literal diagnostic message — no interpolated data."""
    _emit(message)


def say_code(template, code):
    """Emit `template % <int>` — the ONLY dynamic value that reaches stderr is an
    integer, coerced here with int(); a string is never interpolated."""
    _emit(template % int(code))


def http(method, url, headers=None, body=None):
    """Return (status, text). status 0 signals a transport error (retryable)."""
    data = json.dumps(body).encode("utf-8") if body is not None else None
    req = urllib.request.Request(url, method=method, data=data)
    if data is not None:
        req.add_header("content-type", "application/json")
    for k, v in (headers or {}).items():
        req.add_header(k, v)
    try:
        with urllib.request.urlopen(req, timeout=HTTP_TIMEOUT) as r:
            return r.status, r.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode("utf-8", "replace")
    except Exception as e:  # noqa: BLE001 - transport failures are all retryable
        return 0, str(e)


def jbody(text):
    try:
        return json.loads(text)
    except Exception:  # noqa: BLE001
        return {}


def read_state(state_file):
    rid = secret = None
    try:
        with open(state_file, "r", encoding="utf-8") as f:
            for line in f:
                if line.startswith("request_id="):
                    rid = line[len("request_id="):].strip()
                elif line.startswith("request_secret="):
                    secret = line[len("request_secret="):].strip()
    except FileNotFoundError:
        pass
    return rid, secret


def write_state(state_file, rid, secret):
    """Atomically write the 600 state file; the secret never touches argv."""
    tmp = state_file + ".tmp"
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            f.write("request_id=%s\nrequest_secret=%s\n" % (rid, secret))
    except Exception:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise
    os.replace(tmp, state_file)
    os.chmod(state_file, 0o600)


def _discard(state_file):
    try:
        os.unlink(state_file)
    except OSError:
        pass


def read_env_membership(env_path):
    """Parse LOCA_MEMBERSHIP from an identity env file, else None."""
    try:
        with open(env_path, "r", encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if line.startswith("LOCA_MEMBERSHIP="):
                    v = line[len("LOCA_MEMBERSHIP="):].strip()
                    if len(v) >= 2 and v[0] == v[-1] and v[0] in "\"'":
                        v = v[1:-1]
                    return v or None
    except FileNotFoundError:
        pass
    return None


def whoami(server, token):
    code, text = http("GET", server + "/whoami", headers={"x-room-token": token})
    return code, jbody(text)


def membership_verifies(server, token, name):
    """True only if the token authenticates as kind=member with EXACTLY name."""
    if not token:
        return False
    code, who = whoami(server, token)
    return code == 200 and who.get("kind") == "member" and who.get("name") == name


def finalize(server, state_file):
    """ACK the request to close the server window. ONLY a verified 200 counts as
    finalized; a 404 (unknown request or wrong secret) is NOT proof and must not
    delete local state. Returns True when the window is confirmed closed."""
    rid, secret = read_state(state_file)
    if not (rid and secret):
        return True  # nothing left to finalize
    code, _ = http(
        "POST", "%s/join-requests/%s/ack" % (server, rid),
        headers={"x-join-secret": secret},
    )
    if code == 200:
        _discard(state_file)
        return True
    # 404 / transport / 5xx: leave the state file in place, unfinalized.
    return False


def do_onboard(server, name, state_file, env_path):
    if not ORIGIN_RE.match(server):
        say("server must be an http(s) origin without a path")
        return 2
    if not NAME_RE.match(name):
        say("name must be 1-64 ASCII letters, digits, dot, dash, or underscore")
        return 2

    # Step A — already onboarded? Verify the LOCAL env against /whoami (name +
    # kind=member) BEFORE trusting it. This is both the idempotent fast path and
    # the resume path: a re-run after a dropped ACK re-verifies and completes the
    # finalize instead of falsely reporting success on a missing/broken env.
    existing = read_env_membership(env_path)
    if membership_verifies(server, existing, name):
        if finalize(server, state_file):
            say("already onboarded: this identity holds a verified Lobby membership; finalized.")
            return 0
        # The membership is valid, but the server finalize (ack) is still pending
        # — report that (6), never claim finalization we could not confirm.
        say("already onboarded: this identity holds a verified Lobby membership; server "
            "finalize (ack) not confirmed, re-run to complete (nothing is lost).")
        return 6

    # Step B — run the request flow.
    rid, secret = read_state(state_file)
    if rid and secret:
        code, text = http(
            "GET", "%s/join-requests/%s" % (server, rid),
            headers={"x-join-secret": secret},
        )
        if code == 200:
            b = jbody(text)
            if b.get("status") == "denied":
                say("a Master denied the request.")
                _discard(state_file)
                return 3
            if b.get("status") == "approved" and b.get("bootstrap_ready") is False:
                # The window is closed (acked) yet Step A found no valid local
                # membership: the delivered credential is unrecoverable.
                say("request was finalized on the server but no valid local membership "
                    "exists — the credential cannot be re-fetched. Ask the operator to "
                    "re-admit this identity.")
                return 5
        elif code in (401, 404):
            rid = secret = None
            _discard(state_file)

    if not (rid and secret):
        code, text = http(
            "POST", "%s/join-requests" % server, body={"name": name, "kind": "agent"},
        )
        b = jbody(text)
        if code not in (200, 201) or not b.get("request_id") or not b.get("request_secret"):
            # Never echo the response body: a create response carries the
            # per-request secret, so only the (non-secret) status is safe to log.
            say_code("could not create the join request (status %s) — re-run to retry.", code)
            if code == 409:
                say("(that name already exists — pick another, or you may already be a member)")
                return 3
            return 1
        rid, secret = b["request_id"], b["request_secret"]
        write_state(state_file, rid, secret)  # secret lands ONLY in the 600 file

    say("requested to join — waiting for a Master to approve…")
    say("the Master approves you in the main app: People / BUILDING -> Join requests -> Approve.")

    started = time.monotonic()
    while True:
        code, text = http(
            "GET", "%s/join-requests/%s" % (server, rid),
            headers={"x-join-secret": secret},
        )
        if code == 200:
            b = jbody(text)
            if b.get("status") == "denied":
                say("a Master denied the request.")
                _discard(state_file)
                return 3
            if b.get("bootstrap_ready") is True:
                break
            if b.get("status") == "approved" and b.get("bootstrap_ready") is False:
                say("request was finalized on the server but no local membership exists — "
                    "ask the operator to re-admit this identity.")
                return 5
        elif code in (401, 404):
            say("the request is no longer on the server (expired or purged); re-run to start fresh.")
            _discard(state_file)
            return 1
        if MAX_POLL_SECONDS and (time.monotonic() - started) > MAX_POLL_SECONDS:
            say_code("still waiting for approval after %ds — re-run later to resume (no new request).",
                     int(MAX_POLL_SECONDS))
            return 1
        time.sleep(POLL_INTERVAL)

    # Re-fetch the mb_ (idempotent until ACK, so a crash here is resumable).
    code, text = http(
        "POST", "%s/join-requests/%s/bootstrap" % (server, rid),
        headers={"x-join-secret": secret},
    )
    if code == 404:
        say("bootstrap is closed (finalized) but no valid local membership exists — "
            "ask the operator to re-admit this identity.")
        return 5
    mb = jbody(text).get("davet")
    if code not in (200, 201) or not mb:
        # The bootstrap response body carries the mb_ membership credential —
        # log only the status, never the body.
        say_code("bootstrap did not return a membership (status %s) — re-run to retry.", code)
        return 1

    # Verify the credential BEFORE persisting: kind=member AND the exact name.
    code, who = whoami(server, mb)
    if code != 200 or who.get("kind") != "member":
        say("the issued membership did not verify (/whoami kind != member) — not saving; re-run to retry.")
        return 1
    if who.get("name") != name:
        say("credential belongs to a DIFFERENT identity than the one requested — not saving.")
        return 1

    # Persist DIRECTLY into the atomic 600 identity env — mb_ never leaves this
    # process (no stdout, no shell variable, no argv).
    try:
        credentials.update_env_values(
            env_path,
            {"ROOM_SERVER_URL": server, "LOCA_NAME": name, "LOCA_MEMBERSHIP": mb},
        )
    except Exception:  # noqa: BLE001
        # A static message only: the exception value could embed the mb_
        # membership we were writing, so nothing from it is logged.
        say("could not persist the identity env — re-run to retry.")
        return 1

    # Re-verify the PERSISTED env before finalizing — never ACK against an env we
    # have not read back and confirmed.
    if not membership_verifies(server, read_env_membership(env_path), name):
        say("membership was written but did not re-verify from the env; not finalizing — re-run to retry.")
        return 1

    if finalize(server, state_file):
        say("onboarded: membership filed, verified, and acknowledged.")
        return 0
    say("onboarded: membership filed and verified, but the server finalize (ack) "
        "was not confirmed — nothing is lost; re-run to complete it.")
    return 6


def main(argv):
    if len(argv) == 6 and argv[1] == "onboard":
        return do_onboard(argv[2], argv[3], argv[4], argv[5])
    say("usage: join_request.py onboard <server> <name> <state_file> <env_path>")
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
