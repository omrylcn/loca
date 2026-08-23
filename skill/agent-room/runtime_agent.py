#!/usr/bin/env python3
"""Supervise one Loca listener and its optional runtime adapter."""

from __future__ import annotations

import argparse
import json
import os
import signal
import subprocess
import sys
import threading
import time
import urllib.request
from collections import deque
from pathlib import Path

from credentials import CredentialError, load_local_env


def terminate(process: subprocess.Popen[bytes] | None) -> None:
    if process is None or process.poll() is not None:
        return
    process.terminate()


def update_health(path: Path | None, **values: object) -> None:
    if path is None:
        return
    state: dict[str, object] = {}
    try:
        loaded = json.loads(path.read_text(encoding="utf-8"))
        if isinstance(loaded, dict):
            state = loaded
    except (OSError, ValueError):
        pass
    state.update(values)
    state["updated_at_ms"] = int(time.time() * 1000)
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    temp = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    temp.write_text(json.dumps(state, sort_keys=True) + "\n", encoding="utf-8")
    os.chmod(temp, 0o600)
    os.replace(temp, path)


def report_runtime_health(path: Path | None, automatic: bool) -> None:
    server = os.environ.get("ROOM_SERVER_URL", "").rstrip("/")
    membership = os.environ.get("LOCA_MEMBERSHIP", "")
    if not server or not membership:
        return
    state: dict[str, object] = {}
    if path is not None:
        try:
            loaded = json.loads(path.read_text(encoding="utf-8"))
            if isinstance(loaded, dict):
                state = loaded
        except (OSError, ValueError):
            pass
    body = json.dumps(
        {
            "wake": str(state.get("wake") or ("UNVERIFIED" if automatic else "MANUAL")),
            "ack": str(state.get("ack") or ("UNVERIFIED" if automatic else "IDLE")),
            "delivery_id": state.get("delivery_id"),
            "attention_id": state.get("last_attention_id")
            or state.get("last_accepted_attention_id"),
            "stored": bool(state.get("stored", False)),
            "accepted": bool(state.get("accepted", False)),
            "first_response": bool(state.get("first_response", False)),
            "final_response": bool(state.get("final_response", False)),
            "turn_completed": bool(state.get("turn_completed", False)),
        }
    ).encode()
    request = urllib.request.Request(
        f"{server}/runtime/health", data=body, method="POST"
    )
    request.add_header("content-type", "application/json")
    request.add_header("x-room-token", membership)
    with urllib.request.urlopen(request, timeout=5):
        pass


# Crash-loop budget for the v2 adapter path, mirroring monitor_listener.py:
# after this many failures inside the window, stop churning and escalate to a
# terminal diagnostic instead of restarting the adapter forever.
ADAPTER_MAX_RESTARTS_DEFAULT = 6
ADAPTER_FAILURE_WINDOW_DEFAULT = 300.0


class _AdapterState:
    """Restart budget + stderr tail for one supervised v2 adapter child."""

    def __init__(self, label: str, stderr_sink: "deque[str]") -> None:
        self.label = label
        self.stderr_sink = stderr_sink
        self.failures = 0
        self.recent: "deque[float]" = deque()
        self.restart_at = 0.0
        self.terminal = False

    def tail(self) -> str:
        return "\n".join(self.stderr_sink).strip()


def _drain_stderr(process: "subprocess.Popen | None", sink: "deque[str]") -> None:
    """Keep a bounded tail of a child's stderr for diagnostics while still
    mirroring every line to our own stderr (journald) so observability is
    preserved."""
    if process is None or process.stderr is None:
        return
    stream = process.stderr

    def run() -> None:
        try:
            for raw in stream:
                sink.append(raw.rstrip("\n"))
                try:
                    sys.stderr.write(raw if raw.endswith("\n") else raw + "\n")
                    sys.stderr.flush()
                except Exception:
                    pass
        except (OSError, ValueError):
            pass

    threading.Thread(target=run, daemon=True).start()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--inbox", type=Path, required=True)
    parser.add_argument("--worker-cursor", type=Path, required=True)
    parser.add_argument("--health-file", type=Path)
    parser.add_argument("--exec", dest="command")
    parser.add_argument("--persistent-exec", dest="persistent_command")
    parser.add_argument(
        "--shadow-persistent-exec",
        dest="shadow_persistent_command",
        help="run an independently supervised non-relaying shadow adapter",
    )
    parser.add_argument("--persistent-ready-file", type=Path)
    parser.add_argument("--shadow-ready-file", type=Path)
    parser.add_argument("--ready-timeout-seconds", type=float, default=20.0)
    parser.add_argument("--consumer-timeout-seconds", type=float, default=330.0)
    parser.add_argument(
        "--adapter-max-restarts",
        type=int,
        default=ADAPTER_MAX_RESTARTS_DEFAULT,
        help="v2 adapter failures allowed within the window before a terminal escalation",
    )
    parser.add_argument(
        "--adapter-failure-window-seconds",
        type=float,
        default=ADAPTER_FAILURE_WINDOW_DEFAULT,
    )
    parser.add_argument("--preempt-direct-user", action="store_true")
    parser.add_argument("listener", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.command and args.persistent_command:
        parser.error("--exec and --persistent-exec are mutually exclusive")
    try:
        load_local_env()
    except CredentialError as error:
        print(f"runtime supervisor: credential load failed: {error}", file=sys.stderr)
    listener_args = list(args.listener)
    if listener_args and listener_args[0] == "--":
        listener_args.pop(0)
    if not listener_args:
        parser.error("listener command is required after --")

    listener: subprocess.Popen[bytes] | None = None
    consumer: subprocess.Popen[bytes] | None = None
    shadow: subprocess.Popen[bytes] | None = None
    stopping = False

    consumer_stderr: "deque[str]" = deque(maxlen=40)
    shadow_stderr: "deque[str]" = deque(maxlen=40)
    consumer_state = _AdapterState("persistent adapter", consumer_stderr)
    shadow_state = _AdapterState("shadow adapter", shadow_stderr)

    def stop_children(*_unused: object) -> None:
        nonlocal stopping
        stopping = True
        terminate(shadow)
        terminate(consumer)
        terminate(listener)

    signal.signal(signal.SIGTERM, stop_children)
    signal.signal(signal.SIGINT, stop_children)

    def start_listener() -> subprocess.Popen[bytes]:
        print("runtime supervisor: listener starting", file=sys.stderr, flush=True)
        return subprocess.Popen(listener_args)

    def start_consumer() -> subprocess.Popen[bytes] | None:
        if not args.command and not args.persistent_command:
            return None
        print("runtime supervisor: wake bridge starting", file=sys.stderr, flush=True)
        if args.persistent_command:
            process = subprocess.Popen(
                args.persistent_command,
                shell=True,
                start_new_session=True,
                stderr=subprocess.PIPE,
                text=True,
            )
            _drain_stderr(process, consumer_stderr)
            return process
        return subprocess.Popen(
                [
                    sys.executable,
                    str(Path(__file__).with_name("runtime_consumer.py")),
                    "--inbox",
                    str(args.inbox),
                    "--cursor",
                    str(args.worker_cursor),
                    "--exec",
                    args.command,
                    "--timeout-seconds",
                    str(args.consumer_timeout_seconds),
                ]
                + (
                    ["--health-file", str(args.health_file)]
                    if args.health_file is not None
                    else []
                )
                + (["--preempt-direct-user"] if args.preempt_direct_user else [])
            )

    def start_shadow() -> subprocess.Popen[bytes] | None:
        if not args.shadow_persistent_command:
            return None
        print(
            "runtime supervisor: shadow adapter starting",
            file=sys.stderr,
            flush=True,
        )
        process = subprocess.Popen(
            args.shadow_persistent_command,
            shell=True,
            start_new_session=True,
            stderr=subprocess.PIPE,
            text=True,
        )
        _drain_stderr(process, shadow_stderr)
        return process

    def wait_ready(
        process: subprocess.Popen[bytes] | None,
        ready_file: Path | None,
        label: str,
    ) -> None:
        if process is None or ready_file is None:
            return
        deadline = time.monotonic() + args.ready_timeout_seconds
        while time.monotonic() < deadline:
            if process.poll() is not None:
                raise RuntimeError(
                    f"{label} exited before its ingestion readiness handshake"
                )
            try:
                state = json.loads(ready_file.read_text(encoding="utf-8"))
                if isinstance(state, dict) and state.get("ingestion") == "OK":
                    return
            except (OSError, ValueError):
                pass
            time.sleep(0.05)
        raise RuntimeError(f"{label} ingestion readiness handshake timed out")

    def record_adapter_failure(
        state: _AdapterState,
        code: object,
        now: float,
        health_fields: dict[str, object],
    ) -> None:
        """Route a v2 adapter failure (crash OR readiness failure) through a
        crash-loop budget: back off and restart, or after the budget is
        exhausted escalate to a terminal diagnostic that names the child's
        stderr tail and stops the churn. The listener is never touched here, so
        presence stays honest even while the adapter is unhealthy."""
        state.failures += 1
        state.recent.append(now)
        cutoff = now - args.adapter_failure_window_seconds
        while state.recent and state.recent[0] < cutoff:
            state.recent.popleft()
        if len(state.recent) > args.adapter_max_restarts:
            state.terminal = True
            tail = state.tail()
            reason = (
                f"{state.label} restart budget exhausted after "
                f"{len(state.recent)} failures in "
                f"{args.adapter_failure_window_seconds:.0f}s; last exit={code}"
            )
            detail = reason + (
                f"\n--- {state.label} stderr tail ---\n{tail}" if tail else ""
            )
            print(
                f"runtime supervisor: TERMINAL {detail}",
                file=sys.stderr,
                flush=True,
            )
            update_health(
                args.health_file,
                wake="FAILED",
                ack="TERMINAL",
                presence="DEGRADED",
                adapter_terminal=True,
                last_error=reason,
                last_adapter_stderr=tail,
            )
            return
        delay = min(30.0, float(2 ** min(state.failures - 1, 5)))
        state.restart_at = now + delay
        print(
            f"runtime supervisor: {state.label} failed (code={code}); "
            f"restart in {delay:.0f}s",
            file=sys.stderr,
            flush=True,
        )
        if health_fields:
            update_health(args.health_file, **health_fields)

    try:
        if args.persistent_command:
            consumer = start_consumer()
        shadow = start_shadow()
        # A readiness failure must NOT bypass listener startup or crash the
        # supervisor. Treat it like any adapter child failure (backoff/terminal
        # budget) and continue to bring the listener up.
        try:
            wait_ready(consumer, args.persistent_ready_file, "persistent adapter")
        except RuntimeError as error:
            terminate(consumer)
            consumer = None
            record_adapter_failure(
                consumer_state,
                "readiness",
                time.monotonic(),
                {
                    "wake": "RESTARTING",
                    "ack": "PENDING",
                    "last_error": f"persistent adapter readiness failed: {error}",
                },
            )
        try:
            wait_ready(shadow, args.shadow_ready_file, "shadow adapter")
        except RuntimeError as error:
            terminate(shadow)
            shadow = None
            record_adapter_failure(
                shadow_state,
                "readiness",
                time.monotonic(),
                {"last_error": f"shadow adapter readiness failed: {error}"},
            )
        listener = start_listener()
        if not args.persistent_command:
            consumer = start_consumer()
        listener_failures = 0
        consumer_failures = 0
        listener_restart_at = 0.0
        consumer_restart_at = 0.0
        next_health_report = 0.0
        last_health_mtime_ns = -1
        report_error = ""
        while not stopping:
            now = time.monotonic()
            health_mtime_ns = -1
            if args.health_file is not None:
                try:
                    health_mtime_ns = args.health_file.stat().st_mtime_ns
                except OSError:
                    pass
            health_changed = health_mtime_ns != last_health_mtime_ns
            if health_changed or now >= next_health_report:
                try:
                    report_runtime_health(
                        args.health_file,
                        bool(
                            args.command
                            or args.persistent_command
                            or args.shadow_persistent_command
                        ),
                    )
                    report_error = ""
                except Exception as error:
                    detail = str(error)
                    if detail != report_error:
                        print(
                            f"runtime supervisor: health report failed: {detail}",
                            file=sys.stderr,
                            flush=True,
                        )
                    report_error = detail
                last_health_mtime_ns = health_mtime_ns
                next_health_report = now + 5.0
            listener_code = listener.poll() if listener is not None else None
            consumer_code = consumer.poll() if consumer is not None else None
            shadow_code = shadow.poll() if shadow is not None else None
            if listener is not None and listener_code is not None:
                if listener_code == 0:
                    # Clean exits are deliberate (evicted/revoked identity).
                    terminate(consumer)
                    return 0
                listener_failures += 1
                delay = min(30.0, float(2 ** min(listener_failures - 1, 5)))
                print(
                    f"runtime supervisor: listener exited code={listener_code}; "
                    f"restart in {delay:.0f}s",
                    file=sys.stderr,
                    flush=True,
                )
                listener = None
                listener_restart_at = now + delay
                update_health(
                    args.health_file,
                    delivery="RESTARTING",
                    presence="DEGRADED",
                    last_transport_error=f"listener exited code={listener_code}",
                )
            if consumer is not None and consumer_code is not None:
                consumer = None
                if args.persistent_command:
                    # v2 adapter: crash-loop budget + terminal escalation.
                    record_adapter_failure(
                        consumer_state,
                        consumer_code,
                        now,
                        {
                            "wake": "RESTARTING",
                            "ack": "PENDING",
                            "last_error": (
                                f"persistent adapter exited code={consumer_code}"
                            ),
                        },
                    )
                else:
                    # v1 wake bridge: unchanged restart-forever with backoff.
                    consumer_failures += 1
                    delay = min(30.0, float(2 ** min(consumer_failures - 1, 5)))
                    print(
                        f"runtime supervisor: wake bridge exited "
                        f"code={consumer_code}; restart in {delay:.0f}s",
                        file=sys.stderr,
                        flush=True,
                    )
                    consumer_restart_at = now + delay
                    update_health(
                        args.health_file,
                        wake="RESTARTING",
                        ack="PENDING",
                        last_error=f"wake bridge exited code={consumer_code}",
                    )
            if shadow is not None and shadow_code is not None:
                shadow = None
                record_adapter_failure(shadow_state, shadow_code, now, {})
            if listener is None and now >= listener_restart_at:
                listener = start_listener()
            if args.persistent_command:
                if (
                    consumer is None
                    and not consumer_state.terminal
                    and now >= consumer_state.restart_at
                ):
                    consumer = start_consumer()
            elif (
                args.command
                and consumer is None
                and now >= consumer_restart_at
            ):
                consumer = start_consumer()
            if (
                args.shadow_persistent_command
                and shadow is None
                and not shadow_state.terminal
                and now >= shadow_state.restart_at
            ):
                shadow = start_shadow()
            time.sleep(0.2)
    finally:
        stop_children()
        for process in (shadow, consumer, listener):
            if process is None:
                continue
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=2)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
