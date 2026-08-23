#!/usr/bin/env python3
"""Keep one Claude Monitor listener alive and record why it exits."""

from __future__ import annotations

import argparse
import fcntl
import json
import os
import signal
import subprocess
import time
from collections import deque
from datetime import datetime, timezone
from pathlib import Path
from typing import TextIO


def timestamp() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def signal_label(code: int) -> str:
    signum = -code if code < 0 else code - 128 if code > 128 else 0
    if signum <= 0:
        return "none"
    try:
        name = signal.Signals(signum).name
    except ValueError:
        name = f"signal-{signum}"
    suffix = "" if code < 0 else " (encoded)"
    return f"{name}{suffix}"


def append_log(log: TextIO, message: str) -> None:
    log.write(f"{timestamp()} {message}\n")
    log.flush()


def emit_terminal_error(name: str, reason: str) -> None:
    event = {"t": "monitor_error", "name": name, "reason": reason}
    try:
        print(json.dumps(event, ensure_ascii=False), flush=True)
    except (BrokenPipeError, OSError):
        pass


def open_private(path: Path) -> TextIO:
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    os.chmod(path.parent, 0o700)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
    os.chmod(path, 0o600)
    return os.fdopen(fd, "a", encoding="utf-8")


def legacy_listener_output(command: list[str]) -> str | None:
    """Return a non-stdout listen.py sink that cannot wake native Monitor."""
    for index, value in enumerate(command):
        if Path(value).name != "listen.py":
            continue
        output_index = index + 2
        if output_index >= len(command):
            return "missing listener output argument"
        output = command[output_index]
        if output not in ("-", "/dev/stdout", "/dev/fd/1"):
            return output
        return None
    return None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--name", required=True)
    parser.add_argument("--log", type=Path, required=True)
    parser.add_argument("--lock", type=Path, required=True)
    parser.add_argument("--max-restarts", type=int, default=6)
    parser.add_argument("--stable-seconds", type=float, default=30.0)
    parser.add_argument("--failure-window", type=float, default=300.0)
    parser.add_argument("--base-delay", type=float, default=1.0)
    parser.add_argument("--max-delay", type=float, default=10.0)
    parser.add_argument("listener", nargs=argparse.REMAINDER)
    args = parser.parse_args()

    command = list(args.listener)
    if command and command[0] == "--":
        command.pop(0)
    if not command:
        parser.error("listener command is required after --")
    legacy_output = legacy_listener_output(command)
    if legacy_output is not None:
        reason = (
            "Claude Monitor listener output must be /dev/stdout (or '-'); "
            f"project-local sink {legacy_output!r} delivers without waking"
        )
        with open_private(args.log) as log:
            append_log(log, reason)
        emit_terminal_error(args.name, reason)
        return 64
    if args.max_restarts < 0:
        parser.error("--max-restarts must be non-negative")
    if min(
        args.stable_seconds,
        args.failure_window,
        args.base_delay,
        args.max_delay,
    ) < 0:
        parser.error("timing values must be non-negative")

    args.lock.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    os.chmod(args.lock.parent, 0o700)
    lock_fd = os.open(args.lock, os.O_WRONLY | os.O_CREAT, 0o600)
    os.chmod(args.lock, 0o600)
    try:
        fcntl.flock(lock_fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        with open_private(args.log) as log:
            append_log(log, "duplicate monitor supervisor refused")
        emit_terminal_error(args.name, "duplicate monitor supervisor")
        os.close(lock_fd)
        return 75

    with open_private(args.log) as log:
        child: subprocess.Popen[bytes] | None = None
        stopping = False
        stop_signum = 0

        def request_stop(signum: int, _frame: object) -> None:
            nonlocal stopping, stop_signum
            stopping = True
            stop_signum = signum
            if child is not None and child.poll() is None:
                child.terminate()

        handled = [signal.SIGTERM, signal.SIGINT, signal.SIGHUP, signal.SIGQUIT]
        for optional in ("SIGUSR1", "SIGUSR2", "SIGSTKFLT"):
            value = getattr(signal, optional, None)
            if value is not None:
                handled.append(value)
        for handled_signal in set(handled):
            signal.signal(handled_signal, request_stop)

        consecutive_failures = 0
        recent_failures: deque[float] = deque()
        append_log(log, f"supervisor started pid={os.getpid()} name={args.name}")
        try:
            while not stopping:
                started = time.monotonic()
                child = subprocess.Popen(
                    command,
                    stdin=subprocess.DEVNULL,
                    stdout=None,
                    stderr=log,
                )
                append_log(log, f"listener started pid={child.pid}")
                while child.poll() is None and not stopping:
                    time.sleep(0.1)

                if stopping:
                    if child.poll() is None:
                        child.terminate()
                    try:
                        child.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        child.kill()
                        child.wait(timeout=2)
                    break

                code = child.returncode
                lived = time.monotonic() - started
                append_log(
                    log,
                    "listener exited "
                    f"exit_code={code} signal={signal_label(code)} "
                    f"lifetime_seconds={lived:.2f}",
                )
                if code == 0:
                    append_log(log, "clean listener exit; supervisor stopping")
                    return 0
                if lived >= args.stable_seconds:
                    consecutive_failures = 0
                consecutive_failures += 1
                failed_at = time.monotonic()
                recent_failures.append(failed_at)
                cutoff = failed_at - args.failure_window
                while recent_failures and recent_failures[0] < cutoff:
                    recent_failures.popleft()
                if len(recent_failures) > args.max_restarts:
                    reason = (
                        "listener restart budget exhausted after "
                        f"{len(recent_failures)} failures in "
                        f"{args.failure_window:.0f}s; "
                        f"last exit={code} signal={signal_label(code)}"
                    )
                    append_log(log, reason)
                    emit_terminal_error(args.name, reason)
                    return 1

                delay = min(
                    args.max_delay,
                    args.base_delay * (2 ** max(0, consecutive_failures - 1)),
                )
                append_log(log, f"listener restart in {delay:.2f}s")
                deadline = time.monotonic() + delay
                while not stopping and time.monotonic() < deadline:
                    time.sleep(min(0.1, max(0.0, deadline - time.monotonic())))
        finally:
            if child is not None and child.poll() is None:
                child.terminate()
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait(timeout=2)
            label = signal.Signals(stop_signum).name if stop_signum else "none"
            append_log(log, f"supervisor stopped signal={label}")
            os.close(lock_fd)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
