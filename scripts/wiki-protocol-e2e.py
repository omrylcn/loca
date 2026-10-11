#!/usr/bin/env python3
"""Real scheduler/HTTP/shell/coordinator/delta gate, with a deterministic worker.

This does NOT certify native model delegation or actual context injection.
Only a disposable loopback database bypasses the public automatic-enable guard.
No production identity or runtime is read, installed, restarted or modified.
"""
import json
import os
from pathlib import Path
import re
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
SKILL = ROOT / "skill/agent-room"
sys.path.insert(0, str(SKILL))
from wiki_coordinator import WikiCoordinator
from wiki_context import WikiPageContext

ADMIN = "wiki-protocol-disposable-only"


def require(condition, label):
    if not condition:
        raise RuntimeError(label)


def http(base, method, path, body=None, headers=None):
    request = urllib.request.Request(base+path, method=method,
        data=json.dumps(body).encode() if body is not None else None,
        headers={"content-type":"application/json", **(headers or {})})
    with urllib.request.urlopen(request, timeout=3) as response:
        raw = response.read()
        return json.loads(raw) if raw else None


def eventually(probe, label, seconds=22):
    deadline = time.monotonic()+seconds
    while time.monotonic() < deadline:
        try:
            value = probe()
            if value:
                return value
        except (OSError, urllib.error.URLError):
            pass
        time.sleep(0.1)
    raise RuntimeError(label)


def main(native_factory=None):
    binary = ROOT / "target/debug/room-server"
    with tempfile.TemporaryDirectory(prefix="loca-wiki-protocol-") as directory:
        work = Path(directory)
        db = work / "building.sqlite3"
        prefixes = ("LOCA_","ROOM_","DAVET_","ADMIN_","CARE_","SMASTER_")
        env = {key:value for key,value in os.environ.items()
               if not key.startswith(prefixes) and key not in ("CODEX_HOME","CLAUDE_CONFIG_DIR","PYTHONPATH")}
        env["HOME"] = str(work)
        with socket.socket() as sock:
            sock.bind(("127.0.0.1",0))
            port = sock.getsockname()[1]
        base = "http://127.0.0.1:"+str(port)
        server_env = dict(env, ADMIN_TOKEN=ADMIN, BIND_ADDR="127.0.0.1", PORT=str(port),
                          DB_PATH=str(db), REQUIRE_INVITE="1", REQUIRE_SESSIONS="1", RATE_LIMIT="0")
        with (work / "server.log").open("w") as log:
            server = subprocess.Popen([str(binary)],env=server_env,stdout=log,stderr=log)
            try:
                eventually(lambda: http(base,"GET","/health"),"server did not start")
                admin = {"x-admin-token":ADMIN}
                for name in ("editor","reader"):
                    http(base,"POST","/members",{"name":name,"kind":"agent"},admin)
                    davet = http(base,"POST","/rooms/general/invites",{"name":name},admin)["token"]
                    session = http(base,"POST","/sessions",{"name":name,"kind":"agent","loca":"general"},
                                   {"x-room-token":davet})["session_token"]
                    home = work / name
                    (home / ".loca").mkdir(parents=True)
                    identity = home / ".loca" / (name+".env")
                    identity.write_text("ROOM_SERVER_URL="+base+"\nLOCA_NAME="+name+
                                        "\nLOCA_SESSION="+session+"\nDAVET_general="+davet+"\n")
                    identity.chmod(0o600)
                    if name == "reader":
                        reader_headers = {"x-session-token":session}
                http(base,"PUT","/rooms/general/wiki/config",{"editor":"editor","enabled":False,
                                                               "interval_messages":3},admin)
                try:
                    http(base,"PUT","/rooms/general/wiki/config",{"editor":"editor","enabled":True,
                                                                   "interval_messages":3},admin)
                    raise RuntimeError("public enable must remain blocked without native conformance")
                except urllib.error.HTTPError as error:
                    require(error.code == 409,"wrong enable-guard result")
                with sqlite3.connect(db) as connection:
                    connection.execute("UPDATE loca_wiki SET enabled=1 WHERE room='general'")

                def command(name, kind, payload=None, slug=None, success=True):
                    args = ["bash",str(SKILL / "connect.sh"),kind,base,"general",name]
                    if payload is not None:
                        path = work / "publication.json"
                        path.write_text(json.dumps(payload))
                        path.chmod(0o600)
                        args.append(str(path))
                    if slug is not None:
                        args.append(slug)
                    result = subprocess.run(args,env=dict(env,HOME=str(work / name)),
                                            capture_output=True,text=True,timeout=20)
                    require(not re.search(r"(?:mb_|dv_|st_)[0-9a-f]{20,}",result.stdout+result.stderr),
                            "helper leaked credential")
                    if not success:
                        require(result.returncode != 0,"unexpected helper acceptance")
                        return None
                    require(result.returncode == 0,"helper failed: "+kind)
                    return json.loads(result.stdout)

                def queue_ready():
                    return http(base,"GET","/rooms/general/wiki/maintenance",headers=admin)["status"] == "queued"

                source_ids = []
                for text in ("proposal: a revision is not proof", "decision: only changed sections",
                             "finding: leave unrelated content untouched"):
                    source_ids.append(http(base,"POST","/rooms/general/messages",
                        {"sender":"reader","sender_type":"agent","text":text},reader_headers)["id"])
                eventually(queue_ready,"real scheduler did not queue threshold work")
                command("reader","wiki-claim",success=False)
                worker_calls, writes = [], []
                native = native_factory(work) if native_factory else None

                def worker(payload):
                    worker_calls.append(payload)
                    require("lease" not in payload,"worker received publication authority")
                    require([m["id"] for m in payload["messages"]] == source_ids,"incomplete source input")
                    if native:
                        return native.review(payload)
                    new = len(worker_calls) == 2
                    return {"expected_revision":payload["wiki"]["revision"],
                            "reviewed_through":payload["reviewed_through"],"reason":"deterministic protocol fixture",
                            "pages":[{"slug":"overview","title":"Overview","sources":source_ids.copy(),
                                      "body":"## Policy\n"+("new rule" if new else "old rule")+
                                             "\n## Unchanged\nkeep this\n"}]}

                def finish(payload):
                    writes.append(payload)
                    result = command("editor","wiki-finish",payload)
                    if native:
                        print("Loca: native worker publication accepted",flush=True)
                    return result

                coordinator = WikiCoordinator("general",lambda:command("editor","wiki-claim"),
                    lambda:command("editor","wiki"),finish,worker)
                require(coordinator.run_once()["status"] == "accepted","first review was not accepted")
                # Simulate a lost HTTP response: identical payload recovers the
                # acceptance receipt, never another page revision/model call.
                require(command("editor","wiki-finish",writes[-1])["revision"] == 1,"retry duplicated edit")
                require(coordinator.run_once()["status"] == "idle","idle review should not call worker")
                require(len(worker_calls) == 1,"idle or retry invoked worker")
                selected = command("reader","wiki-page",slug="overview")
                context_id = native.context if native else "fixture-model-context"
                context = WikiPageContext("general",context_id)
                packet = context.prepare(dict(selected,pages=[selected["page"]]),["overview"])
                require(packet["pages"][0]["kind"] == "bootstrap","first read not bootstrap")
                # Explicit simulated injection: this is not a native model ACK.
                if native:
                    native.inject(packet, context, "old rule")
                else:
                    require(context.acknowledge_injection(packet["packet_id"],"general","fixture-model-context"),
                            "context baseline not accepted")
                source_ids[:] = [http(base,"POST","/rooms/general/messages",
                    {"sender":"reader","sender_type":"agent","text":"decision supersedes old rule"},reader_headers)["id"]]
                with sqlite3.connect(db) as connection:
                    connection.execute("UPDATE loca_wiki SET interval_messages=1 WHERE room='general'")
                eventually(queue_ready,"second source was not scheduled")
                require(coordinator.run_once()["status"] == "accepted","second review was not accepted")
                selected = command("reader","wiki-page",slug="overview")
                delta = context.prepare(dict(selected,pages=[selected["page"]]),["overview"])
                require(delta["pages"][0]["kind"] == "delta","changed page not incremental")
                require("keep this" not in json.dumps(delta),"unchanged section was resent")
                if native:
                    native.inject(delta, context, "new rule")
                require(selected["revision"] == 2,"publication revision incorrect")
                require(len(http(base,"GET","/rooms/general/messages",headers=admin)) == 4,
                        "maintenance generated chat spam")
                if native:
                    print("Wiki native E2E passed: real scheduler / isolated model worker / HTTP publication / idempotent retry / model bootstrap + section delta / no chat spam")
                else:
                    print("Wiki protocol E2E passed: real scheduler / editor-only claim / delegated fixture / atomic finish / retry / selected read / section delta / no chat spam")
                    print("Native Codex/Claude worker and model-context injection remain UNVERIFIED; public auto-enable remains blocked")
            finally:
                if 'native' in locals() and native:
                    native.close()
                if server.poll() is None:
                    server.terminate()
                    try:
                        server.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        server.kill()
                        server.wait(timeout=5)


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError,OSError,ValueError,urllib.error.HTTPError,subprocess.SubprocessError) as error:
        # No HTTP body, prompt, environment, subprocess output or credential dump.
        print("Wiki protocol E2E failed: "+type(error).__name__,file=sys.stderr)
        if isinstance(error,RuntimeError):
            print(str(error),file=sys.stderr)
        sys.exit(1)
