#!/usr/bin/env python3
"""Claude as a generic-command agent.

Wraps `claude -p` (print mode — uses your existing login, no API key) behind
the JSON stdin -> stdout contract. This is deliberately the same shape a
Codex, Gemini or Ollama wrapper would have: build a prompt from the trigger
and context, run the runtime, return {"text": reply}. Swap the BRAIN command
to change runtimes; the room never knows the difference.
"""
import json
import subprocess
import sys

BRAIN = "claude -p"          # e.g. "codex exec", "ollama run llama3", …
TIMEOUT = 170                # stay under agentd's default --timeout 180


def build_prompt(req):
    name, room = req["agent"], req["room"]
    trigger, ctx = req["trigger"], req.get("context", {})
    lines = [
        f"You are '{name}', a participant in the loca chat room '{room}'.",
        "You were just addressed. Reply as a chat message: short, direct, in",
        "the same language as the message. No preamble, no markdown headings.",
    ]
    notes = ctx.get("notes") or []
    if notes:
        lines.append("\nLiving notes (current project state):")
        for n in notes:
            lines.append(f"  [{n.get('key')}] {n.get('title')}: {n.get('body')}")
    msgs = ctx.get("messages") or []
    if msgs:
        lines.append("\nRecent room history:")
        for m in msgs:
            tgt = f" -> @{m['target']}" if m.get("target") else ""
            lines.append(f"  {m.get('sender')}{tgt}: {m.get('text')}")
    lines.append(f"\nMessage addressed to you, from {trigger.get('sender')}:")
    lines.append(trigger.get("text", ""))
    lines.append("\nYour reply:")
    return "\n".join(lines)


def main():
    req = json.load(sys.stdin)
    p = subprocess.run(BRAIN, shell=True, input=build_prompt(req),
                       text=True, capture_output=True, timeout=TIMEOUT)
    reply = (p.stdout or "").strip()
    if not reply:
        sys.stderr.write(f"brain gave nothing (rc={p.returncode}): {p.stderr[:300]}\n")
        return                      # no output -> agentd posts nothing
    print(json.dumps({"text": reply}, ensure_ascii=False))


if __name__ == "__main__":
    main()
