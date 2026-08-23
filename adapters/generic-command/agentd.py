#!/usr/bin/env python3
"""Loca generic-command adapter — Runtime Adapter Protocol v1.

This is the piece that turns *any* runtime into a loca participant. Where
bot.py is LLM-shaped (it builds a prompt string and posts stdout verbatim),
this adapter passes **structured JSON** both ways, so the command decides
what to do — call a model, run tests, deploy, query a database — and stays
in control of who the reply addresses.

The shared listener persists protocol-v1 envelopes while a single-flight
consumer invokes this adapter. agentd never owns a WebSocket and never blocks
ping/pong while the user command runs.

  output (stdout):
    - a JSON object:  {"text": "...", "target": "...", "reply_to": 123}
        text     – required; empty/missing means "no reply"
        target   – optional; defaults to the trigger's sender
        reply_to – optional; defaults to the trigger's id
    - or a JSON list of such objects (multiple replies, posted in order)
    - or plain non-JSON text: treated as {"text": <stdout>} so trivial
      commands work without a JSON serializer
    - or nothing: no reply

Usage:
  agentd.py --server http://127.0.0.1:8787 --room general \
            --name test-runner --cmd "./run-tests-agent.sh" \
            [--context 15] [--notes] [--timeout 180] [--token TOK]

The user command receives the v1 envelope plus ``agent``, ``trigger`` and
``context`` compatibility fields. Exit zero is ACKed; failure is retried with
the same delivery/op ids.
"""
import argparse
import json
import os
import shlex
import subprocess
import sys
import urllib.request

# Reuse the installed skill's credential and runtime boundary.
_SKILL = os.path.normpath(os.path.join(
    os.path.dirname(os.path.abspath(__file__)), "..", "..", "skill", "agent-room"))
sys.path.insert(0, _SKILL)
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


def addressed(msg, name):
    """Mirror of the server's msg_addresses — needed because room live mode
    overrides filter=mentions and pushes every message."""
    if msg.get("target") in (name, "all"):
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


def fetch_context(server, room, token, n_msgs, with_notes):
    ctx = {"messages": [], "notes": [], "goal": None}
    try:
        msgs = api("GET", f"{server}/rooms/{room}/messages", token=token) or []
        ctx["messages"] = msgs[-n_msgs:] if n_msgs > 0 else []
    except Exception as e:
        sys.stderr.write(f"[agentd] context messages failed: {e}\n")
    try:
        goals = api("GET", f"{server}/rooms/{room}/goals", token=token) or []
        ctx["goal"] = next(
            (
                goal
                for goal in goals
                if isinstance(goal, dict) and goal.get("status") == "active"
            ),
            None,
        )
    except Exception as e:
        # Goal enriches an already durable delivery. Failure to fetch this
        # optional context must not swallow the user's direct call.
        sys.stderr.write(f"[agentd] context goal failed: {e}\n")
    if with_notes:
        try:
            ctx["notes"] = api("GET", f"{server}/rooms/{room}/notes", token=token) or []
        except Exception as e:
            sys.stderr.write(f"[agentd] context notes failed: {e}\n")
    return ctx


def run_command(cmd, payload, timeout):
    """Run the agent command with the JSON payload on stdin; return stdout."""
    try:
        p = subprocess.run(
            cmd, shell=True, input=json.dumps(payload, ensure_ascii=False),
            text=True, capture_output=True, timeout=timeout,
        )
        if p.returncode != 0:
            sys.stderr.write(
                f"[agentd] command rc={p.returncode}: {p.stderr[:300]}\n")
            return None
        return p.stdout
    except subprocess.TimeoutExpired:
        sys.stderr.write("[agentd] command timed out\n")
        return None
    except Exception as e:
        sys.stderr.write(f"[agentd] command failed: {e}\n")
        return None


def parse_replies(stdout):
    """stdout -> list of reply dicts (see contract). Forgiving on purpose."""
    if stdout is None or not stdout.strip():
        return []
    try:
        out = json.loads(stdout)
    except Exception:
        # Plain text: wrap it so trivial commands need no JSON serializer.
        return [{"text": stdout.strip()}]
    if isinstance(out, dict):
        out = [out]
    if not isinstance(out, list):
        return []
    return [r for r in out if isinstance(r, dict) and str(r.get("text") or "").strip()]


def parse_args(argv=None):
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
    ap.add_argument("--cmd", required=True,
                    help="command implementing the JSON stdin->stdout contract")
    ap.add_argument("--context", type=int, default=15, help="recent messages to include")
    ap.add_argument("--notes", action="store_true", help="include living notes in context")
    ap.add_argument("--timeout", type=int, default=180, help="per-invocation command timeout (s)")
    ap.add_argument("--token", default=None, help="explicit override; normally DAVET_<loca> is selected")
    ap.add_argument("--quiet", action="store_true", help="skip the join announcement")
    ap.add_argument("--invoke", action="store_true", help=argparse.SUPPRESS)
    args = ap.parse_args(argv)
    args.token = args.token if args.token is not None else token_for_room(args.room)
    try:
        validate_server_scope(args.server)
    except CredentialError as exc:
        ap.error(str(exc))
    return args


def trigger_from_event(event):
    messages = event.get("messages") if event.get("t") == "turn" else [event]
    messages = [message for message in messages if isinstance(message, dict)]
    return messages[-1] if messages else {}


def post_reply(server, room, name, reply, trigger, index):
    """Use the shared session-renewing sender with a stable operation id."""
    env = dict(os.environ)
    base_op_id = env.get("LOCA_OP_ID") or f"loca-{env.get('LOCA_DELIVERY_ID', 'unknown')}"
    env["LOCA_OP_ID"] = f"{base_op_id}:{index}"
    reply_to = reply.get("reply_to", trigger.get("id"))
    if isinstance(reply_to, int):
        env["LOCA_REPLY_TO"] = str(reply_to)
    target = str(reply.get("target") or trigger.get("sender") or "-")
    subprocess.run(
        [
            os.path.join(_SKILL, "connect.sh"),
            "send",
            server,
            room,
            name,
            target,
            str(reply["text"]),
        ],
        env=env,
        check=True,
        timeout=30,
    )


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
    trigger = trigger_from_event(event)
    if not trigger or trigger.get("sender") == args.name:
        return 0

    token = token_for_room(room)
    payload = dict(envelope)
    payload.update(
        {
            "agent": args.name,
            "room": room,
            "trigger": trigger,
            "context": fetch_context(
                args.server.rstrip("/"),
                room,
                token,
                args.context,
                args.notes,
            ),
        }
    )
    stdout = run_command(args.cmd, payload, args.timeout)
    if stdout is None:
        raise RuntimeError("generic command failed")
    for index, reply in enumerate(parse_replies(stdout), start=1):
        post_reply(
            args.server.rstrip("/"),
            room,
            args.name,
            reply,
            trigger,
            index,
        )
    return 0


def start(args):
    """Install/start the shared listener+consumer runtime for this identity."""
    hook_args = [
        sys.executable,
        os.path.abspath(__file__),
        "--invoke",
        "--name",
        args.name,
        "--room",
        args.room,
        "--server",
        args.server,
        "--cmd",
        args.cmd,
        "--context",
        str(args.context),
        "--timeout",
        str(args.timeout),
    ]
    if args.notes:
        hook_args.append("--notes")
    env_file = os.environ.get("LOCA_ENV", "")
    runtime_args = [
        os.path.join(_SKILL, "runtime.sh"),
        "start",
        args.name,
        "--runtime",
        "hook",
        "--hook",
        shlex.join(hook_args),
    ]
    if env_file:
        runtime_args.extend(["--env", env_file])
    completed = subprocess.run(runtime_args, check=False)
    return completed.returncode


def main():
    args = parse_args()
    if args.invoke:
        return invoke(args)
    return start(args)


if __name__ == "__main__":
    raise SystemExit(main())
