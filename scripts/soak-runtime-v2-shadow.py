#!/usr/bin/env python3
"""Soak the real v1 Codex consumer beside the non-relaying v2 adapter.

The harness is deliberately isolated from production Loca. It creates a
dedicated Codex thread, feeds one durable inbox to the normal v1 consumer and
the persistent v2 shadow, and compares their durable progress at the end.
"""

from __future__ import annotations

import argparse
import json
import os
import shlex
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
SKILL = ROOT / "skill" / "agent-room"
sys.path.insert(0, str(SKILL))

from attention_store import AttentionStore  # noqa: E402
from codex_adapter_v2 import PersistentCodexAdapter  # noqa: E402
from nudge import AppServer, nudge_codex, read_event, response_error  # noqa: E402
from orchestrator_queue import load_state  # noqa: E402


IDENTITY = "loca-dual-shadow-soak"
SERVER = "https://conformance.invalid"
ROOM = "dual-shadow-soak"


def delivery(index: int) -> dict[str, Any]:
    return {
        "protocol_version": 1,
        "delivery_id": f"dual-soak:{index}",
        "server": SERVER,
        "room": ROOM,
        "identity": IDENTITY,
        "priority": "direct_user",
        "reply_required": True,
        "received_at_ms": int(time.time() * 1000),
        "last_id": index,
        "event": {
            "id": index,
            "room": ROOM,
            "sender": "operator",
            "sender_type": "user",
            "target": IDENTITY,
            "text": (
                f"Dual shadow soak message {index}. Do not use tools or "
                "external services. Reply briefly to confirm processing."
            ),
        },
    }


def initialize_client(client: AppServer) -> None:
    initialized = client.request(
        "initialize",
        {
            "clientInfo": {
                "name": "loca_dual_shadow_soak",
                "title": "Loca dual shadow soak",
                "version": "1.0.0",
            }
        },
    )
    if error := response_error(initialized):
        raise RuntimeError(f"Codex initialize failed: {error}")
    client.send_notification("initialized", {})


def create_v1_thread(codex_bin: str, workdir: Path) -> str:
    client = AppServer(codex_bin, 10.0)
    try:
        initialize_client(client)
        response = client.request(
            "thread/start",
            {
                "cwd": str(workdir),
                "approvalPolicy": "never",
                "ephemeral": False,
                "developerInstructions": (
                    "You are the isolated v1 side of a Loca runtime soak. "
                    "Never use tools or external services. For each supplied "
                    "Loca attention, reply briefly and finish the turn."
                ),
            },
        )
        if error := response_error(response):
            raise RuntimeError(f"Codex thread/start failed: {error}")
        result = response.get("result")
        thread = result.get("thread") if isinstance(result, dict) else None
        thread_id = str(thread.get("id") or "") if isinstance(thread, dict) else ""
        if not thread_id:
            raise RuntimeError("Codex thread/start returned no thread id")
        # Codex does not persist an empty thread as a resumable rollout. Seed
        # one completed no-tool turn before handing the thread to v1 nudge.py.
        started = client.request(
            "turn/start",
            {
                "threadId": thread_id,
                "approvalPolicy": "never",
                "input": [
                    {
                        "type": "text",
                        "text": "Reply exactly SOAK_THREAD_READY. Do not use tools.",
                    }
                ],
            },
        )
        if error := response_error(started):
            raise RuntimeError(f"Codex seed turn/start failed: {error}")
        turn = started.get("result", {}).get("turn", {})
        turn_id = str(turn.get("id") or "") if isinstance(turn, dict) else ""
        if not turn_id:
            raise RuntimeError("Codex seed turn/start returned no turn id")
        completed = client.wait_for_turn(thread_id, turn_id, 120.0)
        if completed.get("status") != "completed":
            raise RuntimeError(
                f"Codex seed turn did not complete: {completed.get('status')}"
            )
        return thread_id
    finally:
        client.close()


def v1_hook_main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("--thread-id", required=True)
    parser.add_argument("--skill-path", type=Path, required=True)
    parser.add_argument("--codex-bin", required=True)
    parser.add_argument("--turn-timeout", type=float, default=300.0)
    args = parser.parse_args(argv)
    return nudge_codex(
        read_event(),
        args.thread_id,
        args.skill_path,
        args.codex_bin,
        10.0,
        args.turn_timeout,
        "inherit",
    )


def shadow_child_main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("--inbox", type=Path, required=True)
    parser.add_argument("--state-db", type=Path, required=True)
    parser.add_argument("--health-file", type=Path, required=True)
    parser.add_argument("--workdir", type=Path, required=True)
    parser.add_argument("--codex-bin", required=True)
    args = parser.parse_args(argv)
    stopping = False

    def stop(*_unused: object) -> None:
        nonlocal stopping
        stopping = True

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    adapter = PersistentCodexAdapter(
        store=AttentionStore(args.state_db),
        inbox=args.inbox,
        identity=IDENTITY,
        workdir=args.workdir,
        codex_bin=args.codex_bin,
        relay_mode="shadow",
        relay=lambda _payload: None,
        context_provider=lambda _attention: [],
        health_file=args.health_file,
    )
    try:
        adapter.initialize()
        while not stopping:
            adapter.cycle()
            time.sleep(0.05)
    finally:
        adapter.close()
    return 0


def read_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return {}
    return value if isinstance(value, dict) else {}


def wait_for_shadow_ready(
    health_file: Path, supervisor: subprocess.Popen[bytes], timeout: float
) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if supervisor.poll() is not None:
            raise RuntimeError(
                f"dual supervisor exited before readiness: {supervisor.returncode}"
            )
        if read_json(health_file).get("ingestion") == "OK":
            return
        time.sleep(0.1)
    raise TimeoutError("v2 shadow ingestion readiness timed out")


def append_delivery(inbox: Path, record: dict[str, Any]) -> None:
    with inbox.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(record, ensure_ascii=False) + "\n")
        handle.flush()
        os.fsync(handle.fileno())


def evidence(
    *,
    state_dir: Path,
    duration_seconds: float,
    started: float,
    source_ids: list[str],
    supervisor_exit: int | None,
) -> dict[str, Any]:
    inbox = state_dir / "inbox.jsonl"
    cursor = load_state(state_dir / "v1-cursor.json")
    v1_health = read_json(state_dir / "v1-health.json")
    v2_health = read_json(state_dir / "v2-health.json")
    store = AttentionStore(state_dir / "v2.sqlite3")
    snapshot = store.snapshot(IDENTITY)["attentions"]
    with store.connect() as connection:
        actual_ids = [
            str(row["delivery_id"])
            for row in connection.execute(
                "SELECT delivery_id FROM attentions "
                "WHERE identity = ? ORDER BY rowid",
                (IDENTITY,),
            ).fetchall()
        ]
        prepared = int(
            connection.execute(
                "SELECT COUNT(*) FROM dispatch_intents WHERE status = 'prepared'"
            ).fetchone()[0]
        )
        non_shadow_relays = int(
            connection.execute(
                "SELECT COUNT(*) FROM relay_items WHERE status != 'shadow'"
            ).fetchone()[0]
        )
        turn_count = int(
            connection.execute(
                "SELECT COUNT(DISTINCT turn_id) FROM attentions"
            ).fetchone()[0]
        )
    log_text = (state_dir / "supervisor.log").read_text(
        encoding="utf-8", errors="replace"
    )
    restarts = [
        line
        for line in log_text.splitlines()
        if "exited code=" in line or "restart in " in line
    ]
    v1_reached_end = int(cursor.get("offset") or 0) == inbox.stat().st_size
    exact_v2_coverage = actual_ids == source_ids
    all_accepted = bool(snapshot) and all(row["accepted_at_ms"] for row in snapshot)
    all_completed = bool(snapshot) and all(
        row["turn_completed_at_ms"] for row in snapshot
    )
    ok = all(
        (
            source_ids,
            v1_reached_end,
            v1_health.get("ack") == "OK",
            exact_v2_coverage,
            all_accepted,
            all_completed,
            prepared == 0,
            non_shadow_relays == 0,
            not restarts,
            supervisor_exit in (None, 0, -signal.SIGTERM),
        )
    )
    return {
        "ok": ok,
        "duration_seconds_requested": duration_seconds,
        "elapsed_seconds": round(time.time() - started, 3),
        "source_delivery_count": len(source_ids),
        "source_delivery_ids": source_ids,
        "v1_cursor_offset": int(cursor.get("offset") or 0),
        "inbox_size": inbox.stat().st_size,
        "v1_reached_inbox_end": v1_reached_end,
        "v1_last_acked_delivery_id": cursor.get("delivery_id"),
        "v1_acked_delivery_ids": cursor.get("acked_delivery_ids") or [],
        "v1_health": v1_health,
        "v2_delivery_ids": actual_ids,
        "v2_exact_source_coverage": exact_v2_coverage,
        "v2_attention_count": len(snapshot),
        "v2_all_accepted": all_accepted,
        "v2_all_turn_completed": all_completed,
        "v2_prepared_dispatches": prepared,
        "v2_non_shadow_relays": non_shadow_relays,
        "v2_turn_count": turn_count,
        "v2_health": v2_health,
        "supervisor_exit": supervisor_exit,
        "supervisor_restart_events": restarts,
        "state_dir": str(state_dir),
    }


def run_main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--codex-bin", default="codex")
    parser.add_argument("--duration-seconds", type=float, default=10_800.0)
    parser.add_argument("--interval-seconds", type=float, default=600.0)
    parser.add_argument("--settle-seconds", type=float, default=300.0)
    parser.add_argument("--state-dir", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args(argv)
    if not 15 <= args.duration_seconds <= 86_400:
        parser.error("--duration-seconds must be between 15 and 86400")
    if not 1 <= args.interval_seconds <= args.duration_seconds:
        parser.error("--interval-seconds must be between 1 and the duration")
    if not 10 <= args.settle_seconds <= 1_800:
        parser.error("--settle-seconds must be between 10 and 1800")

    codex_bin = str(Path(args.codex_bin).resolve()) if "/" in args.codex_bin else args.codex_bin
    own_temp = args.state_dir is None
    temp = tempfile.TemporaryDirectory(prefix="loca-dual-shadow-soak-") if own_temp else None
    state_dir = Path(temp.name) if temp is not None else args.state_dir.resolve()
    if state_dir.exists() and any(state_dir.iterdir()):
        parser.error(f"--state-dir must be empty: {state_dir}")
    state_dir.mkdir(parents=True, exist_ok=True)
    os.chmod(state_dir, 0o700)
    inbox = state_dir / "inbox.jsonl"
    inbox.touch()
    skill_file = state_dir / "SOAK-SKILL.md"
    skill_file.write_text(
        "---\nname: loca-soak\ndescription: isolated runtime soak\n---\n"
        "Do not call tools, read credentials, or contact external services. "
        "Reply briefly to acknowledge each supplied conformance attention.\n",
        encoding="utf-8",
    )
    dummy_env = state_dir / "identity.env"
    dummy_env.write_text(
        f"ROOM_SERVER_URL=http://127.0.0.1:9\nLOCA_NAME={IDENTITY}\n",
        encoding="utf-8",
    )
    os.chmod(dummy_env, 0o600)
    v1_thread = create_v1_thread(codex_bin, state_dir)
    (state_dir / "v1-thread").write_text(v1_thread + "\n", encoding="utf-8")

    self_path = Path(__file__).resolve()
    v1_hook = shlex.join(
        [
            "exec",
            sys.executable,
            str(self_path),
            "v1-hook",
            "--thread-id",
            v1_thread,
            "--skill-path",
            str(skill_file),
            "--codex-bin",
            codex_bin,
            "--turn-timeout",
            str(max(300.0, args.interval_seconds)),
        ]
    )
    shadow_hook = shlex.join(
        [
            "exec",
            sys.executable,
            str(self_path),
            "shadow-child",
            "--inbox",
            str(inbox),
            "--state-db",
            str(state_dir / "v2.sqlite3"),
            "--health-file",
            str(state_dir / "v2-health.json"),
            "--workdir",
            str(state_dir),
            "--codex-bin",
            codex_bin,
        ]
    )
    listener_sleep = args.duration_seconds + args.settle_seconds + 120
    supervisor_command = [
        sys.executable,
        str(SKILL / "runtime_agent.py"),
        "--inbox",
        str(inbox),
        "--worker-cursor",
        str(state_dir / "v1-cursor.json"),
        "--health-file",
        str(state_dir / "v1-health.json"),
        "--exec",
        v1_hook,
        "--shadow-persistent-exec",
        shadow_hook,
        "--shadow-ready-file",
        str(state_dir / "v2-health.json"),
        "--ready-timeout-seconds",
        "60",
        "--consumer-timeout-seconds",
        str(max(600.0, args.interval_seconds * 2)),
        "--preempt-direct-user",
        "--",
        sys.executable,
        "-c",
        f"import time; time.sleep({listener_sleep!r})",
    ]
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith("LOCA_") and key != "CODEX_THREAD_ID"
    }
    env["LOCA_ENV"] = str(dummy_env)
    log_handle = (state_dir / "supervisor.log").open("wb")
    supervisor = subprocess.Popen(
        supervisor_command,
        cwd=state_dir,
        env=env,
        stdout=log_handle,
        stderr=subprocess.STDOUT,
    )
    source_ids: list[str] = []
    started = time.time()
    result: dict[str, Any]
    try:
        wait_for_shadow_ready(state_dir / "v2-health.json", supervisor, 60.0)
        next_delivery_at = time.monotonic()
        deadline = time.monotonic() + args.duration_seconds
        index = 1
        while time.monotonic() < deadline:
            if supervisor.poll() is not None:
                raise RuntimeError(
                    f"dual supervisor exited during soak: {supervisor.returncode}"
                )
            now = time.monotonic()
            if now >= next_delivery_at:
                record = delivery(index)
                append_delivery(inbox, record)
                source_ids.append(str(record["delivery_id"]))
                index += 1
                next_delivery_at += args.interval_seconds
            time.sleep(0.1)

        settle_deadline = time.monotonic() + args.settle_seconds
        while time.monotonic() < settle_deadline:
            store = AttentionStore(state_dir / "v2.sqlite3")
            snapshot = store.snapshot(IDENTITY)["attentions"]
            cursor = load_state(state_dir / "v1-cursor.json")
            if (
                int(cursor.get("offset") or 0) == inbox.stat().st_size
                and len(snapshot) == len(source_ids)
                and all(row["accepted_at_ms"] for row in snapshot)
                and all(row["turn_completed_at_ms"] for row in snapshot)
            ):
                break
            if supervisor.poll() is not None:
                raise RuntimeError(
                    f"dual supervisor exited while settling: {supervisor.returncode}"
                )
            time.sleep(0.2)
        supervisor.send_signal(signal.SIGTERM)
        try:
            supervisor.wait(timeout=15)
        except subprocess.TimeoutExpired:
            supervisor.kill()
            supervisor.wait(timeout=5)
        log_handle.close()
        result = evidence(
            state_dir=state_dir,
            duration_seconds=args.duration_seconds,
            started=started,
            source_ids=source_ids,
            supervisor_exit=supervisor.returncode,
        )
    except (Exception, KeyboardInterrupt) as error:
        if supervisor.poll() is None:
            supervisor.send_signal(signal.SIGTERM)
            try:
                supervisor.wait(timeout=10)
            except subprocess.TimeoutExpired:
                supervisor.kill()
                supervisor.wait(timeout=5)
        log_handle.close()
        result = evidence(
            state_dir=state_dir,
            duration_seconds=args.duration_seconds,
            started=started,
            source_ids=source_ids,
            supervisor_exit=supervisor.returncode,
        )
        result["ok"] = False
        result["error"] = str(error)

    rendered = json.dumps(result, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(rendered, encoding="utf-8")
    print(rendered, end="")
    if temp is not None:
        temp.cleanup()
    return 0 if result["ok"] else 1


def main() -> int:
    if len(sys.argv) > 1 and sys.argv[1] == "v1-hook":
        return v1_hook_main(sys.argv[2:])
    if len(sys.argv) > 1 and sys.argv[1] == "shadow-child":
        return shadow_child_main(sys.argv[2:])
    return run_main(sys.argv[1:])


if __name__ == "__main__":
    raise SystemExit(main())
