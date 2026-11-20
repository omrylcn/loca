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
  3  denied, or a permanent refusal (a name taken or refused) — do NOT retry
  5  UNRECOVERABLE: the request was finalized on the server but no valid local
     membership exists (an ACK'd credential with a missing/broken env). The
     credential cannot be re-fetched. Admitting the name again hands out no
     key; a Master gives this name an invite, and `setup` with it claims a key.
  6  onboarded and PERSISTED, but the server finalize (ack) was not confirmed —
     nothing is lost; re-run to complete the finalize
  7  EXPIRED: the request was approved, but its key was not collected within
     the server's delivery window (24 hours from the approval), so the server
     no longer holds it. The membership was kept, so a new request for the name
     is refused; a Master gives this name an invite, and `setup` with it claims a
     key.
  1  transient / verify failure (including a server that could not read the
     request just now, 503) — safe to retry; the request is kept

The second door, an invite handed over by the Master, uses `claim`:

    claim <server> <name> <state_file> <env_path>      (the invite on stdin)

An invite proves which member holds it; claiming with it gets this set-up a
membership key of its own. The set-up names itself with a random claim id,
kept in a mode-600 state file (~/.loca/<name>.claim) that is written BEFORE
the first claim, so a re-run after a lost answer or a crash asks for the SAME
key instead of minting another. The key is verified via `/whoami`, written
straight into the atomic 600 identity env, re-verified from the env, and only
then acknowledged, which ends the server's delivery copy; the state file goes
when the set-up is finished. An identity whose env already holds a verified
membership claims nothing. The acknowledgement is sent only while the env
holds the key this set-up claimed (the state file records a one-way mark of
it once it is filed): a set-up whose claim is still open while the env holds
another working key finishes the claim first, so the server never drops its
copy of a key that nobody has. The claim id never reaches argv, stdout or a log.

Exit codes of `claim`:
  0  key filed, verified and acknowledged (or the identity already held one,
     or an older server that has no ACK)
  1  transient — re-run; the same set-up resumes
  3  the invite was not accepted (revoked, mistyped, or a membership key)
  4  the invite belongs to a DIFFERENT identity — nothing saved
  6  key filed and verified, but the ACK was not confirmed — re-run to finish
  7  delivery ended: the key was not confirmed within 24 hours of the claim
  8  already delivered: this set-up confirmed its key earlier, and this env
     does not hold it
  9  this invite has already set up five keys
"""

import hashlib
import json
import os
import re
import secrets
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


# Admitting an existing name again hands out no key (the server answers
# `already`); only an invite leads back: `setup` with it claims a key.
ASK_FOR_INVITE = ("Ask a Master for an invite for this same name, then run setup with it; "
                 "setup claims a key of this agent's own.")


def say_expired():
    say("the request was approved, but its key was not collected within 24 hours of the "
        "approval, so the server no longer holds it. The membership was kept: ask a Master "
        "for an invite for this same name, then run setup with it; setup claims a key of "
        "this agent's own.")


def do_onboard(server, name, state_file, env_path):
    if not ORIGIN_RE.match(server):
        say("server must be an http(s) origin without a path")
        return 2
    if not NAME_RE.match(name):
        say("name must be 1-64 ASCII letters, digits, dot, dash, or underscore, starting with a letter or digit")
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
                if b.get("bootstrap_expired") is True:
                    say_expired()
                    return 7
                # The window is closed (acked) yet Step A found no valid local
                # membership: the delivered credential is unrecoverable.
                say("request was finalized on the server but no valid local membership "
                    "exists — the credential cannot be re-fetched. " + ASK_FOR_INVITE)
                return 5
        elif code in (401, 404):
            rid = secret = None
            _discard(state_file)

    if not (rid and secret):
        code, text = http(
            "POST", "%s/join-requests" % server, body={"name": name, "kind": "agent"},
        )
        b = jbody(text)
        if code == 400:
            # The name itself is refused (an address word such as `all`):
            # asking again with the same name can never succeed.
            say("the server refused this name: it is not a name a member may wear (`all` "
                "calls everyone). Choose another name; re-running with this one will not help.")
            return 3
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
                if b.get("bootstrap_expired") is True:
                    say_expired()
                    return 7
                say("request was finalized on the server but no local membership exists. "
                    + ASK_FOR_INVITE)
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
    if code == 410:
        say_expired()
        return 7
    if code == 404:
        say("bootstrap is closed (finalized) but no valid local membership exists. "
            + ASK_FOR_INVITE)
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


def invite_management_id(invite):
    """The invite's management id (`ivid_`), derived as the server does. It
    names the invite in the claim state file without storing the invite."""
    return "ivid_" + hashlib.sha256(("invite:" + invite).encode("utf-8")).hexdigest()[:32]


def read_claim_state(state_file):
    state = {}
    try:
        with open(state_file, "r", encoding="utf-8") as f:
            for line in f:
                key, _, value = line.strip().partition("=")
                if key in ("server", "invite_id", "claim_id", "stored"):
                    state[key] = value
    except FileNotFoundError:
        pass
    return state


def stored_mark(key):
    """What the state file records once this set-up's key is in the identity
    file: a one-way mark of that key, never the key. Only a set-up whose mark
    matches the key the identity file holds may acknowledge its claim."""
    return hashlib.sha256(("claim-stored:" + key).encode("utf-8")).hexdigest()[:32]


def write_claim_state(state_file, server, invite_id, claim_id, stored=None):
    """Atomically write the 600 claim state file: before the first claim, and
    again with `stored` once the claimed key is filed and verified."""
    tmp = state_file + ".tmp"
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            f.write("server=%s\ninvite_id=%s\nclaim_id=%s\n" % (server, invite_id, claim_id))
            if stored:
                f.write("stored=%s\n" % stored)
            f.flush()
            os.fsync(f.fileno())
    except Exception:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise
    os.replace(tmp, state_file)
    os.chmod(state_file, 0o600)


def ack_claim(server, invite, claim_id):
    code, _ = http(
        "POST", server + "/membership/claim/ack",
        headers={"x-room-token": invite, "x-claim-id": claim_id},
    )
    return code


def claim_key(server, name, invite, state_file, env_path):
    """Claim this set-up's membership key with an invite; see the module notes.
    Returns the `claim` exit code. Nothing it reads or writes is printed."""
    if not ORIGIN_RE.match(server):
        say("server must be an http(s) origin without a path")
        return 2
    if not NAME_RE.match(name):
        say("name must be 1-64 ASCII letters, digits, dot, dash, or underscore, starting with a letter or digit")
        return 2
    if not invite or any(c.isspace() for c in invite):
        say("an invite is required on stdin")
        return 2
    invite_id = invite_management_id(invite)
    state = read_claim_state(state_file)
    ours = (state.get("server") == server and state.get("invite_id") == invite_id
            and state.get("claim_id"))

    # Already set up: a verified key in the env is kept, and nothing new is
    # claimed. A set-up that stored its key but lost its ACK finishes here.
    #
    # The ACK tells the server to clear its copy of THIS set-up's key, so it
    # is sent only when the identity file holds that key (the state file's
    # `stored` mark matches it). A set-up whose claim is still open while the
    # file holds some other verified key (the claim's answer was lost after
    # that key failed to verify once, say) must not acknowledge: the server
    # would drop its copy of a key nobody has, and the key would stay valid.
    # It goes on below instead and finishes the same set-up: asks again with
    # the same claim id, gets the same key, files it, and then acknowledges.
    held = read_env_membership(env_path)
    replaces_other_key = False
    if membership_verifies(server, held, name):
        if not ours:
            _discard(state_file)
            say("this identity already holds a verified Lobby membership; no new key was claimed.")
            return 0
        if state.get("stored") == stored_mark(held):
            if ack_claim(server, invite, state["claim_id"]) not in (200, 404, 405):
                say("this identity holds a verified Lobby membership, but the server did not "
                    "confirm the set-up's acknowledgement — re-run to finish (nothing is lost).")
                return 6
            _discard(state_file)
            say("this identity already holds a verified Lobby membership; no new key was claimed.")
            return 0
        replaces_other_key = True

    if ours:
        claim_id = state["claim_id"]
    else:
        claim_id = "cl_" + secrets.token_urlsafe(32)
        try:
            os.makedirs(os.path.dirname(os.path.abspath(state_file)), mode=0o700, exist_ok=True)
            write_claim_state(state_file, server, invite_id, claim_id)  # BEFORE the claim
        except OSError:
            say("could not write the claim state file — nothing was claimed; re-run to retry.")
            return 1
    code, text = http(
        "POST", server + "/membership/claim",
        headers={"x-room-token": invite, "x-claim-id": claim_id},
    )
    if code == 409:
        _discard(state_file)
        say("already delivered: this set-up received its key and confirmed it earlier, but "
            "this identity file does not hold a working one. Run setup again to start a new "
            "set-up with this invite, or ask the Master for a new davet.")
        return 8
    if code == 410:
        _discard(state_file)
        say("delivery ended: the key was not confirmed within 24 hours of the claim, so the "
            "server no longer holds it. Run setup again to start a new set-up with this "
            "invite; if that is refused, ask the Master for a new davet.")
        return 7
    if code == 429:
        _discard(state_file)
        say("this davet has already set up five keys. Ask the Master to re-issue the davet "
            "(a new invite can set up keys of its own), then run setup with it.")
        return 9
    if code in (400, 401):
        _discard(state_file)
        say("the invite was not accepted: it may have been revoked, mistyped, or be a "
            "membership key. Ask the Master.")
        return 3
    body = jbody(text)
    key = body.get("membership_token")
    if code not in (200, 201) or not key:
        # The answer carries the key: log only the status, never the body.
        say_code("the claim did not return a key (status %s) — re-run to retry the same set-up.",
                 code)
        return 1
    if body.get("name") != name:
        _discard(state_file)
        say("the invite belongs to a DIFFERENT identity than the one requested — not saving.")
        return 4

    # Verify, persist atomically, re-verify from the env, and only then ACK.
    if not membership_verifies(server, key, name):
        say("the claimed key did not verify (/whoami) — not saving; re-run to retry the same set-up.")
        return 1
    try:
        credentials.update_env_values(
            env_path,
            {"ROOM_SERVER_URL": server, "LOCA_NAME": name, "LOCA_MEMBERSHIP": key},
        )
    except Exception:  # noqa: BLE001
        say("could not persist the identity env — re-run to retry the same set-up.")
        return 1
    if not membership_verifies(server, read_env_membership(env_path), name):
        say("the key was written but did not re-verify from the env — re-run to retry.")
        return 1
    try:
        write_claim_state(state_file, server, invite_id, claim_id, stored_mark(key))
    except OSError:
        say("the key is filed and verified, but the set-up's state file could not be "
            "updated, so it was not acknowledged — re-run to finish (nothing is lost).")
        return 6
    if replaces_other_key:
        say("this set-up's own key, claimed earlier, is now the one in the identity file. "
            "The key the file held before still works and is listed under Profile -> Keys, "
            "where it can be revoked if nothing else uses it.")
    ack = ack_claim(server, invite, claim_id)
    if ack == 200 or (ack in (404, 405) and not body.get("ack_required")):
        # 404/405 without `ack_required`: an older server that hands out the
        # member's key and keeps no delivery copy, so there is nothing to end.
        _discard(state_file)
        return 0
    say("the key is filed and verified, but the server did not confirm the acknowledgement "
        "— re-run to finish (nothing is lost).")
    return 6


def do_claim(server, name, state_file, env_path):
    invite = sys.stdin.readline().strip()
    return claim_key(server, name, invite, state_file, env_path)


def main(argv):
    if len(argv) == 6 and argv[1] == "onboard":
        return do_onboard(argv[2], argv[3], argv[4], argv[5])
    if len(argv) == 6 and argv[1] == "claim":
        return do_claim(argv[2], argv[3], argv[4], argv[5])
    say("usage: join_request.py onboard <server> <name> <state_file> <env_path>")
    say("       join_request.py claim <server> <name> <state_file> <env_path>  (invite on stdin)")
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
