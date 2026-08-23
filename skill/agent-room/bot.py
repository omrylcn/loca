#!/usr/bin/env python3
"""Loca bot — an LLM-shaped Runtime Adapter Protocol v1 command.

This is the piece that makes an agent actually *respond on its own*. A chat
session (Claude Code, an IDE) only lives while someone is typing at it: nothing
outside can start a new turn, so a message arriving in the room cannot wake it.
This bot supplies a managed background brain: the common listener queues the
message, then the single-flight consumer invokes Claude and posts the answer.

    durable delivery  ->  claude -p "..."  ->  idempotent post  ->  ACK

By default it shells out to the `claude` CLI in print mode (`-p`), so it uses
your existing login — no API key needed. Point --brain at any command that
reads a prompt on stdin and prints a reply on stdout to use something else.

Usage:
  bot.py --server http://127.0.0.1:8787 --room general --name helper
         [--brain 'claude -p'] [--context 15] [--system 'extra instructions']

The shared listener owns WebSocket/ping/reconnect/backfill. The bot command is
single-flight, so a long model call cannot make presence disappear.
"""
import argparse
import json
import os
import shlex
import subprocess
import sys
import urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from credentials import (  # noqa: E402
    CredentialError,
    load_local_env,
    token_for_room,
    validate_server_scope,
)


def api(method, url, body=None, token=None):
    validate_server_scope(url)
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("content-type", "application/json")
    if token:
        req.add_header("x-room-token", token)
    if os.environ.get("LOCA_SESSION"):
        req.add_header("x-session-token", os.environ["LOCA_SESSION"])
    with urllib.request.urlopen(req, timeout=15) as r:
        raw = r.read().decode()
        return json.loads(raw) if raw.strip() else None


def recent(server, room, token, limit):
    """Last `limit` messages, for context on wake."""
    try:
        msgs = api("GET", f"{server}/rooms/{room}/messages", token=token) or []
        return msgs[-limit:]
    except Exception:
        return []


def addressed(msg, name):
    """Does this message address `name`? Mirrors the server's msg_addresses:
    target == name/all, or a word-boundary @name/@all in the text. Needed
    because room live mode overrides filter=mentions and pushes *every*
    message — without this check the bot would burn a model call per message
    (and two bots could ping-pong forever)."""
    tgt = msg.get("target")
    if tgt in (name, "all"):
        return True
    needle = "@" + name.lower()
    tok = ""
    for c in msg.get("text", "").lower() + " ":
        if c.isalnum() or c in "@-_":
            tok += c
        else:
            if tok in (needle, "@all"):
                return True
            tok = ""
    return False


def build_prompt(name, room, msg, context, system):
    lines = [
        f"You are '{name}', a participant in the loca chat room '{room}'.",
        "You were just addressed. Reply as a chat message: short, direct, in the",
        "same language as the message. No preamble, no markdown headings — this",
        "goes straight into a chat window.",
    ]
    if system:
        lines.append(system)
    if context:
        lines.append("\nRecent room history:")
        for m in context:
            tgt = f" -> @{m['target']}" if m.get("target") else ""
            lines.append(f"  {m.get('sender')}{tgt}: {m.get('text')}")
    lines.append(f"\nMessage addressed to you, from {msg.get('sender')}:")
    lines.append(msg.get("text", ""))
    lines.append("\nYour reply:")
    return "\n".join(lines)


def think(brain, prompt):
    """Run the brain command with the prompt on stdin; return its stdout."""
    try:
        p = subprocess.run(
            brain, shell=True, input=prompt, text=True,
            capture_output=True, timeout=180,
        )
        out = (p.stdout or "").strip()
        if not out:
            sys.stderr.write(f"brain gave nothing (rc={p.returncode}): {p.stderr[:300]}\n")
        return out
    except subprocess.TimeoutExpired:
        sys.stderr.write("brain timed out\n")
        return ""
    except Exception as e:
        sys.stderr.write(f"brain failed: {e}\n")
        return ""


def parse_args(argv=None):
    # Select credentials before building defaults: the named env supplies this
    # identity's server and per-loca davets on a multi-agent machine.
    pre = argparse.ArgumentParser(add_help=False)
    pre.add_argument("--name")
    known, _ = pre.parse_known_args(argv)
    try:
        load_local_env(known.name)
    except CredentialError as exc:
        pre.error(str(exc))

    ap = argparse.ArgumentParser()
    ap.add_argument("--server", default=os.environ.get("ROOM_SERVER_URL", "http://127.0.0.1:8787"))
    ap.add_argument("--room", default="general")
    ap.add_argument("--name", required=True)
    ap.add_argument("--brain", default="claude -p", help="command that reads a prompt on stdin, prints a reply")
    ap.add_argument("--context", type=int, default=15, help="how many recent messages to include")
    ap.add_argument("--system", default="", help="extra instructions for this bot's persona/role")
    ap.add_argument("--token", default=None, help="explicit override; normally DAVET_<loca> is selected")
    ap.add_argument("--invoke", action="store_true", help=argparse.SUPPRESS)
    args = ap.parse_args(argv)
    args.token = args.token if args.token is not None else token_for_room(args.room)
    try:
        validate_server_scope(args.server)
    except CredentialError as exc:
        ap.error(str(exc))
    return args


def invoke(args):
    raw = os.environ.get("LOCA_DELIVERY") or sys.stdin.read()
    envelope = json.loads(raw)
    if not isinstance(envelope, dict) or envelope.get("protocol_version") != "1":
        raise RuntimeError("unsupported or invalid Loca adapter protocol")
    room = str(envelope.get("room") or "")
    if args.room and room != args.room:
        return 0
    event = envelope.get("event")
    if not isinstance(event, dict):
        raise RuntimeError("delivery event is missing")
    incoming = event.get("messages") if event.get("t") == "turn" else [event]
    incoming = [
        message
        for message in incoming
        if isinstance(message, dict) and message.get("sender") != args.name
    ]
    if not incoming:
        return 0
    trigger = dict(incoming[-1])
    if len(incoming) > 1:
        trigger["text"] = "\n".join(
            f"{message.get('sender')}: {message.get('text', '')}"
            for message in incoming
        )
    token = token_for_room(room)
    context = recent(args.server.rstrip("/"), room, token, args.context)
    reply = think(
        args.brain,
        build_prompt(args.name, room, trigger, context, args.system),
    )
    if not reply:
        raise RuntimeError("brain returned no reply")

    env = dict(os.environ)
    env["LOCA_OP_ID"] = env.get("LOCA_OP_ID") or (
        f"loca-{envelope['delivery_id']}"
    )
    if isinstance(trigger.get("id"), int):
        env["LOCA_REPLY_TO"] = str(trigger["id"])
    subprocess.run(
        [
            os.path.join(os.path.dirname(os.path.abspath(__file__)), "connect.sh"),
            "send",
            args.server.rstrip("/"),
            room,
            args.name,
            str(trigger.get("sender") or "-"),
            reply,
        ],
        env=env,
        check=True,
        timeout=30,
    )
    return 0


def start(args):
    script = os.path.abspath(__file__)
    hook_args = [
        sys.executable,
        script,
        "--invoke",
        "--name",
        args.name,
        "--room",
        args.room,
        "--server",
        args.server,
        "--brain",
        args.brain,
        "--context",
        str(args.context),
    ]
    if args.system:
        hook_args.extend(["--system", args.system])
    runtime_args = [
        os.path.join(os.path.dirname(script), "runtime.sh"),
        "start",
        args.name,
        "--runtime",
        "hook",
        "--hook",
        shlex.join(hook_args),
    ]
    env_file = os.environ.get("LOCA_ENV", "")
    if env_file:
        runtime_args.extend(["--env", env_file])
    return subprocess.run(runtime_args, check=False).returncode


def main():
    args = parse_args()
    if args.invoke:
        return invoke(args)
    return start(args)


if __name__ == "__main__":
    raise SystemExit(main())
