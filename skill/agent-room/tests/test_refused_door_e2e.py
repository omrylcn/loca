"""A loca whose door says no is not asked again every two seconds.

Against the real server, with the fewest seats a loca can have (two), in a
sandbox Building where the shared key seats anyone (in an invite-only Building
an invite is a seat, so a full loca refuses the invite and nobody is turned
away at the socket; there the socket refusal meets a member only when the
Master or a Smaster sits in). While both seats are held the listener is told ``loca full`` and the socket is closed. It
must say so once (every printed line can wake a model) and ask again less
and less often, and it must still take the seat by itself once it is free.
"""

import os
import subprocess
import tempfile
import time
import unittest
from pathlib import Path

from test_listener_backfill_e2e import SERVER_BIN, free_port
from test_session_expiry_e2e import call, master

SKILL_DIR = Path(__file__).resolve().parents[1]
REPO_ROOT = SKILL_DIR.parents[1]
MASTER = "expiry-e2e-master"
SANDBOX_KEY = "refused-door-sandbox-key"


class RefusedDoorEndToEndTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not SERVER_BIN.is_file():
            subprocess.run(
                ["cargo", "build", "-q", "-p", "server"],
                cwd=REPO_ROOT,
                check=True,
                timeout=600,
            )

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.port = free_port()
        self.base = f"http://127.0.0.1:{self.port}"
        env = dict(os.environ)
        env.update(
            {
                "PORT": str(self.port),
                "BIND_ADDR": "127.0.0.1",
                "DB_PATH": str(self.root / "loca.sqlite3"),
                "ADMIN_TOKEN": MASTER,
                "ROOM_TOKEN": SANDBOX_KEY,
                # The server clamps seats per loca to 2..=32.
                "LOCA_CAPACITY": "2",
                "RUST_LOG": "warn",
            }
        )
        env.pop("REQUIRE_INVITE", None)
        server_log = (self.root / "server.log").open("ab")
        self.addCleanup(server_log.close)
        server = subprocess.Popen(
            [str(SERVER_BIN)],
            cwd=REPO_ROOT,
            env=env,
            stdout=server_log,
            stderr=subprocess.STDOUT,
        )
        self.addCleanup(self.stop, server)
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            try:
                if call(self.base, "/health")[0] == 200:
                    break
            except OSError:
                pass
            time.sleep(0.05)
        else:
            self.fail("room-server did not become healthy")
        master(self.base, "/rooms", {"name": "solo"})

    @staticmethod
    def stop(process):
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)

    def listener(self, name):
        """Start a listener for `name` in the loca, holding the sandbox key.
        Returns the process, its output file and its log file."""
        home = self.root / name
        (home / ".loca").mkdir(parents=True)
        env_file = home / ".loca" / f"{name}.env"
        env_file.write_text(
            "\n".join(
                [
                    f"ROOM_SERVER_URL={self.base}",
                    f"LOCA_NAME={name}",
                    f"ROOM_TOKEN={SANDBOX_KEY}",
                ]
            )
            + "\n",
            encoding="utf-8",
        )
        env_file.chmod(0o600)
        env = dict(os.environ)
        for key in list(env):
            if key.startswith("DAVET_") or key in (
                "ROOM_TOKEN", "LOCA_SESSION", "LOCA_MEMBERSHIP", "LOCA_NAME",
            ):
                env.pop(key)
        env.update({"HOME": str(home), "LOCA_ENV": str(env_file)})
        out = self.root / f"{name}.jsonl"
        log_path = self.root / f"{name}.log"
        log = log_path.open("ab")
        self.addCleanup(log.close)
        process = subprocess.Popen(
            [
                "python3", "-u", str(SKILL_DIR / "listen.py"),
                f"ws://127.0.0.1:{self.port}/ws?room=solo&name={name}&type=agent&filter=mentions",
                str(out),
                "--skip-own", name,
            ],
            env=env,
            stdout=subprocess.DEVNULL,
            stderr=log,
        )
        self.addCleanup(self.stop, process)
        return process, out, log_path

    def seated(self):
        status, members = call(
            self.base, "/rooms/solo/members", headers={"x-admin-token": MASTER}
        )
        self.assertEqual(status, 200)
        return sorted(m["name"] for m in members if m.get("online", True))

    def wait_seated(self, name, timeout, log_path):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if name in self.seated():
                return
            time.sleep(0.2)
        self.fail(
            f"{name} was not seated within {timeout}s:\n"
            + log_path.read_text(errors="replace")
        )

    def test_a_full_loca_is_said_once_and_asked_again_less_often(self):
        first, _, first_log = self.listener("holder-a")
        self.wait_seated("holder-a", 10, first_log)
        _second, _, second_log = self.listener("holder-b")
        self.wait_seated("holder-b", 10, second_log)

        worker, out, worker_log = self.listener("worker")
        # Long enough for four attempts at the old fixed two-second retry.
        time.sleep(7)
        self.assertIsNone(worker.poll(), worker_log.read_text(errors="replace"))
        told = [
            line
            for line in (out.read_text(errors="replace") if out.exists() else "").splitlines()
            if "loca full" in line
        ]
        self.assertEqual(
            len(told),
            1,
            f"the full loca was said {len(told)} times:\n"
            + "\n".join(told)
            + "\n--- listener log:\n"
            + worker_log.read_text(errors="replace"),
        )
        self.assertNotIn("worker", self.seated())

        # A seat is freed: the listener takes it by itself, without anyone
        # restarting it.
        self.stop(first)
        self.wait_seated("worker", 20, worker_log)


if __name__ == "__main__":
    unittest.main()
