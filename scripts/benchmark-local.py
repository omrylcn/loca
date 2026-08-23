#!/usr/bin/env python3
"""Measure Loca's local SQLite message path without touching a real server.

The benchmark always boots a disposable loopback-only room-server with fresh
credentials and a temporary database.  It is a comparison tool, not a CI
performance assertion: record its JSON before and after storage changes.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import json
import os
from pathlib import Path
import socket
import statistics
import subprocess
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request


ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / "target" / "release" / "room-server"


def percentile(values: list[float], fraction: float) -> float:
    ordered = sorted(values)
    index = max(0, min(len(ordered) - 1, int((len(ordered) - 1) * fraction)))
    return ordered[index]


def summary_ms(values: list[float]) -> dict[str, float]:
    return {
        "p50": round(percentile(values, 0.50), 3),
        "p95": round(percentile(values, 0.95), 3),
        "max": round(max(values), 3),
        "mean": round(statistics.fmean(values), 3),
    }


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def request_json(
    base: str,
    method: str,
    path: str,
    token: str,
    body: dict[str, object] | None = None,
) -> tuple[object, float]:
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(
        base + path,
        data=data,
        method=method,
        headers={
            "content-type": "application/json",
            "x-room-token": token,
        },
    )
    started = time.perf_counter()
    try:
        with urllib.request.urlopen(req, timeout=15) as response:
            payload = json.load(response)
    except urllib.error.HTTPError as error:
        detail = error.read().decode(errors="replace")
        raise RuntimeError(f"{method} {path}: HTTP {error.code}: {detail}") from error
    return payload, (time.perf_counter() - started) * 1_000


class LocalServer:
    def __init__(self, database: Path, log: Path, port: int, token: str) -> None:
        self.database = database
        self.log = log
        self.port = port
        self.token = token
        self.process: subprocess.Popen[bytes] | None = None
        self.log_handle = None

    @property
    def base(self) -> str:
        return f"http://127.0.0.1:{self.port}"

    def start(self) -> None:
        # Do not inherit deployment knobs such as ADMIN_CONSOLE_PORT,
        # REQUIRE_SESSIONS, TLS/proxy settings, or production credentials. A
        # benchmark launched from an operator shell must remain a disposable
        # loopback process with behavior fixed by this script.
        env = {
            "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
            "BIND_ADDR": "127.0.0.1",
            "PORT": str(self.port),
            "DB_PATH": str(self.database),
            "ADMIN_TOKEN": "adm_local_benchmark_only",
            "ROOM_TOKEN": self.token,
            "RATE_LIMIT": "0",
            "RUST_LOG": "warn",
        }
        try:
            self.log_handle = self.log.open("ab")
            self.process = subprocess.Popen(
                [str(BINARY)],
                cwd=ROOT,
                env=env,
                stdout=self.log_handle,
                stderr=subprocess.STDOUT,
            )
            for _ in range(100):
                if self.process.poll() is not None:
                    raise RuntimeError(f"room-server exited; see {self.log}")
                try:
                    with urllib.request.urlopen(self.base + "/health", timeout=0.5):
                        return
                except (OSError, urllib.error.URLError):
                    time.sleep(0.05)
            raise RuntimeError(f"room-server did not become healthy; see {self.log}")
        except BaseException:
            self.stop()
            raise

    def stop(self) -> None:
        if self.process is not None and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        if self.log_handle is not None:
            self.log_handle.close()
        self.process = None
        self.log_handle = None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--messages", type=int, default=1_000)
    parser.add_argument("--rooms", type=int, default=4)
    parser.add_argument("--concurrency", type=int, default=8)
    parser.add_argument("--page-size", type=int, default=200)
    args = parser.parse_args()
    if min(args.messages, args.rooms, args.concurrency, args.page_size) < 1:
        parser.error("all numeric arguments must be positive")

    subprocess.run(
        ["cargo", "build", "--release", "--locked", "-p", "server"],
        cwd=ROOT,
        check=True,
    )

    token = "local_benchmark_room_token"
    port = free_port()
    with tempfile.TemporaryDirectory(prefix="loca-benchmark-") as temporary:
        temp = Path(temporary)
        server = LocalServer(temp / "loca.db", temp / "server.log", port, token)
        try:
            server.start()
            def post(index: int) -> float:
                room = f"bench-{index % args.rooms}"
                _, latency = request_json(
                    server.base,
                    "POST",
                    f"/rooms/{room}/messages",
                    token,
                    {
                        "sender": "benchmark",
                        "sender_type": "agent",
                        "text": f"message-{index}",
                        "op_id": f"benchmark-{index}",
                    },
                )
                return latency

            write_started = time.perf_counter()
            with concurrent.futures.ThreadPoolExecutor(
                max_workers=args.concurrency,
            ) as pool:
                write_latencies = list(pool.map(post, range(args.messages)))
            write_elapsed = time.perf_counter() - write_started

            server.stop()
            server.start()

            read_latencies: list[float] = []
            recovered = 0
            read_started = time.perf_counter()
            for room_index in range(args.rooms):
                room = f"bench-{room_index}"
                cursor = 0
                while True:
                    query = urllib.parse.urlencode(
                        {"since": cursor, "limit": args.page_size},
                    )
                    page, latency = request_json(
                        server.base,
                        "GET",
                        f"/rooms/{room}/messages?{query}",
                        token,
                    )
                    if not isinstance(page, list):
                        raise RuntimeError("message page was not a JSON list")
                    read_latencies.append(latency)
                    if not page:
                        break
                    recovered += len(page)
                    cursor = int(page[-1]["id"])
                    if len(page) < args.page_size:
                        break
            read_elapsed = time.perf_counter() - read_started
            if recovered != args.messages:
                raise RuntimeError(
                    f"restart recovered {recovered}/{args.messages} messages",
                )

            result = {
                "scope": "disposable-loopback-sqlite",
                "messages": args.messages,
                "rooms": args.rooms,
                "concurrency": args.concurrency,
                "page_size": args.page_size,
                "write": {
                    "elapsed_seconds": round(write_elapsed, 3),
                    "messages_per_second": round(args.messages / write_elapsed, 2),
                    "latency_ms": summary_ms(write_latencies),
                },
                "restart_backfill": {
                    "recovered": recovered,
                    "elapsed_seconds": round(read_elapsed, 3),
                    "pages": len(read_latencies),
                    "latency_ms": summary_ms(read_latencies),
                },
            }
            print(json.dumps(result, indent=2, sort_keys=True))
        finally:
            server.stop()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
