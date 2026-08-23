#!/usr/bin/env python3
"""ci-agent — a room participant with no LLM at all.

Proof that the adapter contract is runtime-neutral: this "agent" is just a
shell command. Address it (@ci) and it runs the project's test command and
posts a pass/fail summary back to the room.

Run it with agentd:

  python3 adapters/generic-command/agentd.py \
    --room general --name ci \
    --cmd "CI_DIR=/path/to/repo python3 adapters/generic-command/examples/ci-agent.py"

Env:
  CI_DIR  – working directory (default: current)
  CI_CMD  – command to run (default: "cargo test -q")

Contract: trigger JSON on stdin -> reply JSON on stdout (see agentd.py).
"""
import json
import os
import subprocess
import sys

req = json.load(sys.stdin)
trigger = req.get("trigger", {})
text = (trigger.get("text") or "").lower()

cmd = os.environ.get("CI_CMD", "cargo test -q")
cwd = os.environ.get("CI_DIR", ".")

# A tiny bit of intent, no model needed: only run on test-ish requests;
# otherwise explain what we do (still a polite room citizen).
if not any(w in text for w in ("test", "run", "ci", "check", "koş", "kos")):
    print(json.dumps({
        "text": f"ben ci — '@{req.get('agent','ci')} test' dersen `{cmd}` koşar, sonucu buraya yazarım."
    }, ensure_ascii=False))
    sys.exit(0)

try:
    p = subprocess.run(cmd, shell=True, cwd=cwd, capture_output=True,
                       text=True, timeout=600)
    out = (p.stdout or "") + (p.stderr or "")
    # keep the reply chat-sized: last meaningful lines only
    tail = [l for l in out.strip().splitlines() if l.strip()][-6:]
    status = "✅ PASS" if p.returncode == 0 else f"❌ FAIL (rc={p.returncode})"
    body = f"{status} — `{cmd}`\n" + "\n".join(tail[-4:])
except subprocess.TimeoutExpired:
    body = f"⏱ timeout — `{cmd}` 600s'de bitmedi"
except Exception as e:
    body = f"⚠ çalıştıramadım: {e}"

print(json.dumps({"text": body}, ensure_ascii=False))
