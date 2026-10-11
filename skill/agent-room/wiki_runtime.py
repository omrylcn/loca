#!/usr/bin/env python3
"""Opt-in Wiki maintenance runner, independent of room chat turns.

Native readiness permits an operator to enable this candidate's room policy;
it does not certify long-soak or rollout approval. This runner does not grant a
Wiki role, enable a room or impersonate an editor. Publication recovery always
replays the original epoch/payload.
"""
from copy import deepcopy
import argparse
import json
import os
import re
from pathlib import Path
import signal
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
from urllib.parse import urlsplit

from portable_lock import lock_file
from wiki_coordinator import WikiCoordinator


class WorkerUncertain(RuntimeError):
    """Inference may have been accepted; never automatically repeat it."""


class WikiTransportError(OSError):
    def __init__(self, status):
        self.status = status
        super().__init__("Wiki authenticated transport rejected request")


class WikiJournal:
    """Single-writer durable dispatch/result/receipt journal; no credentials."""
    def __init__(self, path, server, name):
        origin = urlsplit(server)
        if (origin.scheme not in ("http","https") or not origin.hostname
                or origin.username or origin.password or origin.query or origin.fragment
                or origin.path not in ("","/") or not name):
            raise ValueError("Wiki journal requires an exact server origin and identity")
        path = Path(path)
        if path.is_symlink() or path.parent.is_symlink():
            raise ValueError("Wiki journal must not be a symlink")
        path.parent.mkdir(parents=True,exist_ok=True,mode=0o700)
        self.connection = None
        self.lock = None
        try:
            flags = os.O_RDWR | os.O_CREAT | getattr(os,"O_NOFOLLOW",0)
            self.lock = os.open(str(path)+".lock",flags,0o600)
            lock_file(self.lock,blocking=False)
            fd = os.open(path,flags,0o600)
            os.close(fd)
            self.connection = sqlite3.connect(path)
            # A typo must not add journal tables to a Building database (or
            # change the permissions of an unrelated pre-existing file).
            tables = {row[0] for row in self.connection.execute(
                "SELECT name FROM sqlite_master WHERE type='table'")}
            application_id = self.connection.execute("PRAGMA application_id").fetchone()[0]
            if tables and application_id != 0x4c574a31:
                raise ValueError("Existing database is not a Wiki execution journal")
            if tables:
                rows = self.connection.execute("SELECT server,name FROM identity").fetchall()
                if rows != [(server.rstrip("/"),name)]:
                    raise ValueError("Wiki journal belongs to another origin or identity")
            path.chmod(0o600)
            self.connection.execute("PRAGMA synchronous=FULL")
            self.connection.executescript("""
                BEGIN;
                PRAGMA application_id=1280789041;
                CREATE TABLE IF NOT EXISTS identity(server TEXT NOT NULL,name TEXT NOT NULL);
                CREATE TABLE IF NOT EXISTS jobs(
                    id INTEGER PRIMARY KEY, room TEXT NOT NULL, batch_id INTEGER NOT NULL,
                    epoch INTEGER NOT NULL, status TEXT NOT NULL, batch TEXT NOT NULL,
                    snapshot TEXT NOT NULL, review TEXT, receipt TEXT, reason TEXT,
                    UNIQUE(room,batch_id,epoch));
                CREATE UNIQUE INDEX IF NOT EXISTS active_room ON jobs(room)
                    WHERE status IN ('dispatch_intent','publication_pending','blocked');
                CREATE TABLE IF NOT EXISTS routes(job_id INTEGER NOT NULL,room TEXT NOT NULL,
                    target TEXT NOT NULL,payload TEXT NOT NULL,status TEXT NOT NULL DEFAULT 'pending',
                    PRIMARY KEY(job_id,target));
            """)
            with self.connection:
                rows = self.connection.execute("SELECT server,name FROM identity").fetchall()
                binding = (server.rstrip("/"),name)
                if rows and rows != [binding]:
                    raise ValueError("Wiki journal belongs to another origin or identity")
                if not rows:
                    self.connection.execute("INSERT INTO identity VALUES(?,?)",binding)
        except BaseException:
            self.close()
            raise

    def close(self):
        if self.connection is not None:
            self.connection.close()
            self.connection = None
        if self.lock is not None:
            os.close(self.lock)
            self.lock = None

    def latest(self, room):
        self.connection.row_factory = sqlite3.Row
        row = self.connection.execute("SELECT * FROM jobs WHERE room=? ORDER BY id DESC LIMIT 1",
                                      (room,)).fetchone()
        if row is None:
            return None
        result = dict(row)
        for key in ("batch","snapshot","review","receipt"):
            result[key] = json.loads(result[key]) if result[key] is not None else None
        return result

    def intent(self, room, batch, snapshot):
        batch_json,snapshot_json = json.dumps(batch),json.dumps(snapshot)
        if len((batch_json+snapshot_json).encode()) > 2*1024*1024:
            raise ValueError("Wiki journal input exceeds durable budget")
        with self.connection:
            pending = self.connection.execute("SELECT COUNT(*) FROM jobs WHERE status!='accepted'").fetchone()[0]
            if pending >= 16:
                raise ValueError("Wiki journal needs reconciliation before more work")
            self.connection.execute("""INSERT INTO jobs(room,batch_id,epoch,status,batch,snapshot)
                VALUES(?,?,?,'dispatch_intent',?,?)""",
                (room,batch["lease"]["batch_id"],batch["lease"]["epoch"],
                 batch_json,snapshot_json))
        return self.latest(room)

    def result(self, job, review):
        with self.connection:
            changed = self.connection.execute("""UPDATE jobs SET status='publication_pending',review=?
                WHERE id=? AND status='dispatch_intent'""",(json.dumps(review),job["id"])).rowcount
            if changed != 1:
                raise RuntimeError("Wiki dispatch state changed before storing result")
            for route in review.get("routes",[]):
                self.connection.execute("INSERT INTO routes(job_id,room,target,payload) VALUES(?,?,?,?)",
                    (job["id"],job["room"],route["target"],json.dumps(route)))

    def pending_routes(self, room):
        return self.connection.execute("""SELECT r.job_id,r.target,r.payload FROM routes r JOIN jobs j ON j.id=r.job_id
            WHERE r.room=? AND r.status='pending' AND j.status='accepted' ORDER BY r.job_id,r.target""",(room,)).fetchall()

    def routed(self, job_id, target):
        with self.connection:
            self.connection.execute("DELETE FROM routes WHERE job_id=? AND target=?",(job_id,target))

    def accepted(self, job, receipt):
        with self.connection:
            # Accepted publication is authoritative on the server. Keep bounded
            # receipt metadata, not a permanent duplicate of private chat.
            changed = self.connection.execute("""UPDATE jobs SET status='accepted',receipt=?,
                batch=?,snapshot='{}',review=NULL
                WHERE id=? AND status='publication_pending'""",
                (json.dumps(receipt),json.dumps({"lease":job["batch"]["lease"]}),job["id"])).rowcount
            if changed != 1:
                raise RuntimeError("Wiki publication state changed before recording receipt")
            self.connection.execute("""DELETE FROM jobs WHERE status='accepted' AND id NOT IN
                (SELECT id FROM jobs WHERE status='accepted' ORDER BY id DESC LIMIT 128)
                AND id NOT IN (SELECT job_id FROM routes)""")

    def block(self, job, reason):
        with self.connection:
            self.connection.execute("UPDATE jobs SET status='blocked',reason=? WHERE id=?",
                                    (reason,job["id"]))


class WikiRunner:
    def __init__(self, room, journal, transport, worker):
        self.room,self.journal,self.transport,self.worker = room,journal,transport,worker

    def route_pending(self):
        for job_id,target,payload in self.journal.pending_routes(self.room):
            self.transport.offer(json.loads(payload))
            self.journal.routed(job_id,target)

    def publish(self, job):
        # Finish itself checks current principal and fences cancellation/epoch.
        # Do not renew first: an accepted, response-lost finish must be retried
        # even after expiry, using its original idempotent receipt identity.
        payload = {"lease":deepcopy(job["batch"]["lease"]),"review":deepcopy(job["review"])}
        receipt = self.transport.finish(payload)
        WikiCoordinator.verify_receipt(receipt,job["snapshot"])
        self.journal.accepted(job,receipt)
        self.route_pending()
        return {"status":"accepted","batch_id":job["batch_id"],"revision":receipt["revision"]}

    def run_once(self):
        self.route_pending()
        job = self.journal.latest(self.room)
        if job and job["status"] == "publication_pending":
            return self.publish(job)
        if job and job["status"] in ("dispatch_intent","blocked"):
            # No exactly-once claim about inference is possible after loss of
            # its response. Persist the uncertainty; a restart cannot hide it.
            if job["status"] == "dispatch_intent":
                self.journal.block(job,"native dispatch outcome unknown; reconciliation required")
            return {"status":"blocked","reason":job["reason"] or "native dispatch requires reconciliation"}
        if self.worker is None:
            return {"status":"blocked","reason":"isolated Wiki worker unavailable"}
        batch = self.transport.claim()
        if batch is None:
            return {"status":"idle"}
        if job and job["batch"]["lease"] == batch["lease"]:
            return {"status":"accepted","batch_id":job["batch_id"],"revision":job["receipt"]["revision"]}
        snapshot = self.transport.read()
        if hasattr(self.transport,"requests"):
            requests = self.transport.requests().get("requests",[])
            batch["context_requests"] = requests
        coordinator = WikiCoordinator(self.room,None,None,None,None)
        payload = coordinator.prepare(batch,snapshot)
        payload["context_requests"] = deepcopy(batch.get("context_requests",[]))
        if len(json.dumps(payload,ensure_ascii=False).encode())>coordinator.max_input_bytes:
            raise ValueError("Wiki context requests exceeded worker input budget")
        lease = deepcopy(batch["lease"])
        def renew():
            value = self.transport.renew(lease)
            if (not isinstance(value,dict) or type(value.get("lease_expires_at")) is not int
                    or value["lease_expires_at"] <= int(time.time()*1000)):
                raise PermissionError("Wiki worker lease no longer valid")
        renew()
        job = self.journal.intent(self.room,batch,snapshot)
        try:
            review = coordinator.validate_review(self.worker(payload,renew),batch,snapshot)
        except Exception:
            self.journal.block(job,"native worker failed or returned invalid review")
            return {"status":"blocked","reason":"native worker outcome requires reconciliation"}
        self.journal.result(job,review)
        return self.publish(self.journal.latest(self.room))


class ShellTransport:
    """Only the shipped credential boundary handles sessions/davets."""
    def __init__(self, server, room, name, env):
        self.server,self.room,self.name,self.env = server,room,name,env

    def call(self, command, payload=None):
        with tempfile.TemporaryDirectory(prefix="loca-wiki-request-") as directory:
            args = ["bash",str(Path(__file__).with_name("connect.sh")),command,
                    self.server,self.room,self.name]
            if payload is not None:
                path = Path(directory)/"body.json"
                fd = os.open(path,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
                with os.fdopen(fd,"w") as body:
                    json.dump(payload,body)
                args.append(str(path))
            value = subprocess.run(args,env=self.env,capture_output=True,text=True,timeout=35)
            if value.returncode:
                # Never echo HTTP bodies/helper output: these may contain data.
                matched = re.search(r"Wiki request rejected \(HTTP ([0-9]{3})\)",value.stderr)
                raise WikiTransportError(int(matched[1]) if matched else None)
            result = json.loads(value.stdout)
            if result is not None and not isinstance(result,dict):
                raise ValueError("Wiki transport returned an invalid envelope")
            return result

    def claim(self): return self.call("wiki-claim")
    def read(self): return self.call("wiki")
    def renew(self, lease): return self.call("wiki-renew",lease)
    def finish(self, payload): return self.call("wiki-finish",payload)
    def ready(self, runtime): return self.call("wiki-ready",{"runtime":runtime})
    def offer(self, payload): return self.call("wiki-offer",payload)
    def requests(self): return self.call("wiki-requests")


def discover_rooms(server, name):
    """Canonical invitations from membership; never reverse normalized env keys."""
    from credentials import load_local_env, validate_server_scope
    load_local_env(name)
    validate_server_scope(server)
    membership = os.environ.get("LOCA_MEMBERSHIP","")
    if not membership:
        raise ValueError("Wiki discovery requires this identity's Building membership")
    request = urllib.request.Request(server.rstrip("/")+"/whoami",headers={"x-room-token":membership})
    with urllib.request.urlopen(request,timeout=5) as response:
        value = json.load(response)
    if value.get("name")!=name or value.get("kind")!="member":
        raise ValueError("Wiki discovery identity mismatch")
    rooms = value.get("locas")
    if (not isinstance(rooms,list) or len(rooms)>7 or len(set(rooms))!=len(rooms)
            or any(not isinstance(r,str) or not r or len(r)>128 for r in rooms)):
        raise ValueError("Wiki discovery exceeded authorized room budget")
    return rooms


class ProcessWorker:
    """Worker cannot inherit Loca transport; renew/check while it runs."""
    def __init__(self, runtime, binary=None, node=None, timeout=180):
        from wiki_worker import preflight
        preflight(runtime,binary,node)
        self.runtime,self.binary,self.node,self.timeout = runtime,binary,node,timeout

    def __call__(self, payload, renew):
        args = [sys.executable,str(Path(__file__).with_name("wiki_worker.py")),
                "--runtime",self.runtime,"--timeout-seconds",str(self.timeout)]
        if self.binary: args += ["--binary",self.binary]
        if self.node: args += ["--node",self.node]
        # Parent process has room credentials for HTTP; child deliberately does
        # not. Native login source paths are metadata, never Loca credentials.
        from wiki_worker import safe_parent_environment, stop_process
        child = subprocess.Popen(args,env=safe_parent_environment(),stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,text=True,start_new_session=True)
        outcome = {}
        def communicate():
            try:
                outcome["streams"] = child.communicate(json.dumps(payload))
            except Exception:
                outcome["failed"] = True
        thread = threading.Thread(target=communicate,daemon=True)
        thread.start()
        deadline,next_renew = time.monotonic()+self.timeout+10,time.monotonic()+30
        try:
            while thread.is_alive():
                thread.join(timeout=.2)
                if time.monotonic() >= deadline:
                    raise WorkerUncertain("Wiki worker timed out")
                if time.monotonic() >= next_renew:
                    renew()
                    next_renew = time.monotonic()+30
            if child.returncode or outcome.get("failed"):
                raise WorkerUncertain("Wiki native worker did not complete")
            output = outcome["streams"][0]
            if len(output.encode()) > 1200000:
                raise WorkerUncertain("Wiki worker output exceeded budget")
            return json.loads(output)
        finally:
            stop_process(child,group=True)
            thread.join(timeout=2)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server",required=True)
    parser.add_argument("--name",required=True)
    parser.add_argument("--rooms",default="auto",help="auto: discover only this identity's canonical invitations")
    parser.add_argument("--journal",required=True,type=Path)
    parser.add_argument("--runtime",choices=("codex","claude"),required=True)
    parser.add_argument("--binary")
    parser.add_argument("--node")
    parser.add_argument("--once",action="store_true")
    args = parser.parse_args()
    rooms = [] if args.rooms=="auto" else args.rooms.split(",")
    if len(rooms)>7 or any(not room or len(room)>128 for room in rooms) or len(set(rooms))!=len(rooms):
        parser.error("supply one to seven unique room names")
    stop = threading.Event()
    def halt(_sig,_frame):
        stop.set()
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM,halt)
    signal.signal(signal.SIGINT,halt)
    journal = WikiJournal(args.journal,args.server,args.name)
    try:
        worker = ProcessWorker(args.runtime,args.binary,args.node)
        runners = {room:WikiRunner(room,journal,ShellTransport(args.server,room,args.name,os.environ.copy()),worker)
                   for room in rooms}
        failures = {room:0 for room in rooms}
        next_attempt = {room:0.0 for room in rooms}
        disabled,reported = set(),{}
        def report(room, result):
            line = json.dumps({"room":room,**result},sort_keys=True)
            if reported.get(room) != line:
                print(line,flush=True)
                reported[room] = line
        while not stop.is_set():
            if args.rooms=="auto":
                discovered = discover_rooms(args.server,args.name)
                for room in list(runners):
                    if room not in discovered:
                        del runners[room]
                        disabled.discard(room)
                for room in discovered:
                    if room not in runners:
                        runners[room] = WikiRunner(room,journal,ShellTransport(args.server,room,args.name,os.environ.copy()),worker)
                        failures[room],next_attempt[room] = 0,0.0
                rooms = discovered
            for runner in runners.values():
                room = runner.room
                if room in disabled or time.monotonic()<next_attempt[room]:
                    continue
                try:
                    runner.transport.ready(args.runtime)
                    snapshot = runner.transport.read()
                    if snapshot.get("editor_name") != args.name or not snapshot.get("enabled"):
                        report(room,{"status":"standby"})
                        continue
                    result = runner.run_once()
                    report(room,result)
                    failures[room] = 0
                    if result["status"] == "blocked":
                        disabled.add(room)
                except WikiTransportError as error:
                    if error.status==404:
                        report(room,{"status":"standby","reason":"Wiki not assigned"})
                        continue
                    failures[room] += 1
                    report(room,{"status":"access_or_transport_failed","http_status":error.status,"attempt":failures[room]})
                    if error.status in (401,403,409) or failures[room]>=3:
                        disabled.add(room)
                    next_attempt[room] = time.monotonic()+30
                except (OSError,ValueError,subprocess.SubprocessError):
                    failures[room] += 1
                    report(room,{"status":"transport_failed","attempt":failures[room]})
                    if failures[room] >= 3:
                        disabled.add(room)
                    next_attempt[room] = time.monotonic()+min(60,15*(2**failures[room]))
            # One failed room must not strand the editor's other authorized
            # rooms. All-blocked is terminal; repeated idle is not a log storm.
            if rooms and len(disabled) == len(rooms):
                return 2
            if args.once:
                return 1 if disabled or any(failures.values()) else 0
            stop.wait(15)
        return 0
    finally:
        journal.close()


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except KeyboardInterrupt:
        raise SystemExit(130)
    except (OSError,ValueError,RuntimeError,sqlite3.Error):
        print("Wiki runner blocked; check login, ownership and private journal (sanitized)",file=sys.stderr)
        raise SystemExit(2)
