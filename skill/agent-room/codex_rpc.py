"""Shared stdio transport for the persistent Codex v2 adapter; no wake entrypoint."""

from __future__ import annotations

import json
import queue
import subprocess
import threading
import time
from collections import deque
from typing import Any

# A recoverable host failure (Codex's inner bwrap sandbox cannot create its
# network namespace) otherwise hides as an opaque app-server stderr dump. Name
# it so the supervisor/health file carry the fix instead of endless churn.
SANDBOX_NETNS_MARKERS = ("RTM_NEWADDR", "Operation not permitted", "namespace")


def classify_sandbox_failure(text: str) -> str | None:
    """Return a typed, actionable message when app-server stderr shows the
    Codex sandbox network-namespace denial, else ``None``."""
    if not text:
        return None
    lowered = text.lower()
    if any(marker.lower() in lowered for marker in SANDBOX_NETNS_MARKERS):
        return (
            "Codex sandbox netns unavailable; start with "
            "--codex-sandbox danger-full-access or fix host userns"
        )
    return None


class AppServer:
    def __init__(self, codex_bin: str, timeout: float) -> None:
        self.timeout = timeout
        self.pending: list[dict[str, Any]] = []
        self.stderr_tail: deque[str] = deque(maxlen=40)
        self.proc = subprocess.Popen(
            [codex_bin, "app-server", "--stdio"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
        )
        self.next_id = 1
        self.messages: queue.Queue[dict[str, Any]] = queue.Queue()
        self.reader = threading.Thread(target=self._read_stdout, daemon=True)
        self.reader.start()
        self.stderr_reader = threading.Thread(target=self._read_stderr, daemon=True)
        self.stderr_reader.start()

    def _read_stdout(self) -> None:
        assert self.proc.stdout is not None
        for line in self.proc.stdout:
            try:
                message = json.loads(line)
            except json.JSONDecodeError:
                continue
            if isinstance(message, dict):
                self.messages.put(message)

    def _read_stderr(self) -> None:
        assert self.proc.stderr is not None
        for line in self.proc.stderr:
            self.stderr_tail.append(line.rstrip())

    def failure_detail(self) -> str:
        if self.proc.poll() is not None:
            self.stderr_reader.join(timeout=0.2)
        tail = "\n".join(self.stderr_tail).strip()
        code = self.proc.poll()
        status = f"exit {code}" if code is not None else "still running"
        base = f"Codex app-server {status}" + (f":\n{tail}" if tail else "")
        # Surface the recoverable sandbox netns denial as a typed, actionable
        # cause so it is not buried in a raw stderr dump.
        typed = classify_sandbox_failure(tail)
        return f"{typed}\n{base}" if typed else base

    def close(self) -> None:
        if self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=1)
            except subprocess.TimeoutExpired:
                self.proc.kill()

    def send_notification(self, method: str, params: dict[str, Any]) -> None:
        assert self.proc.stdin is not None
        self.proc.stdin.write(json.dumps({"method": method, "params": params}) + "\n")
        self.proc.stdin.flush()

    def read_message(self, deadline: float) -> dict[str, Any] | None:
        while time.monotonic() < deadline:
            try:
                return self.messages.get(
                    timeout=min(0.2, max(0.0, deadline - time.monotonic()))
                )
            except queue.Empty:
                if self.proc.poll() is not None:
                    break
                continue
        return None

    def request(self, method: str, params: dict[str, Any]) -> dict[str, Any]:
        request_id = self.next_id
        self.next_id += 1
        assert self.proc.stdin is not None
        assert self.proc.stdout is not None
        self.proc.stdin.write(
            json.dumps({"method": method, "id": request_id, "params": params}) + "\n"
        )
        self.proc.stdin.flush()

        deadline = time.monotonic() + self.timeout
        while time.monotonic() < deadline:
            response = self.read_message(deadline)
            if response is None:
                if self.proc.poll() is not None:
                    raise RuntimeError(self.failure_detail())
                continue
            if response.get("id") == request_id:
                return response
            self.pending.append(response)
        raise TimeoutError(f"Codex app-server timed out during {method}")

    def wait_for_turn(
        self,
        thread_id: str,
        turn_id: str | None,
        timeout: float,
    ) -> dict[str, Any]:
        """Keep app-server alive until the model turn actually finishes.

        ``timeout`` is an inactivity timeout, not a cap on useful model work.
        Codex turns may legitimately spend longer than five minutes running
        tests or builds.  App-server emits progress while that work is alive,
        so each event renews the deadline.  The outer runtime consumer still
        provides a hard process cap for a genuinely wedged adapter.
        """
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            message = self.pending.pop(0) if self.pending else self.read_message(deadline)
            if message is None:
                if self.proc.poll() is not None:
                    raise RuntimeError(self.failure_detail())
                continue
            deadline = time.monotonic() + timeout
            method = message.get("method")
            params = message.get("params")
            if (
                method == "turn/completed"
                and isinstance(params, dict)
                and params.get("threadId") == thread_id
            ):
                turn = params.get("turn")
                if not isinstance(turn, dict):
                    continue
                if turn_id is None or turn.get("id") == turn_id:
                    return turn
            if "id" in message and method:
                raise RuntimeError(
                    f"Codex turn blocked waiting for client request: {method}"
                )
        raise TimeoutError("Codex app-server timed out waiting for turn/completed")


def response_error(response: dict[str, Any]) -> str:
    if "error" not in response:
        return ""
    error = response.get("error")
    if isinstance(error, dict):
        return str(error.get("message") or error or "unknown app-server error")
    return str(error or "unknown app-server error")
