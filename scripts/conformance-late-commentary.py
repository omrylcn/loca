#!/usr/bin/env python3
"""Real Codex/WS/relay regression for a direct call steered after commentary.

Only disposable loopback identities are used. Native Codex login is required;
no model responder fixture or manual room reply substitutes for inference.
"""
import argparse
from contextlib import contextmanager
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
SKILL = ROOT / "skill/agent-room"
sys.path.insert(0, str(SKILL))
from attention_store import AttentionStore
from codex_adapter_v2 import ConnectShRelay, PersistentCodexAdapter
from credentials import load_local_env
from runtime_agent import report_runtime_health

spec = importlib.util.spec_from_file_location("protocol", ROOT / "scripts/wiki-protocol-e2e.py")
protocol = importlib.util.module_from_spec(spec)
spec.loader.exec_module(protocol)


@contextmanager
def environment(env):
    old = dict(os.environ)
    os.environ.clear()
    os.environ.update(env)
    try:
        yield
    finally:
        os.environ.clear()
        os.environ.update(old)


def main():
    started = time.monotonic()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--codex-bin", default="codex")
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/room-server")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--early-final", action="store_true",
                        help="Require a native final before a later same-turn direct call")
    args = parser.parse_args()
    result = {"ok": False, "source_sha": subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
        "source_dirty": bool(subprocess.check_output(
            ["git", "status", "--porcelain", "--untracked-files=no"], cwd=ROOT, text=True).strip()),
        "server_binary_sha256": hashlib.sha256(args.binary.read_bytes()).hexdigest()}
    result["early_final"] = args.early_final
    native_home = str(Path.home() / ".codex")
    prefixes = ("LOCA_", "ROOM_", "DAVET_", "ADMIN_", "CARE_", "SMASTER_")
    env = {k: v for k, v in os.environ.items() if not k.startswith(prefixes)}
    env.setdefault("CODEX_HOME", native_home)
    with tempfile.TemporaryDirectory(prefix="loca-late-commentary-") as directory:
        work = Path(directory)
        db = work / "building.db"
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        base = f"http://127.0.0.1:{port}"
        env.update(HOME=str(work), ROOM_SERVER_URL=base)
        server_env = dict(env, ADMIN_TOKEN=protocol.ADMIN, BIND_ADDR="127.0.0.1",
                          PORT=str(port), DB_PATH=str(db), REQUIRE_INVITE="1",
                          REQUIRE_SESSIONS="1", RATE_LIMIT="0")
        adapter = listener = server = None
        with (work / "server.log").open("w") as server_log, (work / "listener.log").open("w") as listener_log:
            try:
                server = subprocess.Popen([str(args.binary)], env=server_env,
                                          stdout=server_log, stderr=server_log)
                protocol.eventually(lambda: protocol.http(base, "GET", "/health"), "server startup failed")
                admin = {"x-admin-token": protocol.ADMIN}
                headers = {}
                for name, kind in (("reader", "agent"), ("operator", "user")):
                    member = protocol.http(base, "POST", "/members", {"name": name, "kind": kind}, admin)
                    davet = protocol.http(base, "POST", "/rooms/general/invites", {"name": name}, admin)["token"]
                    session = protocol.http(base, "POST", "/sessions", {"name": name, "kind": kind, "loca": "general"},
                                            {"x-room-token": davet})["session_token"]
                    headers[name] = {"x-session-token": session}
                    if name == "reader":
                        identity = work / "reader.env"
                        identity.write_text(f"ROOM_SERVER_URL={base}\nLOCA_NAME=reader\nLOCA_MEMBERSHIP={member['token']}\n"
                                            f"LOCA_SESSION={session}\nDAVET_general={davet}\n")
                        identity.chmod(0o600)
                        env.update(LOCA_ENV=str(identity), LOCA_NAME="reader")
                inbox = work / "inbox.jsonl"
                inbox.touch()
                listener = subprocess.Popen([sys.executable, str(SKILL / "listen.py"),
                    f"ws://127.0.0.1:{port}/ws?room=general&name=reader&type=agent&filter=mentions&turn_max=1",
                    str(work / "messages.jsonl"), "--skip-own", "reader", "--turn-log", str(inbox)],
                    env=env, stdout=listener_log, stderr=listener_log)
                protocol.eventually(lambda: any(m.get("name") == "reader" for m in
                    protocol.http(base, "GET", "/rooms/general/members", headers=admin)), "listener not online")
                with environment(env):
                    store = AttentionStore(work / "ledger.db")
                    adapter = PersistentCodexAdapter(store=store, inbox=inbox, identity="reader",
                        workdir=work, codex_bin=args.codex_bin, relay_mode="live",
                        relay=ConnectShRelay(SKILL / "connect.sh"), context_provider=lambda _a: [],
                        health_file=work / "health.json")
                    adapter.initialize()
                    def say(text):
                        return protocol.http(base, "POST", "/rooms/general/messages", {
                            "sender": "operator", "sender_type": "user", "target": "reader", "text": text,
                        }, headers["operator"])["id"]
                    def rows():
                        with store.connect() as connection:
                            return [dict(r) for r in connection.execute("SELECT * FROM relay_items ORDER BY created_at_ms, op_id")]
                    def wait(probe, label):
                        deadline = time.monotonic() + 180
                        while time.monotonic() < deadline:
                            adapter.cycle()
                            if probe():
                                return
                            time.sleep(0.05)
                        raise RuntimeError(label)
                    first_kind = "final" if args.early_final else "commentary"
                    initial = ("@reader For this runtime conformance test, emit an agent message in final_answer phase "
                               "containing INITIAL_RECEIPT, then ALSO call the shell tool to run sleep 20 once. "
                               "Keep working after the initial answer: the answer and the tool call belong to this same turn. "
                               "A newer direct ping may arrive during the command.") if args.early_final else (
                               "@reader Emit a commentary message containing INITIAL_RECEIPT. Then use the shell tool "
                               "to run sleep 20 once. Do not finalize until that command finishes. A newer direct ping may arrive.")
                    first_id = say(initial)
                    wait(lambda: any(r["relay_kind"] == first_kind and r["status"] == "accepted" and
                                     "INITIAL_RECEIPT" in r["text"] for r in rows()), "initial native " + first_kind + " missing")
                    protocol.require(bool(adapter.active_turns), "first turn ended before the late ping")
                    turn_id = next(iter(adapter.active_turns.values()))
                    late_id = say("@reader New direct ping: emit a commentary containing LATE_DIRECT_RECEIPT to acknowledge THIS ping, "
                                  "then continue the running work. Final answer must contain FINAL_RECEIPT. Do not send messages through shell tools.")
                    late_sent_at = time.monotonic()
                    wait(lambda: any(r["relay_kind"] == "commentary" and r["status"] == "accepted" and
                                     "LATE_DIRECT_RECEIPT" in r["text"] for r in rows()), "late native commentary was not relayed")
                    late_response_seconds = round(time.monotonic() - late_sent_at, 3)
                    wait(lambda: len(store.snapshot("reader")["attentions"]) == 2 and all(
                        a["final_response_at_ms"] and a["turn_completed_at_ms"] for a in store.snapshot("reader")["attentions"]),
                        "final response lifecycle did not complete")
                    attentions = store.snapshot("reader")["attentions"]
                    protocol.require({a["turn_id"] for a in attentions} == {turn_id}, "late ping started another turn")
                    with sqlite3.connect(db) as connection:
                        replies = connection.execute("SELECT id,text,op_id FROM messages WHERE sender='reader' AND room='general' ORDER BY id").fetchall()
                    accepted = [r for r in rows() if r["status"] == "accepted"]
                    # A final answer may repeat a receipt phrase. Duplicate
                    # transport means duplicate op_id, not repeated wording.
                    protocol.require(len(replies) == len(accepted), "server/ledger output count differs")
                    for relay in accepted:
                        matching = [(mid, text) for mid, text, op in replies if op == relay["op_id"]]
                        protocol.require(len(matching) == 1 and matching[0][1] == relay["text"],
                                         "missing, duplicate or changed server output for completed item")
                    for marker, kind, coverage_size in (("INITIAL_RECEIPT", first_kind, 1),
                                                        ("LATE_DIRECT_RECEIPT", "commentary", 2),
                                                        ("FINAL_RECEIPT", "final", 2)):
                        matching = [r for r in accepted if r["relay_kind"] == kind and marker in r["text"]
                                    and len(json.loads(r["covered_attention_ids_json"])) == coverage_size]
                        protocol.require(len(matching) == 1, "missing/duplicate completed native item: " + marker)
                    load_local_env("reader")
                    report_runtime_health(work / "health.json", True)
                    runtime = next(r["runtime"] for r in protocol.http(base, "GET", "/residents", headers=admin) if r["name"] == "reader")
                    protocol.require(runtime["ack"] == "OK" and all(runtime.get(k) for k in
                        ("stored", "accepted", "first_response", "final_response", "turn_completed")), "server lifecycle receipt incomplete")
                    result.update(ok=True, delivery_ids=[first_id, late_id], turn_id=turn_id,
                                  late_response_seconds=late_response_seconds,
                                  elapsed_seconds=round(time.monotonic() - started, 3),
                                  reply_ids=[mid for mid, _, _ in replies], runtime_receipt=runtime,
                                  relay_items=[{k: r[k] for k in ("item_id", "relay_kind", "status", "covered_attention_ids_json")} for r in rows()])
            except Exception as error:
                result.update(failure_class=type(error).__name__, failure=str(error))
                if adapter:
                    result["relay_items"] = [{k: r[k] for k in ("item_id", "relay_kind", "status", "covered_attention_ids_json")} for r in rows()]
                if args.output:
                    args.output.write_text(json.dumps(result, indent=2) + "\n")
                raise
            finally:
                if adapter:
                    adapter.close()
                for process in (listener, server):
                    if process and process.poll() is None:
                        process.terminate()
                        try:
                            process.wait(timeout=10)
                        except subprocess.TimeoutExpired:
                            process.kill()
                            process.wait(timeout=10)
    rendered = json.dumps(result, indent=2) + "\n"
    if args.output:
        args.output.write_text(rendered)
    print(rendered, end="")


if __name__ == "__main__":
    main()
