#!/usr/bin/env python3
"""Real restart/HTTP regression gate. Disposable loopback Building; no prod access."""
import argparse
import json
import os
from pathlib import Path
import secrets
import sqlite3
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[1]


def require(ok, label):
    if not ok:
        raise RuntimeError(label)


class Building:
    def __init__(self, work, binary):
        self.work, self.binary = work, binary
        self.db = work / "building.db"
        self.root = secrets.token_hex(32)
        self.process = None

    def start(self):
        port_file = self.work / "port"
        port_file.unlink(missing_ok=True)
        env = {key: value for key, value in os.environ.items()
               if not key.startswith(("LOCA_", "ROOM_", "DAVET_", "ADMIN_", "REQUIRE_", "CARE_"))
               and key not in ("DB_PATH", "PORT", "BOUND_PORT_FILE", "CORS_ALLOW_ORIGIN")}
        env.update(HOME=str(self.work), BIND_ADDR="127.0.0.1", PORT="0",
                   BOUND_PORT_FILE=str(port_file), DB_PATH=str(self.db),
                   ADMIN_TOKEN=self.root, REQUIRE_INVITE="1", REQUIRE_SESSIONS="1", RATE_LIMIT="0")
        self.log = (self.work / "server.log").open("ab")
        self.process = subprocess.Popen([str(self.binary)], env=env, stdout=self.log, stderr=self.log)
        for _ in range(150):
            require(self.process.poll() is None, "fixture server exited")
            if port_file.exists() and port_file.read_text().strip():
                self.base = "http://127.0.0.1:" + port_file.read_text().strip()
                try:
                    if self.request("GET", "/health")[0] == 200:
                        return
                except OSError:
                    pass
            time.sleep(0.02)
        raise RuntimeError("fixture startup timed out")

    def stop(self):
        if self.process and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        if hasattr(self, "log"):
            self.log.close()

    def restart(self):
        self.stop()
        self.start()

    def request(self, method, path, body=None, *, key=None, session=None):
        headers = {"content-type": "application/json"}
        if key:
            headers["x-admin-token"] = key
        if session:
            headers["x-session-token"] = session
        request = urllib.request.Request(self.base + path, method=method, headers=headers,
                                        data=json.dumps(body).encode() if body is not None else None)
        try:
            response = urllib.request.urlopen(request, timeout=5)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            data = response.read().decode()
            try:
                value = json.loads(data)
            except ValueError:
                value = None
            return response.status, value

    def session(self, key):
        status, result = self.request("POST", "/sessions", {"name": "fixture", "kind": "user"}, key=key)
        require(status == 201, "session mint failed")
        return result["session_token"]

    def post(self, room, session, op):
        return self.request("POST", f"/rooms/{room}/messages", {
            "sender": "fixture", "sender_type": "user", "text": "test", "op_id": op,
        }, session=session)


def check(binary):
    with tempfile.TemporaryDirectory(prefix="loca-integrity-e2e-") as directory:
        building = Building(Path(directory), binary)
        try:
            building.start()
            status, smaster = building.request("POST", "/smasters", {"name": "deputy"}, key=building.root)
            require(status == 201, "deputy fixture failed")
            deputy = building.session(smaster["token"])
            master = building.session(building.root)
            building.restart()
            require(building.request("GET", "/profile", session=deputy)[1]["building_role"] == "smaster",
                    "restart elevated deputy to Master")
            require(building.request("POST", "/smasters", {"name": "forbidden"}, session=deputy)[0] == 401,
                    "restarted deputy gained Master action")
            require(building.request("POST", "/pairings?ttl_hours=8760", session=deputy)[0] == 401,
                    "restarted deputy gained Master pairing")
            status, keep = building.post("keep", master, "keep-1")
            require(status == 201, "live room post failed")
            for index in range(5):
                status, high = building.post("sealed", master, f"sealed-{index}")
                require(status == 201, "sealed fixture history failed")
            status, task = building.request("POST", "/rooms/sealed/tasks", {"title": "original", "by": "fixture"}, session=master)
            require(status == 201, "original task fixture failed")
            require(building.request("PUT", "/rooms/sealed/settings", {"archived": True}, session=master)[0] == 200,
                    "archive failed")
            require(building.request("DELETE", "/rooms/sealed", session=master)[0] == 204, "seal failed")
            require(building.request("POST", "/members", {"name": "guest", "kind": "agent"}, key=building.root)[0] == 201,
                    "guest fixture failed")
            for restarted in (False, True):
                if restarted:
                    building.restart()
                for endpoint in ("call", "invites"):
                    require(building.request("POST", f"/rooms/sealed/{endpoint}", {"name": "guest"}, key=building.root)[0] == 409,
                            "davet revived a sealed room")
                require(building.request("PUT", "/rooms/sealed/settings", {"archived": False}, session=master)[0] >= 400,
                        "settings erased permanent seal")
                require(building.post("sealed", master, "forbidden-post")[0] >= 400, "post revived permanent seal")
            status, fresh = building.post("keep", master, "after-seal-restart")
            require(status == 201 and fresh["id"] > high["id"], "restart reused an archived message id")
            status, repeated = building.post("keep", master, "after-seal-restart")
            require(status == 201 and repeated["id"] == fresh["id"], "accepted retry duplicated a message")
            with sqlite3.connect(building.db) as connection:
                require(connection.execute("SELECT sealed_at FROM rooms WHERE room='sealed'").fetchone()[0] is not None,
                        "seal marker lost")
                require(connection.execute("SELECT title FROM tasks WHERE room='sealed' AND id=?", (task["id"],)).fetchone()[0] == "original",
                        "sealed task overwritten")
            old_root = building.root
            building.root = secrets.token_hex(32)
            building.restart()
            require(building.request("GET", "/members", key=old_root)[0] == 401, "old recovery root still authorized")
            require(building.request("GET", "/members", session=master)[0] == 401, "old root session survived rotation")
            require(building.request("GET", "/members", key=building.root)[0] == 200, "new recovery root unavailable")
            print("PASS: deputy authority/restart, root rotation, permanent seal, retained tasks, global IDs, idempotent retry")
        finally:
            building.stop()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/room-server")
    check(parser.parse_args().binary.resolve())
