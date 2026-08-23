#!/usr/bin/env python3
"""Run a real-Codex, no-production-relay Adapter Protocol v2 conformance probe."""

from __future__ import annotations

import argparse
import json
import sys
import tempfile
import time
from pathlib import Path
from urllib.parse import urlparse


ROOT = Path(__file__).resolve().parents[1]
SKILL = ROOT / "skill" / "agent-room"
sys.path.insert(0, str(SKILL))

from attention_store import AttentionStore  # noqa: E402
from codex_adapter_v2 import ConnectShRelay, PersistentCodexAdapter  # noqa: E402


def delivery(
    index: int,
    identity: str,
    server: str,
    room: str,
    *,
    long_turn_seconds: int = 0,
) -> dict:
    if index == 1 and long_turn_seconds:
        text = (
            f"Use the shell tool to run `sleep {long_turn_seconds}` once, then "
            "reply with LONG_TURN_DONE and summarize any newer Loca attention."
        )
    else:
        text = (
            f"Conformance input {index}. Do not use tools. "
            "Reply briefly after processing all supplied Loca attention."
        )
    return {
        "protocol_version": 1,
        "delivery_id": f"conformance:{index}",
        "server": server,
        "room": room,
        "identity": identity,
        "priority": "direct_user",
        "reply_required": True,
        "received_at_ms": int(time.time() * 1000),
        "event": {
            "id": index,
            "room": room,
            "sender": "operator",
            "sender_type": "user",
            "target": identity,
            "text": text,
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--codex-bin", default="codex")
    parser.add_argument("--count", type=int, default=6)
    parser.add_argument("--timeout-seconds", type=float, default=180.0)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--relay-mode", choices=("shadow", "live"), default="shadow")
    parser.add_argument("--server", default="https://conformance.invalid")
    parser.add_argument("--room", default="adapter-v2-conformance")
    parser.add_argument("--long-turn-seconds", type=int, default=0)
    args = parser.parse_args()
    if not 2 <= args.count <= 64:
        parser.error("--count must be between 2 and 64")
    if not 0 <= args.long_turn_seconds <= 300:
        parser.error("--long-turn-seconds must be between 0 and 300")
    if args.relay_mode == "live" and urlparse(args.server).hostname not in {
        "127.0.0.1",
        "localhost",
        "::1",
    }:
        parser.error("live conformance is deliberately limited to a loopback server")

    started = time.time()
    with tempfile.TemporaryDirectory(prefix="loca-v2-conformance-") as tmp:
        root = Path(tmp)
        inbox = root / "inbox.jsonl"
        inbox.touch()
        store = AttentionStore(root / "state.sqlite3")
        relays: list[dict] = []
        live_relay = ConnectShRelay(SKILL / "connect.sh")

        def relay(payload: dict) -> None:
            relays.append(dict(payload))
            if args.relay_mode == "live":
                live_relay(payload)

        adapter = PersistentCodexAdapter(
            store=store,
            inbox=inbox,
            identity="loca-v2-conformance",
            workdir=root,
            codex_bin=args.codex_bin,
            relay_mode=args.relay_mode,
            relay=relay,
            context_provider=lambda _attention: [],
        )
        try:
            adapter.initialize()
            def append_range(start: int, stop: int) -> None:
                with inbox.open("a", encoding="utf-8") as handle:
                    for index in range(start, stop):
                        handle.write(
                            json.dumps(
                                delivery(
                                    index,
                                    adapter.identity,
                                    args.server,
                                    args.room,
                                    long_turn_seconds=args.long_turn_seconds,
                                )
                            )
                            + "\n"
                        )

            if args.long_turn_seconds:
                append_range(1, 2)
                dispatch_deadline = time.monotonic() + 30
                while time.monotonic() < dispatch_deadline:
                    adapter.cycle()
                    snapshot = store.snapshot(adapter.identity)["attentions"]
                    if snapshot and snapshot[0]["accepted_at_ms"]:
                        break
                    time.sleep(0.05)
                else:
                    raise TimeoutError("long turn was not accepted")
                time.sleep(min(2.0, max(0.25, args.long_turn_seconds / 10)))
                append_range(2, args.count + 1)
            else:
                append_range(1, args.count + 1)

            deadline = time.monotonic() + args.timeout_seconds
            while time.monotonic() < deadline:
                adapter.cycle()
                snapshot = store.snapshot(adapter.identity)["attentions"]
                if (
                    len(snapshot) == args.count
                    and all(row["accepted_at_ms"] for row in snapshot)
                    and all(row["turn_completed_at_ms"] for row in snapshot)
                ):
                    break
                time.sleep(0.05)
            else:
                raise TimeoutError("real Codex conformance did not complete")

            with store.connect() as connection:
                unresolved = connection.execute(
                    "SELECT COUNT(*) FROM dispatch_intents WHERE status = 'prepared'"
                ).fetchone()[0]
                final_outputs = connection.execute(
                    "SELECT COUNT(*) FROM relay_items "
                    "WHERE relay_kind = 'final' AND status = ?",
                    ("shadow" if args.relay_mode == "shadow" else "accepted",),
                ).fetchone()[0]
                turn_count = connection.execute(
                    "SELECT COUNT(DISTINCT turn_id) FROM attentions"
                ).fetchone()[0]
            result = {
                "ok": unresolved == 0 and final_outputs > 0,
                "codex_bin": args.codex_bin,
                "attention_count": args.count,
                "turn_count": int(turn_count),
                "prepared_dispatches": int(unresolved),
                "final_outputs": int(final_outputs),
                "relay_mode": args.relay_mode,
                "relay_calls": len(relays),
                "elapsed_seconds": round(time.time() - started, 3),
            }
        finally:
            adapter.close()

    rendered = json.dumps(result, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(rendered, encoding="utf-8")
    print(rendered, end="")
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
