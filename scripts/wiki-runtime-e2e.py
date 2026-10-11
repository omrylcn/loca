#!/usr/bin/env python3
"""Opt-in real shipped Wiki runner gate; no production identities or enablement.

Native drafting, lease renewal and lost-publication-response recovery use the
shipped runner, not a model oracle. Routing and prolonged soak remain separate.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import socket
import sqlite3
import subprocess
import sys
import tempfile
import urllib.error
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("wiki_protocol",ROOT/"scripts/wiki-protocol-e2e.py")
protocol = importlib.util.module_from_spec(spec)
spec.loader.exec_module(protocol)
from wiki_runtime import WikiJournal, WikiRunner, ShellTransport, ProcessWorker


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runtime",choices=("codex","claude"),required=True)
    parser.add_argument("--binary")
    parser.add_argument("--node")
    args = parser.parse_args()
    # Missing native capability fails before creating any disposable Building.
    native = ProcessWorker(args.runtime,args.binary,args.node)
    with tempfile.TemporaryDirectory(prefix="loca-wiki-runtime-e2e-") as directory:
        work = Path(directory)
        db = work/"building.sqlite3"
        with socket.socket() as socket_:
            socket_.bind(("127.0.0.1",0))
            port = socket_.getsockname()[1]
        base = "http://127.0.0.1:"+str(port)
        env = {key:value for key,value in os.environ.items()
               if not key.startswith(("LOCA_","ROOM_","DAVET_","ADMIN_","CARE_","SMASTER_"))}
        server_env = dict(env,HOME=str(work),ADMIN_TOKEN=protocol.ADMIN,BIND_ADDR="127.0.0.1",
            PORT=str(port),DB_PATH=str(db),REQUIRE_INVITE="1",REQUIRE_SESSIONS="1",RATE_LIMIT="0")
        with (work/"server.log").open("w") as log:
            server = subprocess.Popen([str(ROOT/"target/debug/room-server")],env=server_env,stdout=log,stderr=log)
            journal = None
            try:
                protocol.eventually(lambda:protocol.http(base,"GET","/health"),"server startup failed")
                admin = {"x-admin-token":protocol.ADMIN}
                protocol.http(base,"POST","/members",{"name":"editor","kind":"agent"},admin)
                davet = protocol.http(base,"POST","/rooms/general/invites",{"name":"editor"},admin)["token"]
                session = protocol.http(base,"POST","/sessions",{"name":"editor","kind":"agent","loca":"general"},
                    {"x-room-token":davet})["session_token"]
                home = work/"editor"
                (home/".loca").mkdir(parents=True)
                identity = home/".loca/editor.env"
                identity.write_text("ROOM_SERVER_URL="+base+"\nLOCA_NAME=editor\nLOCA_SESSION="+session+
                    "\nDAVET_general="+davet+"\n")
                identity.chmod(0o600)
                protocol.http(base,"PUT","/rooms/general/wiki/config",
                    {"editor":"editor","enabled":False,"interval_messages":2},admin)
                protocol.http(base,"POST","/members",{"name":"operator","kind":"user"},admin)
                user_davet = protocol.http(base,"POST","/rooms/general/invites",{"name":"operator"},admin)["token"]
                user_session = protocol.http(base,"POST","/sessions",{"name":"operator","kind":"user","loca":"general"},
                    {"x-room-token":user_davet})["session_token"]
                user_headers = {"x-session-token":user_session}
                editor_headers={"x-session-token":session}
                protocol.http(base,"POST","/rooms/general/wiki/runtime",{"runtime":args.runtime},editor_headers)
                protocol.http(base,"PUT","/rooms/general/wiki/config",
                    {"editor":"editor","enabled":True,"interval_messages":2},admin)
                ids = []
                for text in ("Operator decision: the product database will be PostgreSQL; SQLite remains for local tests.",
                             "Operator constraint: deployments require explicit approval. No deployment is approved now."):
                    ids.append(protocol.http(base,"POST","/rooms/general/messages",
                        {"sender":"operator","sender_type":"user","text":text},user_headers)["id"])
                protocol.eventually(lambda:protocol.http(base,"GET","/rooms/general/wiki/maintenance",headers=admin)
                    ["status"]=="queued","scheduler did not queue sources")
                transport = ShellTransport(base,"general","editor",dict(env,HOME=str(home)))
                accepted = transport.finish
                requests = []
                def lose_response(payload):
                    requests.append(payload)
                    accepted(payload)
                    raise OSError("simulated HTTP response loss after real acceptance")
                transport.finish = lose_response
                journal = WikiJournal(work/"runner.sqlite3",base,"editor")
                runner = WikiRunner("general",journal,transport,native)
                try:
                    runner.run_once()
                    raise RuntimeError("lost response was reported as accepted")
                except OSError:
                    pass
                job = journal.latest("general")
                protocol.require(job["status"]=="publication_pending","validated result was not durable")
                protocol.require(job["review"]["pages"],"native worker produced no reusable knowledge")
                bodies = "\n".join(page["body"] for page in job["review"]["pages"])
                protocol.require("postgresql" in bodies.lower(),"native worker did not preserve the database decision")
                protocol.require(all(set(page["sources"]) <= set(ids) for page in job["review"]["pages"]),
                    "native worker used unseen sources")
                journal.close()
                journal = WikiJournal(work/"runner.sqlite3",base,"editor")
                transport.finish = accepted
                def forbidden(_payload,_renew):
                    raise RuntimeError("recovery incorrectly invoked another model")
                recovered = WikiRunner("general",journal,transport,forbidden).run_once()
                protocol.require(recovered["status"]=="accepted","restart did not recover accepted publication")
                snapshot = transport.read()
                protocol.require(snapshot["revision"]==1 and snapshot["reviewed_through"]==ids[-1],
                    "publication replay duplicated revision or lost cursor coverage")
                protocol.require(WikiRunner("general",journal,transport,forbidden).run_once()["status"]=="idle",
                    "no-input runner is not idle")
                protocol.require(len(protocol.http(base,"GET","/rooms/general/messages",headers=admin))==2,
                    "Wiki runner generated chat spam")
                print(args.runtime+": real shipped worker/runner, renewal, durable result, lost-response restart, exactly one publication, no-input idle PASS")
                print("Recipient routing, capability enablement and prolonged unattended soak remain UNVERIFIED")
            finally:
                if journal: journal.close()
                server.terminate()
                try: server.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    server.kill(); server.wait(timeout=5)


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print("Wiki runtime E2E FAILED (sanitized): "+type(error).__name__,file=sys.stderr)
        if isinstance(error,urllib.error.HTTPError):
            print("HTTP status/path: "+str(error.code)+" "+urlsplit(error.url).path,file=sys.stderr)
        raise SystemExit(1)
