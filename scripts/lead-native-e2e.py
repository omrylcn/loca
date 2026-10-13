#!/usr/bin/env python3
"""Opt-in disposable loopback Lead -> real Codex -> accepted reply/ACK gate."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("onboarding", ROOT / "scripts/onboarding-e2e.py")
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--timeout", type=int, default=150)
    parser.add_argument("--direct", action="store_true", help="also usable for the direct-attention gate")
    args = parser.parse_args()
    codex = shutil.which("codex")
    gate.require(codex is not None, "native Codex unavailable")
    native_home = os.environ.get("CODEX_HOME", str(Path.home() / ".codex"))
    with tempfile.TemporaryDirectory(prefix="loca-lead-native-") as directory:
        work = Path(directory)
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        base = "http://127.0.0.1:" + str(port)
        env = gate.isolated_env(work, CODEX_HOME=native_home)
        processes = []
        with (work / "private.log").open("w") as log:
            try:
                server_env = dict(env, ADMIN_TOKEN=gate.ADMIN, BIND_ADDR="127.0.0.1",
                                  PORT=str(port), DB_PATH=str(work / "server.db"),
                                  REQUIRE_INVITE="1", REQUIRE_SESSIONS="1", RATE_LIMIT="0")
                processes.append(subprocess.Popen([str(args.binary.resolve())], env=server_env,
                                                  stdout=log, stderr=log))
                gate.eventually(lambda: gate.http(base, "GET", "/health"), "server startup")
                operator = gate.http(base, "POST", "/sessions",
                    {"name": "fixture-operator", "kind": "user"}, admin=True)
                operator_headers = {"x-session-token": operator["session_token"]}
                name = "native-lead-probe"
                member = gate.http(base, "POST", "/members", {"name": name, "kind": "agent"}, admin=True)
                invite = gate.http(base, "POST", "/rooms/probe/invites", {"name": name}, admin=True)
                identity = work / "probe.env"
                identity.write_text("ROOM_SERVER_URL=" + base + "\nLOCA_NAME=" + name
                                    + "\nLOCA_MEMBERSHIP=" + member["token"]
                                    + "\nDAVET_probe=" + invite["token"] + "\n")
                identity.chmod(0o600)
                env["LOCA_ENV"] = str(identity)
                inbox = work / "inbox.jsonl"
                inbox.touch(mode=0o600)
                health = work / "adapter-health.json"
                ledger = work / "ledger.db"
                skill = ROOT / "skill/agent-room"
                processes.append(subprocess.Popen(
                    [sys.executable, str(skill / "codex_adapter_v2.py"), "--inbox", str(inbox),
                     "--state-db", str(ledger), "--identity", name, "--workdir", str(work),
                     "--codex-bin", codex, "--relay-mode", "live", "--sandbox", "danger-full-access",
                     "--health-file", str(health)], env=env, stdout=log, stderr=log))
                gate.eventually(lambda: health.exists(), "adapter startup", timeout=40)
                gate.http(base, "POST", "/rooms/probe/lead", {"lead": name}, admin=True)
                processes.append(subprocess.Popen(
                    [sys.executable, str(skill / "listen.py"),
                     base.replace("http://", "ws://") + "/ws?room=probe&name=" + name + "&type=agent&filter=mentions",
                     str(work / "messages.jsonl"), "--skip-own", name, "--only-direct", name,
                     "--turn-log", str(inbox), "--cursor", str(work / "cursor.json")],
                    env=env, stdout=log, stderr=log))
                gate.eventually(lambda: any(m.get("name") == name for m in
                    gate.http(base, "GET", "/rooms/probe/members", admin=True)), "listener online")
                marker = "NATIVE_LEAD_OK_" + uuid.uuid4().hex[:10]
                source = gate.http(base, "POST", "/rooms/probe/messages",
                    {"sender": "fixture-operator", "sender_type": "user",
                     "text": ("@" + name + " " if args.direct else "")
                             + "Lead conformance test. Reply with exactly " + marker
                             + ". Use no tools. This asks for a real room reply."}, headers=operator_headers)
                replies = gate.eventually(lambda: [m for m in gate.http(
                    base, "GET", "/rooms/probe/messages?since=" + str(source["id"]), admin=True)
                    if m.get("sender") == name and marker in m.get("text", "")],
                    "native Lead reply missing", timeout=args.timeout)
                def acked():
                    with sqlite3.connect(ledger) as connection:
                        return connection.execute(
                            "SELECT count(*) FROM attentions WHERE identity=? AND priority=? "
                            "AND accepted_at_ms IS NOT NULL AND turn_completed_at_ms IS NOT NULL",
                            (name, "direct_user" if args.direct else "lead_room")).fetchone()[0] == 1
                gate.eventually(acked, "native turn completion/ACK missing", timeout=30)
                replies = [m for m in gate.http(base, "GET", "/rooms/probe/messages?since="
                    + str(source["id"]), admin=True)
                    if m.get("sender") == name and marker in m.get("text", "")]
                gate.require(len(replies) == 1, "native gate requires exactly one accepted reply")
                gate.require(json.loads(health.read_text()).get("ack") == "OK", "adapter ACK missing")
                print(json.dumps({"result": "PASS", "source_id": source["id"],
                                  "reply_ids": [m["id"] for m in replies],
                                  "boundary": ("direct" if args.direct else "plain Lead")
                                      + " delivery -> native Codex -> one accepted reply -> completion/adapter ACK"}))
            finally:
                for process in reversed(processes):
                    gate.stop(process)


if __name__ == "__main__":
    main()
