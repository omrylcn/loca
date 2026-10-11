"""Independent opt-in Wiki sidecar; failure never restarts the chat adapter."""
import os
from pathlib import Path
import subprocess
import sys
import time


class WikiSupervisor:
    def __init__(self, runtime, rooms, journal, env=None, clock=time.monotonic, spawn=subprocess.Popen):
        self.env = dict(os.environ if env is None else env)
        self.clock,self.spawn = clock,spawn
        self.process = None
        self.failures,self.restart_at,self.terminal = 0,0.0,False
        self.status = "STARTING"
        name,server = self.env.get("LOCA_NAME"),self.env.get("ROOM_SERVER_URL")
        if not name or not server or runtime not in ("codex","claude"):
            raise ValueError("Wiki supervision requires an explicit bound identity and native runtime")
        self.args = [sys.executable,str(Path(__file__).with_name("wiki_runtime.py")),
            "--name",name,"--server",server,"--rooms",rooms,"--journal",str(journal),"--runtime",runtime]
        binary = self.env.get("CODEX_BIN" if runtime == "codex" else "CLAUDE_BIN")
        if binary: self.args += ["--binary",binary]
        if runtime == "claude" and self.env.get("CLAUDE_NODE"):
            self.args += ["--node",self.env["CLAUDE_NODE"]]

    def tick(self):
        if self.terminal:
            return self.status
        if self.process is not None and self.process.poll() is not None:
            code = self.process.returncode
            self.process.wait()
            self.process = None
            self.failures += 1
            if code in (0,2) or self.failures >= 3:
                self.terminal,self.status = True,"BLOCKED"
                print("Wiki supervisor: terminal diagnostic; durable journal retained",file=sys.stderr,flush=True)
                return self.status
            self.restart_at = self.clock()+min(60,15*(2**(self.failures-1)))
            self.status = "RESTARTING"
        if self.process is None and self.clock() >= self.restart_at:
            try:
                # Idle/publication status is not a Claude Monitor wake event.
                self.process = self.spawn(self.args,env=self.env,stdout=subprocess.DEVNULL)
                # PID only means the runner started, not model or delivery GO.
                self.status = "STARTED_UNVERIFIED"
            except OSError:
                self.failures += 1
                self.restart_at = self.clock()+15
                self.status = "BLOCKED" if self.failures>=3 else "RESTARTING"
                self.terminal = self.failures>=3
        return self.status

    def stop(self):
        if self.process is None:
            return
        if self.process.poll() is None:
            self.process.terminate()
        try:
            self.process.wait(timeout=8)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(timeout=3)
        self.process = None
