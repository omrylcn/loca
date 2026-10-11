from pathlib import Path
import json
import os
import subprocess
import sys
import tempfile
import time
import unittest

sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from wiki_supervisor import WikiSupervisor


class Process:
    def __init__(self): self.returncode = None; self.stopped = False
    def poll(self): return self.returncode
    def wait(self, **_kwargs): return self.returncode
    def terminate(self): self.stopped = True; self.returncode = -15


class WikiSupervisorTests(unittest.TestCase):
    def setup_supervisor(self):
        self.spawned,self.now = [],0
        def spawn(args,env,stdout):
            self.assertEqual(stdout,subprocess.DEVNULL)
            process = Process()
            self.spawned.append((process,args,env))
            return process
        return WikiSupervisor("codex","room-a",Path("journal"),
            env={"LOCA_NAME":"editor","ROOM_SERVER_URL":"https://example.test"},
            clock=lambda:self.now,spawn=spawn)

    def test_pid_is_not_reported_as_verified(self):
        supervisor = self.setup_supervisor()
        self.assertEqual(supervisor.tick(),"STARTED_UNVERIFIED")
        self.assertIn("--rooms",self.spawned[0][1])
        supervisor.tick()
        self.assertEqual(len(self.spawned),1)

    def test_blocked_native_job_is_terminal_not_infinite_restart(self):
        supervisor = self.setup_supervisor(); supervisor.tick()
        self.spawned[-1][0].returncode = 2
        self.assertEqual(supervisor.tick(),"BLOCKED")
        self.now = 100
        supervisor.tick()
        self.assertEqual(len(self.spawned),1)

    def test_transient_exit_has_backoff_and_three_failure_limit(self):
        supervisor = self.setup_supervisor(); supervisor.tick()
        for number in range(3):
            self.spawned[-1][0].returncode = 1
            supervisor.tick()
            self.now += 100
            supervisor.tick()
        self.assertEqual(supervisor.status,"BLOCKED")
        self.assertEqual(len(self.spawned),3)

    def test_shutdown_stops_only_the_owned_sidecar(self):
        supervisor = self.setup_supervisor(); supervisor.tick()
        process = self.spawned[0][0]
        supervisor.stop()
        self.assertTrue(process.stopped)
        self.assertIsNone(supervisor.process)

    def test_identity_is_required_not_guessed(self):
        with self.assertRaises(ValueError):
            WikiSupervisor("codex","room-a",Path("journal"),env={})

    def test_real_parent_keeps_listener_alive_when_wiki_capability_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            credentials = root/"editor.env"
            credentials.write_text("LOCA_NAME=editor\nROOM_SERVER_URL=http://127.0.0.1:9\n")
            credentials.chmod(0o600)
            env = {key:value for key,value in os.environ.items()
                if not key.startswith(("LOCA_","ROOM_","DAVET_","ADMIN_","CARE_"))}
            env.update(HOME=str(root),LOCA_ENV=str(credentials),CODEX_BIN=str(root/"missing-native"))
            health = root/"health.json"
            args = [sys.executable,str(Path(__file__).resolve().parents[1]/"runtime_agent.py"),
                "--inbox",str(root/"inbox"),"--worker-cursor",str(root/"cursor"),
                "--health-file",str(health),"--wiki-runtime","codex","--wiki-rooms","room-a",
                "--wiki-journal",str(root/"wiki.sqlite3"),"--",
                sys.executable,"-c","import time; time.sleep(30)"]
            process = subprocess.Popen(args,env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
            try:
                state = {}
                deadline = time.monotonic()+5
                while time.monotonic()<deadline:
                    try: state = json.loads(health.read_text())
                    except (OSError,ValueError): pass
                    if state.get("wiki")=="BLOCKED": break
                    time.sleep(.05)
                self.assertEqual(state.get("wiki"),"BLOCKED")
                self.assertIsNone(process.poll(),"Wiki failure killed the healthy listener supervisor")
            finally:
                process.terminate()
                try: process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill(); process.wait(timeout=3)


if __name__ == "__main__": unittest.main()
