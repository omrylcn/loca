#!/usr/bin/env python3
"""Unattended native Wiki runner + production Codex reader on loopback only.

Use --soak-seconds 10800 for the duration gate; short runs are preflight only.
No installed identity, live room, service or production database is touched.
"""
import argparse
from contextlib import contextmanager
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import socket
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
import urllib.error
import uuid
from urllib.parse import urlsplit

ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/"skill/agent-room"))
spec=importlib.util.spec_from_file_location("wiki_protocol",ROOT/"scripts/wiki-protocol-e2e.py")
protocol=importlib.util.module_from_spec(spec); spec.loader.exec_module(protocol)
from attention_store import AttentionStore
from codex_adapter_v2 import PersistentCodexAdapter,ContextProvider,ConnectShRelay
from runtime_agent import report_runtime_health
from credentials import load_local_env


def require_runtime_receipt(runtime, attention_id):
    """Reject presence or a stale/partial heartbeat as end-to-end ACK proof."""
    protocol.require(isinstance(runtime,dict),"server runtime receipt missing")
    protocol.require(runtime.get("attention_id")==attention_id,"server receipt belongs to another attention")
    protocol.require(runtime.get("ack")=="OK" and runtime.get("wake")=="OK",
                     "server runtime ACK/wake is not OK")
    protocol.require(all(runtime.get(key) is True for key in
                         ("stored","accepted","first_response","final_response","turn_completed","ready")),
                     "server runtime receipt has incomplete milestones")
    protocol.require(runtime.get("seen_at",0)>0 and runtime.get("progress_at",0)>0,
                     "server runtime receipt has no server timestamp")


@contextmanager
def environment(env):
    old=dict(os.environ)
    os.environ.clear(); os.environ.update(env)
    try: yield
    finally: os.environ.clear(); os.environ.update(old)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runtime",choices=("codex","claude"),default="codex")
    parser.add_argument("--binary")
    parser.add_argument("--node")
    parser.add_argument("--codex-bin",default="codex")
    parser.add_argument("--soak-seconds",type=int,default=0)
    parser.add_argument("--interval-seconds",type=int,default=900)
    parser.add_argument("--output",type=Path)
    args=parser.parse_args()
    if not 0<=args.soak_seconds<=86400: parser.error("soak duration must be 0–86400 seconds")
    if not 60<=args.interval_seconds<=3600: parser.error("probe interval must be 60–3600 seconds")
    source_sha=subprocess.check_output(["git","rev-parse","HEAD"],cwd=ROOT,text=True).strip()
    dirty=bool(subprocess.check_output(["git","status","--porcelain","--untracked-files=no"],cwd=ROOT,text=True).strip())
    if args.soak_seconds and dirty: parser.error("duration gate requires a committed, clean candidate")
    native_home=Path.home()
    env={key:value for key,value in os.environ.items() if not key.startswith(("LOCA_","ROOM_","DAVET_","ADMIN_","CARE_","SMASTER_"))}
    env.setdefault("CODEX_HOME",str(native_home/".codex"))
    env.setdefault("CLAUDE_CONFIG_DIR",str(native_home/".claude"))
    started=time.monotonic()
    phase="startup"; rounds=0; restarts=0; runtime_receipts=[]
    with tempfile.TemporaryDirectory(prefix="loca-wiki-autonomy-") as directory:
        work=Path(directory); db=work/"building.db"
        # The shared Cargo target may be rebuilt by another worktree. Pin the
        # actual binary for the entire test rather than silently testing a mix.
        binary=work/"room-server"
        shutil.copy2(ROOT/"target/debug/room-server",binary)
        binary_sha=hashlib.sha256(binary.read_bytes()).hexdigest()
        with socket.socket() as sock: sock.bind(("127.0.0.1",0)); port=sock.getsockname()[1]
        base=f"http://127.0.0.1:{port}"
        admin={"x-admin-token":protocol.ADMIN}
        server_env=dict(env,HOME=str(work),ADMIN_TOKEN=protocol.ADMIN,BIND_ADDR="127.0.0.1",PORT=str(port),
                        DB_PATH=str(db),REQUIRE_INVITE="1",REQUIRE_SESSIONS="1",RATE_LIMIT="0")
        runner=server=adapter=listener=editor_listener=None
        logs=[]
        def launch_server():
            log=(work/"server.log").open("a"); logs.append(log)
            process=subprocess.Popen([str(binary)],env=server_env,stdout=log,stderr=log)
            protocol.eventually(lambda:protocol.http(base,"GET","/health"),"server startup failed")
            return process
        def launch_runner():
            log=(work/"runner.log").open("a"); logs.append(log)
            command=[sys.executable,str(ROOT/"skill/agent-room/wiki_runtime.py"),"--server",base,"--name","editor",
                "--rooms","auto","--journal",str(work/"journal.db"),"--runtime",args.runtime]
            if args.binary: command += ["--binary",args.binary]
            if args.node: command += ["--node",args.node]
            return subprocess.Popen(command,env=editor_env,stdout=log,stderr=log)
        def halt(process):
            if process and process.poll() is None:
                process.terminate()
                try: process.wait(timeout=8)
                except subprocess.TimeoutExpired: process.kill(); process.wait(timeout=3)
        def progress(ok=False,error=None):
            result={"runtime":args.runtime,"elapsed_seconds":round(time.monotonic()-started,2),
                "phase":phase,"production_codex_reader_replies":rounds,
                "server_runner_reader_restarts":restarts,"ok":ok,"source_sha":source_sha,
                "source_dirty":dirty,"server_binary_sha256":binary_sha,
                "requested_seconds":args.soak_seconds,"interval_seconds":args.interval_seconds,
                "duration_gate":ok and not dirty and args.soak_seconds>=10800 and time.monotonic()-started>=args.soak_seconds,
                "server_delivery_ack_verified":ok and len(runtime_receipts)==rounds and rounds>0,
                "server_runtime_receipts":runtime_receipts}
            if error:
                result["failure_class"]=type(error).__name__
                if isinstance(error,urllib.error.HTTPError):
                    result["http_status"]=error.code
                    result["http_path"]=urlsplit(error.url).path
            if args.output:
                temp=args.output.with_suffix(".tmp")
                temp.write_text(json.dumps(result,indent=2)+"\n"); temp.replace(args.output)
            return result
        try:
            progress()
            server=launch_server()
            identities={}
            davets={}
            for name,kind in (("editor","agent"),("reader","agent"),("operator","user")):
                member=protocol.http(base,"POST","/members",{"name":name,"kind":kind},admin)
                davet=protocol.http(base,"POST","/rooms/general/invites",{"name":name},admin)["token"]
                davets[name]=davet
                session=protocol.http(base,"POST","/sessions",{"name":name,"kind":kind,"loca":"general"},{"x-room-token":davet})["session_token"]
                home=work/name; (home/".loca").mkdir(parents=True,mode=0o700)
                identity=home/".loca"/(name+".env")
                identity.write_text("ROOM_SERVER_URL="+base+"\nLOCA_NAME="+name+"\nLOCA_MEMBERSHIP="+member["token"]+
                    "\nLOCA_SESSION="+session+"\nDAVET_general="+davet+"\n")
                identity.chmod(0o600)
                identities[name]=(dict(env,HOME=str(home),LOCA_ENV=str(identity),LOCA_NAME=name),{"x-session-token":session})
            editor_env,editor_headers=identities["editor"]
            reader_env,reader_headers=identities["reader"]
            operator_headers=identities["operator"][1]
            protocol.http(base,"PUT","/rooms/general/wiki/config",{"editor":"editor","enabled":False,"interval_messages":2},admin)
            protocol.http(base,"POST","/rooms/general/wiki/context",{"slugs":["overview"],"reason":"Need the database policy for my work"},reader_headers)
            runner=launch_runner()
            def enable():
                protocol.http(base,"PUT","/rooms/general/wiki/config",{"editor":"editor","enabled":True,"interval_messages":2},admin)
                return True
            protocol.eventually(enable,"native readiness did not enable automation",seconds=60)
            def say(text):
                payload={"sender":"operator","sender_type":"user","text":text,"op_id":"wiki-soak-"+uuid.uuid4().hex}
                try:
                    return protocol.http(base,"POST","/rooms/general/messages",payload,operator_headers)["id"]
                except urllib.error.HTTPError as error:
                    if error.code!=401: raise
                    # Browser/operator fixture re-authentication is not agent
                    # recovery. Sessions are intentionally memory-only. Agent
                    # listeners below must refresh via the real Lobby path.
                    session=protocol.http(base,"POST","/sessions",{"name":"operator","kind":"user","loca":"general"},{"x-room-token":davets["operator"]})["session_token"]
                    operator_headers["x-session-token"]=session
                    return protocol.http(base,"POST","/rooms/general/messages",payload,operator_headers)["id"]
            say("Operator decision: PostgreSQL is the product database. SQLite is used only in tests.")
            last=say("Operator constraint: deploy requires explicit approval. No deployment is approved.")
            def published(after):
                # The production adapter has its own continuously running
                # event loop. The in-process harness must keep that loop alive
                # while a separate native Wiki worker is drafting, too.
                if adapter is not None:
                    adapter.cycle()
                value=protocol.http(base,"GET","/rooms/general/wiki",headers=admin)
                return value if value["reviewed_through"]>=after and value["revision"]>0 else False
            first=protocol.eventually(lambda:published(last),"unattended native publication failed",seconds=240)
            protocol.require("postgresql" in json.dumps(first).lower(),"decision was not published")
            protocol.require(runner.poll() is None,"Wiki runner exited after publication")
            store=AttentionStore(work/"reader-ledger.db"); inbox=work/"inbox.jsonl"; inbox.touch()
            helper=ROOT/"skill/agent-room/connect.sh"
            def launch_listener(name):
                log=(work/(name+"-listener.log")).open("a"); logs.append(log)
                command=[sys.executable,str(ROOT/"skill/agent-room/listen.py"),
                    base.replace("http://","ws://")+f"/ws?room=general&name={name}&type=agent&filter=mentions&turn_max=1",
                    str(work/(name+"-messages.jsonl")),"--skip-own",name,
                    "--cursor",str(work/(name+"-cursor.json"))]
                if name=="reader": command += ["--turn-log",str(inbox)]
                process=subprocess.Popen(command,env=identities[name][0],stdout=log,stderr=log)
                protocol.eventually(lambda:any(m.get("name")==name for m in
                    protocol.http(base,"GET","/rooms/general/members",headers=admin)),"reader listener was not online")
                return process
            def launch_adapter():
                value=PersistentCodexAdapter(store=store,inbox=inbox,identity="reader",workdir=work,
                    codex_bin=args.codex_bin,relay_mode="live",relay=ConnectShRelay(helper),
                    context_provider=ContextProvider(helper,"reader"),health_file=work/"reader-health.json")
                value.initialize()
                return value
            editor_listener=launch_listener("editor")
            listener=launch_listener("reader")
            with environment(reader_env):
                adapter=launch_adapter()
                def question(wanted):
                    nonlocal rounds,phase
                    phase="native-response"; progress()
                    mid=say("@reader Answer the current database choice using the supplied Wiki. Do not use tools. One short answer.")
                    deadline=time.monotonic()+180
                    last_reply=None
                    while time.monotonic()<deadline:
                        adapter.cycle()
                        with sqlite3.connect(db) as connection:
                            replies=connection.execute("SELECT id,text FROM messages WHERE room='general' AND sender='reader' AND id>?",(mid,)).fetchall()
                        if any(wanted in row[1].lower() for row in replies) and not adapter.active_turns:
                            with sqlite3.connect(work/"reader-ledger.db") as connection:
                                unresolved=connection.execute("SELECT COUNT(*) FROM attentions WHERE reply_required=1 AND terminal_status IS NULL AND (final_response_at_ms IS NULL OR turn_completed_at_ms IS NULL)").fetchone()[0]
                            if unresolved==0:
                                last_reply=max(row[0] for row in replies); break
                        time.sleep(.1)
                    else: raise RuntimeError("production adapter did not relay the native Wiki-informed response")
                    with sqlite3.connect(db) as connection:
                        receipts=connection.execute("SELECT COUNT(*) FROM wiki_context_receipts WHERE room='general'").fetchone()[0]
                    protocol.require(receipts>0,"native context acceptance was not durably recorded")
                    protocol.require(adapter.health.get("wiki_context") in ("RECORDED","CURRENT"),
                        "current reader context receipt is unavailable or still pending")
                    phase="verify-unique-native-turn"
                    with sqlite3.connect(work/"reader-ledger.db") as connection:
                        rows=connection.execute("SELECT attention_id,turn_id FROM attentions WHERE delivery_id=?",
                                                (f"general:{mid}",)).fetchall()
                        protocol.require(len(rows)==1 and bool(rows[0][1]),"direct call has no unique native turn")
                        attention_id,turn_id=rows[0]
                        phase="verify-unique-final-relay"
                        finals=connection.execute("SELECT COUNT(*) FROM relay_items WHERE turn_id=? AND relay_kind='final' AND status='accepted'",
                                                  (turn_id,)).fetchone()[0]
                        protocol.require(finals==1,"native turn has no unique accepted final relay")
                    # Use the shipped supervisor reporting function and read
                    # back the server projection, not just local health JSON.
                    phase="verify-server-health-submit"
                    load_local_env("reader")
                    report_runtime_health(work/"reader-health.json",True)
                    residents=protocol.http(base,"GET","/residents",headers=admin)
                    runtime=next((row.get("runtime") for row in residents if row.get("name")=="reader"),None)
                    phase="verify-server-health-readback"
                    runtime_receipts.append({"delivery_id":f"general:{mid}","turn_id":turn_id,
                                             "reply_message_id":last_reply,"runtime":runtime})
                    require_runtime_receipt(runtime,attention_id)
                    rounds+=1
                    # Question + response are legitimate Wiki input too. Drain
                    # their threshold review before asserting idle behaviour.
                    protocol.eventually(lambda:published(last_reply),"post-reply review did not settle",seconds=240)
                    phase="quiet-observation"; progress()
                    print(f"{args.runtime}: probe {rounds}: real listener -> native Codex -> accepted reply + final lifecycle PASS",flush=True)
                for count in (1,2):
                    if count==2:
                        say("Operator correction: product database is now MariaDB, replacing PostgreSQL. SQLite remains test-only.")
                        last=say("The existing deployment approval constraint is unchanged.")
                        protocol.eventually(lambda:published(last),"second native publication failed",seconds=240)
                    question("postgresql" if count==1 else "mariadb")
                    with sqlite3.connect(db) as connection:
                        receipts=connection.execute("SELECT COUNT(*) FROM wiki_context_receipts WHERE room='general'").fetchone()[0]
                    protocol.require(receipts>=count,"native context acceptance was not durably recorded")
                    print(f"{args.runtime}: unattended publication {count} -> real Codex turn -> real reply relay -> context receipt PASS",flush=True)
                # Restart every owner with the same durable state, then prove
                # a real response. A live PID alone is not recovery evidence.
                adapter.close(); adapter=None
                phase="durable-restart"; progress()
                halt(listener); listener=None; halt(runner); halt(editor_listener); editor_listener=None; halt(server)
                server=launch_server(); editor_listener=launch_listener("editor")
                runner=launch_runner(); listener=launch_listener("reader")
                adapter=launch_adapter(); restarts+=1
                question("mariadb")
                next_probe=time.monotonic()+args.interval_seconds
                deadline=started+args.soak_seconds
                revision=protocol.http(base,"GET","/rooms/general/wiki",headers=admin)["revision"]
                while time.monotonic()<deadline:
                    protocol.require(runner.poll() is None and server.poll() is None and listener.poll() is None and editor_listener.poll() is None,"unattended process died")
                    adapter.cycle()
                    snapshot=protocol.http(base,"GET","/rooms/general/wiki",headers=admin)
                    protocol.require(snapshot["revision"]==revision,"idle Wiki generated another publication")
                    if time.monotonic()>=next_probe:
                        # A changing source checkpoint forces fresh maintenance;
                        # repeat calls exercise persisted reader state, not only
                        # the first successful turn after setup.
                        say(f"Operator checkpoint {rounds}: MariaDB remains the product database. No deployment approval.")
                        last=say(f"Checkpoint {rounds} verified: the Wiki must keep test-only SQLite distinct from production.")
                        protocol.eventually(lambda:published(last),"periodic publication failed",seconds=240)
                        question("mariadb")
                        revision=protocol.http(base,"GET","/rooms/general/wiki",headers=admin)["revision"]
                        next_probe=time.monotonic()+args.interval_seconds
                    progress()
                    time.sleep(min(10,max(0,deadline-time.monotonic())))
                # Finish with another full restart and a response, not an idle
                # grace period that could conceal the last failed delivery.
                if args.soak_seconds:
                    adapter.close(); adapter=None
                    phase="final-durable-restart"; progress()
                    halt(listener); listener=None; halt(runner); halt(editor_listener); editor_listener=None; halt(server)
                    server=launch_server(); editor_listener=launch_listener("editor")
                    runner=launch_runner(); listener=launch_listener("reader")
                    adapter=launch_adapter(); restarts+=1; question("mariadb")
                adapter.close(); adapter=None
            phase="complete"
            result=progress(ok=True)
            print(json.dumps(result),flush=True)
        except Exception as error:
            progress(error=error)
            raise
        finally:
            if adapter: adapter.close()
            halt(listener)
            halt(runner); halt(editor_listener); halt(server)
            for log in logs: log.close()


if __name__=="__main__":
    try: main()
    except Exception as error:
        print("Wiki autonomy gate FAILED (sanitized): "+type(error).__name__,file=sys.stderr)
        raise SystemExit(1)
