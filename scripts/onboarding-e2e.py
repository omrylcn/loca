#!/usr/bin/env python3
"""Fresh shipped ZIP -> approval -> Lobby -> recall -> room delivery, no mocks;
and the invite door: setup claims a key of its own and a re-run claims nothing.

With --legacy-archive ZIP, the installed older kit in that ZIP is run against
this server instead (the older-kit column of the key-hashing design's
compatibility matrix): join request, Lobby, a live call, a server restart
with the listener running (or, with --legacy-binary, the upgrade from that
older server with the listener running), `send`, a call made while the
listener was away, a listener restart and the Master's re-send that brings
it back, and `setup` with a membership key and with an invite.

Uses a disposable loopback Building and isolated HOME for each runtime. This
tests the installed skill and real server, not model inference/native wake.
"""

import argparse
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
import zipfile


ROOT = Path(__file__).resolve().parents[1]
ADMIN = "onboarding-e2e-master"  # Disposable local fixture, never prod auth.


def require(condition, label):
    if not condition:
        raise RuntimeError(label)


def isolated_env(home, **extra):
    prefixes = ("LOCA_", "ROOM_", "DAVET_", "ADMIN_", "CARE_", "SMASTER_")
    env = {key: value for key, value in os.environ.items()
           if not key.startswith(prefixes)
           and key not in ("CODEX_HOME", "CLAUDE_CONFIG_DIR", "PYTHONPATH")}
    env.update(HOME=str(home), LOCA_JOIN_POLL_TIMEOUT="15", **extra)
    return env


def http(base, method, path, body=None, admin=False, headers=None):
    request = urllib.request.Request(
        base + path, method=method,
        data=json.dumps(body).encode() if body is not None else None,
        headers={"content-type": "application/json",
                 **({"x-admin-token": ADMIN} if admin else {}), **(headers or {})},
    )
    try:
        with urllib.request.urlopen(request, timeout=3) as response:
            raw = response.read()
            return json.loads(raw) if raw else None
    except urllib.error.HTTPError as error:
        # Path has only fixture names/request IDs; never log headers or response bodies.
        raise RuntimeError(method + " " + path + ": HTTP " + str(error.code)) from None


def eventually(probe, label, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            result = probe()
            if result:
                return result
        except (OSError, urllib.error.URLError):
            pass
        time.sleep(0.1)
    raise RuntimeError(label)


def stop(process):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)
    for stream in (process.stdout, process.stderr):
        if stream:
            stream.close()


def run_checked(command, env, label):
    result = subprocess.run(command, env=env, capture_output=True, text=True, timeout=30)
    # Never include raw subprocess diagnostics in exceptions: they may be secret-bearing.
    require(result.returncode == 0, label)
    require(not re.search(r"(?:mb_|dv_|st_|jrs_)[0-9a-f]{20,}",
                          result.stdout + result.stderr), label + ": credential leak")
    return result


def identity_values(path):
    # Read only our isolated fixture, without evaluating shell syntax.
    return dict(line.split("=", 1) for line in path.read_text().splitlines()
                if line and not line.startswith("#"))


def check_runtime(base, kit, work, runtime, operator_session):
    name = "fresh-" + runtime
    home = work / runtime
    home.mkdir()
    env = isolated_env(home)
    run_checked(["bash", str(kit / "install.sh"), "--upgrade-only", "--target", runtime],
                env, runtime + ": fresh install failed")
    skill = home / ("." + runtime) / "skills/loca"
    command = ["bash", str(skill / "connect.sh")]
    joining = subprocess.Popen(command + ["request-join", base, name], env=env,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    try:
        pending = eventually(
            lambda: next((row for row in http(base, "GET", "/join-requests", admin=True)["pending"]
                          if row["name"] == name), None), runtime + ": no join request")
        approved = http(base, "POST", "/join-requests/" + pending["id"] + "/approve", admin=True)
        require(approved["status"] == "approved", runtime + ": approval failed")
        out, err = joining.communicate(timeout=25)
        require(joining.returncode == 0, runtime + ": request-join failed after approval")
        require("LOBBY" in out and "monitor setup required" in out,
                runtime + ": misleading onboarding success")
        require(not re.search(r"(?:mb_|jrs_)[0-9a-f]{20,}", out + err),
                runtime + ": onboarding credential leak")
    finally:
        stop(joining)

    identity = home / ".loca" / (name + ".env")
    require(identity.stat().st_mode & 0o777 == 0o600, runtime + ": unsafe env permissions")
    values = identity_values(identity)
    require(values["LOCA_NAME"] == name, runtime + ": wrong identity")
    require(not any(key.startswith("DAVET_") for key in values),
            runtime + ": membership must not imply room access")
    # Rerunning onboarding must reuse the same approved membership, not create another request.
    membership = values["LOCA_MEMBERSHIP"]
    run_checked(command + ["request-join", base, name], env, runtime + ": onboarding retry failed")
    require(identity_values(identity)["LOCA_MEMBERSHIP"] == membership,
            runtime + ": retry changed membership")

    env["LOCA_ENV"] = str(identity)
    inbox = home / "inbox.jsonl"
    cursor = home / "cursor.json"
    listener_command = [sys.executable, "-u", str(skill / "listen.py"),
                        base.replace("http://", "ws://") +
                        "/ws?room=&name=" + name + "&type=agent&filter=mentions&turn_max=1",
                        str(inbox), "--cursor", str(cursor), "--skip-own", name]
    room = "onboarding-" + runtime
    http(base, "POST", "/rooms", {"name": room}, admin=True)

    def online():
        return any(row["name"] == name and row["online"]
                   for row in http(base, "GET", "/residents", admin=True))

    with (home / "listener.log").open("w") as log:
        listener = subprocess.Popen(listener_command, env=env, stdout=log, stderr=log)
        try:
            eventually(online, runtime + ": Lobby presence missing")
            http(base, "POST", "/rooms/" + room + "/call", {"name": name}, admin=True)
            eventually(lambda: "DAVET_" + room.replace("-", "_") in identity_values(identity),
                       runtime + ": Lobby call did not persist invite")
            sent = http(base, "POST", "/rooms/" + room + "/messages",
                        {"sender": "operator", "sender_type": "user",
                         "text": "@" + name + " onboarding control"}, admin=True,
                        headers={"x-session-token": operator_session})
            eventually(lambda: inbox.exists() and "onboarding control" in inbox.read_text(),
                       runtime + ": direct message not delivered")
            require(sent["id"] > 0, runtime + ": control send missing ID")

            # Re-send: the Master replaces the invite. The running listener takes
            # the fresh one from its Lobby line and keeps delivering. (This
            # fixture Building has no invite-only door, so the old invite's 401 is
            # proven by the server tests, not here.)
            invite_key = "DAVET_" + room.replace("-", "_")
            old_invite = identity_values(identity)[invite_key]
            fresh = http(base, "POST", "/rooms/" + room + "/call",
                         {"name": name, "reissue": True}, admin=True)
            fresh_invite = fresh.get("token") or ""
            require(fresh_invite.startswith("dv_") and fresh_invite != old_invite,
                    runtime + ": re-send did not issue a fresh invite")
            eventually(lambda: identity_values(identity).get(invite_key) == fresh_invite,
                       runtime + ": listener did not keep the re-sent invite")
            require(sum(row["name"] == name for row in
                        http(base, "GET", "/rooms/" + room + "/invites", admin=True)) == 1,
                    runtime + ": a re-send left other than one invite")
            http(base, "POST", "/rooms/" + room + "/messages",
                 {"sender": "operator", "sender_type": "user",
                  "text": "@" + name + " re-send control"}, admin=True,
                 headers={"x-session-token": operator_session})
            eventually(lambda: "re-send control" in inbox.read_text(),
                       runtime + ": delivery after re-send failed")
        finally:
            stop(listener)

        # Restart the installed listener with its durable cursor; no manual setup needed.
        listener = subprocess.Popen(listener_command, env=env, stdout=log, stderr=log)
        try:
            eventually(online, runtime + ": Lobby restart failed")
            http(base, "POST", "/rooms/" + room + "/messages",
                 {"sender": "operator", "sender_type": "user",
                  "text": "@" + name + " restart control"}, admin=True,
                 headers={"x-session-token": operator_session})
            eventually(lambda: "restart control" in inbox.read_text(),
                       runtime + ": restart delivery failed")
            result = run_checked(command + ["send", base, room, name, "operator", "fixture reply"],
                                 env, runtime + ": installed send failed")
            require(json.loads(result.stdout)["id"] > 0, runtime + ": send receipt missing")
            messages = http(base, "GET", "/rooms/" + room + "/messages", admin=True)
            require(sum(row["sender"] == name and row["text"] == "fixture reply"
                        for row in messages) == 1, runtime + ": reply absent or duplicated")
        finally:
            stop(listener)
    print(runtime + ": ZIP install / approval / idempotent onboarding / Lobby / recall / delivery / re-send / restart / send OK")


def check_invite_setup(base, kit, work, runtime):
    """The Master's other door: `setup` with an invite claims a Lobby key of the
    installation's own, never the member's key, and a re-run claims nothing."""
    name = "invite-" + runtime
    home = work / ("invite-" + runtime)
    home.mkdir()
    identity = home / ".loca" / (name + ".env")
    env = isolated_env(home, LOCA_ENV=str(identity))
    run_checked(["bash", str(kit / "install.sh"), "--upgrade-only", "--target", runtime],
                env, runtime + ": invite install failed")
    connect = ["bash", str(home / ("." + runtime) / "skills/loca/connect.sh")]
    member = http(base, "POST", "/members", {"name": name, "kind": "agent"}, admin=True)
    original = member["token"]
    room = "invite-" + runtime
    http(base, "POST", "/rooms", {"name": room}, admin=True)
    invite = http(base, "POST", "/rooms/" + room + "/invites", {"name": name}, admin=True)
    require(invite.get("member_id", "").startswith("mbid_") and "member" not in invite,
            runtime + ": invite answer handed out the member's key")
    for attempt in ("first", "re-run"):
        result = subprocess.run(connect + ["setup", base, name], env=env, input=invite["token"] + "\n",
                                capture_output=True, text=True, timeout=60)
        require(result.returncode == 0, runtime + ": invite setup failed (" + attempt + ")")
        require(not re.search(r"(?:mb_|dv_|st_)[0-9a-f]{20,}|cl_[A-Za-z0-9_-]{20,}",
                              result.stdout + result.stderr),
                runtime + ": invite setup credential leak")
        values = identity_values(identity)
        if attempt == "first":
            claimed = values["LOCA_MEMBERSHIP"]
        require(values["LOCA_MEMBERSHIP"] == claimed, runtime + ": re-run changed the key")
        require(claimed.startswith("mb_") and claimed != original,
                runtime + ": setup stored the member's own key")
        require(values["DAVET_" + room.replace("-", "_")] == invite["token"],
                runtime + ": invite not filed")
        require(not (home / ".loca" / (name + ".claim")).exists(),
                runtime + ": claim state left behind")
        keys = http(base, "GET", "/profile/credentials", headers={"x-admin-token": claimed})
        require(sum(key["label"] == "Membership key (claimed with davet for #" + room + ")"
                    for key in keys) == 1, runtime + ": not exactly one claimed key")
    require(identity.stat().st_mode & 0o777 == 0o600, runtime + ": unsafe invite env permissions")
    again = http(base, "POST", "/members", {"name": name, "kind": "agent"}, admin=True)
    require(again.get("already") is True and "token" not in again,
            runtime + ": re-admission handed out the member's key")
    print(runtime + ": invite setup claims its own key / re-run claims nothing / no key handed out OK")


def check_legacy_kit(base, kit, work, runtime, operator_session, restart):
    """The older kit, installed, against this server: what it still does."""
    name = "legacy-" + runtime
    home = work / name
    home.mkdir()
    env = isolated_env(home)
    run_checked(["bash", str(kit / "install.sh"), "--upgrade-only", "--target", runtime],
                env, runtime + ": older kit install failed")
    skill = home / ("." + runtime) / "skills/loca"
    command = ["bash", str(skill / "connect.sh")]
    joining = subprocess.Popen(command + ["request-join", base, name], env=env,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    try:
        pending = eventually(
            lambda: next((row for row in http(base, "GET", "/join-requests", admin=True)["pending"]
                          if row["name"] == name), None), runtime + ": no older-kit join request")
        http(base, "POST", "/join-requests/" + pending["id"] + "/approve", admin=True)
        out, err = joining.communicate(timeout=25)
        require(joining.returncode == 0, runtime + ": older-kit request-join failed")
        require("LOBBY" in out, runtime + ": older kit did not reach the Lobby")
        require(not re.search(r"(?:mb_|jrs_)[0-9a-f]{20,}", out + err),
                runtime + ": older-kit onboarding credential leak")
    finally:
        stop(joining)
    identity = home / ".loca" / (name + ".env")
    require(identity.stat().st_mode & 0o777 == 0o600, runtime + ": unsafe env permissions")
    env["LOCA_ENV"] = str(identity)
    inbox = home / "inbox.jsonl"
    listener_command = [sys.executable, "-u", str(skill / "listen.py"),
                        base.replace("http://", "ws://") +
                        "/ws?room=&name=" + name + "&type=agent&filter=mentions&turn_max=1",
                        str(inbox), "--cursor", str(home / "cursor.json"), "--skip-own", name]

    def online():
        return any(row["name"] == name and row["online"]
                   for row in http(base, "GET", "/residents", admin=True))

    def say(room, text):
        http(base, "POST", "/rooms/" + room + "/messages",
             {"sender": "operator", "sender_type": "user", "text": "@" + name + " " + text},
             admin=True, headers={"x-session-token": operator_session[0]})
        eventually(lambda: inbox.exists() and text in inbox.read_text(),
                   runtime + ": older kit did not receive '" + text + "'", timeout=20)

    first, later = "legacy-a-" + runtime, "legacy-b-" + runtime
    for room in (first, later):
        http(base, "POST", "/rooms", {"name": room}, admin=True)
    log = (home / "listener.log").open("w")
    listener = subprocess.Popen(listener_command, env=env, stdout=log, stderr=log)
    try:
        eventually(online, runtime + ": older kit not present in the Lobby")
        http(base, "POST", "/rooms/" + first + "/call", {"name": name}, admin=True)
        eventually(lambda: "DAVET_" + first.replace("-", "_") in identity_values(identity),
                   runtime + ": older kit did not keep the called invite")
        say(first, "before the restart")
        # The server goes away and comes back (upgraded, with --legacy-binary)
        # while the listener runs: its loca thread renews with the invite it
        # keeps, which the server now knows only by its hash.
        operator_session[0] = restart()
        eventually(online, runtime + ": older kit did not come back to the Lobby")
        say(first, "after the restart")
        result = run_checked(command + ["send", base, first, name, "operator", "older reply"],
                             env, runtime + ": older-kit send failed")
        require(json.loads(result.stdout)["id"] > 0, runtime + ": older-kit send receipt missing")
        messages = http(base, "GET", "/rooms/" + first + "/messages", admin=True)
        require(sum(row["sender"] == name and row["text"] == "older reply"
                    for row in messages) == 1, runtime + ": older-kit reply absent or duplicated")
    finally:
        stop(listener)
    # Called while away: the invite's delivery copy is sent on the next
    # Lobby connect. The loca the restarted listener was seated in before is
    # not re-entered (the older kit starts a loca only from a frame), until
    # the Master re-sends its invitation.
    http(base, "POST", "/rooms/" + later + "/call", {"name": name}, admin=True)
    listener = subprocess.Popen(listener_command, env=env, stdout=log, stderr=log)
    try:
        eventually(online, runtime + ": older kit restart failed")
        say(later, "called while away")
        http(base, "POST", "/rooms/" + first + "/call", {"name": name, "reissue": True},
             admin=True)
        say(first, "after the re-send")
    finally:
        stop(listener)
        log.close()

    # setup with a membership key and with an invite.
    for door in ("key", "invite"):
        who = "legacy-" + door + "-" + runtime
        setup_home = work / who
        setup_home.mkdir()
        setup_identity = setup_home / ".loca" / (who + ".env")
        setup_env = isolated_env(setup_home, LOCA_ENV=str(setup_identity))
        run_checked(["bash", str(kit / "install.sh"), "--upgrade-only", "--target", runtime],
                    setup_env, runtime + ": older kit install failed")
        member = http(base, "POST", "/members", {"name": who, "kind": "agent"}, admin=True)
        secret = member["token"]
        if door == "davet":
            http(base, "POST", "/rooms", {"name": who}, admin=True)
            secret = http(base, "POST", "/rooms/" + who + "/invites", {"name": who},
                          admin=True)["token"]
        result = subprocess.run(
            ["bash", str(setup_home / ("." + runtime) / "skills/loca/connect.sh"),
             "setup", base, who],
            env=setup_env, input=secret + "\n", capture_output=True, text=True, timeout=60)
        require(result.returncode == 0, runtime + ": older-kit setup with a " + door + " failed")
        require(not re.search(r"(?:mb_|dv_|st_)[0-9a-f]{20,}", result.stdout + result.stderr),
                runtime + ": older-kit setup credential leak")
        stored = identity_values(setup_identity)["LOCA_MEMBERSHIP"]
        require((stored == member["token"]) == (door == "key"),
                runtime + ": older-kit setup stored the wrong key")
        keys = http(base, "GET", "/profile/credentials", headers={"x-admin-token": stored})
        require(len(keys) >= 1, runtime + ": older-kit " + door + " setup key does not sign in")
    print(runtime + ": older kit: join / Lobby / call / server restart" +
          " / send / call while away / re-send / setup with a key and an invite OK")


def start_building(binary, env, log, base):
    process = subprocess.Popen([str(binary)], env=env, stdout=log, stderr=log)
    eventually(lambda: http(base, "GET", "/health"), "server startup failed", timeout=60)
    return process


def operator_session(base):
    pairing = http(base, "POST", "/pairings", admin=True)
    return http(base, "POST", "/sessions", {"name": "operator", "kind": "user"},
                headers={"x-pairing-code": pairing["pairing_code"]})["session_token"]


def run_legacy(binary, archive, legacy_binary):
    require(archive.is_file(), "older kit ZIP is missing")
    with tempfile.TemporaryDirectory(prefix="loca-onboarding-legacy-") as tmp:
        work = Path(tmp)
        with zipfile.ZipFile(archive) as bundle:
            bundle.extractall(work)
        kit = work / "loca-remote-agent"
        checksums = subprocess.run(["sha256sum", "-c", "SHA256SUMS"], cwd=kit,
                                   capture_output=True, text=True, timeout=10)
        require(checksums.returncode == 0, "older kit ZIP checksum verification failed")
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        base = "http://127.0.0.1:" + str(port)
        with (work / "server.log").open("w") as log:
            print("older kit " + (kit / "VERSION").read_text().strip() + " against "
                  + ("an upgrade from " + str(legacy_binary) if legacy_binary
                     else "this server"))
            for runtime in ("codex", "claude"):
                # A Building of its own per runtime, started on the older
                # server when one is given: an upgraded database cannot go back.
                env = isolated_env(work, ADMIN_TOKEN=ADMIN, PORT=str(port),
                                   BIND_ADDR="127.0.0.1",
                                   DB_PATH=str(work / (runtime + ".sqlite3")), RATE_LIMIT="0",
                                   REQUIRE_SESSIONS="1", LEGACY_WS_QUERY_AUTH="0")
                current = [start_building(legacy_binary or binary, env, log, base)]
                try:
                    def restart():
                        stop(current[0])
                        current[0] = start_building(binary, env, log, base)
                        print("real server version=" + http(base, "GET", "/health")["version"])
                        return operator_session(base)

                    http(base, "POST", "/admission-stock", {"count": 1}, admin=True)
                    session = [operator_session(base)]
                    check_legacy_kit(base, kit, work, runtime, session, restart)
                finally:
                    stop(current[0])
                if legacy_binary:
                    # The upgrade is one-way: the older server refuses the
                    # upgraded database and exits.
                    older = subprocess.run([str(legacy_binary)], env=env, stdout=log,
                                           stderr=log, timeout=60)
                    require(older.returncode != 0,
                            runtime + ": the older server started on an upgraded database")
                    print(runtime + ": the older server refuses the upgraded database")
    print("ALL OLDER-KIT ONBOARDING CHECKS PASSED (no model inference/native wake claim)")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/debug/room-server")
    parser.add_argument("--archive", type=Path,
                        help="Test this exact ZIP without rebuilding or replacing it")
    parser.add_argument("--legacy-archive", type=Path,
                        help="Run an older published kit ZIP against this server instead")
    parser.add_argument("--legacy-binary", type=Path,
                        help="With --legacy-archive: start on this older room-server, then "
                             "upgrade to --binary while the older kit's listener runs")
    args = parser.parse_args()
    binary = Path(args.binary).resolve()
    require(binary.is_file(), "build room-server before running this gate")
    if args.legacy_archive is not None:
        legacy_binary = args.legacy_binary.resolve() if args.legacy_binary else None
        require(legacy_binary is None or legacy_binary.is_file(), "older room-server is missing")
        run_legacy(binary, args.legacy_archive.resolve(), legacy_binary)
        return
    if args.archive is None:
        subprocess.run([str(ROOT / "scripts/build-remote-agent-kit.sh")], cwd=ROOT,
                       check=True, stdout=subprocess.DEVNULL)
    candidate = args.archive.resolve() if args.archive else ROOT / "dist/loca-remote-agent.zip"
    require(candidate.is_file(), "candidate ZIP is missing")
    with tempfile.TemporaryDirectory(prefix="loca-onboarding-e2e-") as tmp:
        work = Path(tmp)
        with zipfile.ZipFile(candidate) as archive:
            archive.extractall(work)
        kit = work / "loca-remote-agent"
        for line in (ROOT / "scripts/skill-bundle-files.txt").read_text().splitlines():
            if not line or line.startswith("#"):
                continue
            for source, name in (("skill/agent-room/", "loca"),
                                 ("skill/loca-care/", "loca-care")):
                if line.startswith(source):
                    shipped = kit / name / line[len(source):]
                    require(shipped.is_file(), "candidate ZIP missing runtime file: " + line)
                    require(shipped.read_bytes() == (ROOT / line).read_bytes(),
                            "candidate ZIP runtime differs from source: " + line)
        checksums = subprocess.run(["sha256sum", "-c", "SHA256SUMS"], cwd=kit,
                                   capture_output=True, text=True, timeout=10)
        require(checksums.returncode == 0, "shipped ZIP checksum verification failed")
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        base = "http://127.0.0.1:" + str(port)
        env = isolated_env(work, ADMIN_TOKEN=ADMIN, PORT=str(port), BIND_ADDR="127.0.0.1",
                           DB_PATH=str(work / "building.sqlite3"), RATE_LIMIT="0",
                           REQUIRE_SESSIONS="1", LEGACY_WS_QUERY_AUTH="0")
        with (work / "server.log").open("w") as log:
            server = subprocess.Popen([str(binary)], env=env, stdout=log, stderr=log)
            try:
                info = eventually(lambda: http(base, "GET", "/health"), "server startup failed")
                print("real server version=" + info["version"])
                http(base, "POST", "/admission-stock", {"count": 2}, admin=True)
                pairing = http(base, "POST", "/pairings", admin=True)
                operator = http(base, "POST", "/sessions",
                                {"name": "operator", "kind": "user"},
                                headers={"x-pairing-code": pairing["pairing_code"]})
                for runtime in ("codex", "claude"):
                    check_runtime(base, kit, work, runtime, operator["session_token"])
                    check_invite_setup(base, kit, work, runtime)
            finally:
                stop(server)
    print("ALL ONBOARDING E2E CHECKS PASSED (no model inference/native wake claim)")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError, subprocess.SubprocessError) as error:
        # Labels above are static. Avoid dumping HTTP bodies, envs or subprocess output.
        print("onboarding E2E failed: " + type(error).__name__, file=sys.stderr)
        if isinstance(error, RuntimeError):
            print(str(error), file=sys.stderr)
        sys.exit(1)
