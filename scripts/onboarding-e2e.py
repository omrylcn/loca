#!/usr/bin/env python3
"""Fresh shipped ZIP -> approval -> Lobby -> recall -> room delivery, no mocks.

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
    print(runtime + ": ZIP install / approval / idempotent onboarding / Lobby / recall / delivery / restart / send OK")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/debug/room-server")
    parser.add_argument("--archive", type=Path,
                        help="Test this exact ZIP without rebuilding or replacing it")
    args = parser.parse_args()
    binary = Path(args.binary).resolve()
    require(binary.is_file(), "build room-server before running this gate")
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
