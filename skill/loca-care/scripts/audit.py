#!/usr/bin/env python3
"""Read-only Building presence audit for the configured Loca caretaker."""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import sys
import urllib.error
import urllib.request
from pathlib import Path


def _load_credentials_module():
    skills_root = Path(__file__).resolve().parents[2]
    candidates = (
        skills_root / "loca" / "credentials.py",
        skills_root / "agent-room" / "credentials.py",
    )
    for path in candidates:
        if not path.is_file():
            continue
        spec = importlib.util.spec_from_file_location("loca_credentials", path)
        if spec is None or spec.loader is None:
            continue
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module
    raise RuntimeError("base loca skill is missing; install loca before loca-care")


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Audit every registered Building member without exposing credentials."
    )
    parser.add_argument("--server", help="Loca http(s) origin; defaults to identity env")
    parser.add_argument("--name", default="loca-care", help="caretaker identity name")
    parser.add_argument(
        "--env", dest="env_file", help="explicit caretaker identity env file"
    )
    parser.add_argument("--format", choices=("text", "json"), default="text")
    parser.add_argument(
        "--only-problems",
        action="store_true",
        help="show only away residents and online agent runtime problems",
    )
    parser.add_argument(
        "--fail-on-away",
        action="store_true",
        help="exit 3 when at least one member is away",
    )
    parser.add_argument(
        "--fail-on-degraded",
        action="store_true",
        help="exit 4 when an online agent runtime is degraded or unverified",
    )
    return parser


def _state(row: dict) -> str:
    online = bool(row.get("online"))
    seated = bool(row.get("locas"))
    if online and seated:
        return "online/loca"
    if online:
        return "online/lobby"
    if seated:
        return "away/invited"
    return "away/lobby"


def _runtime_state(row: dict) -> str:
    """Classify model readiness independently from transport presence."""
    if row.get("kind") != "agent":
        return "not-applicable"
    runtime = row.get("runtime")
    if not isinstance(runtime, dict):
        return "unverified"
    return "healthy" if runtime.get("ready") is True else "degraded"


def _runtime_stage(runtime: object) -> str | None:
    if not isinstance(runtime, dict) or not runtime.get("attention_id"):
        return None
    if runtime.get("final_response"):
        return "responded"
    if runtime.get("turn_completed"):
        return "relay-pending"
    if runtime.get("first_response"):
        return "replied"
    if runtime.get("accepted"):
        return "accepted"
    if runtime.get("stored"):
        return "queued"
    return "unknown"


def _audit(server: str, membership: str, session: str) -> list[dict]:
    headers = {"accept": "application/json", "x-room-token": membership}
    if session:
        headers["x-session-token"] = session
    request = urllib.request.Request(
        server.rstrip("/") + "/care/residents", headers=headers
    )
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            payload = json.load(response)
    except urllib.error.HTTPError as error:
        body = error.read(200).decode("utf-8", "replace").strip()
        detail = body or error.reason
        raise RuntimeError(f"caretaker audit rejected (HTTP {error.code}): {detail}") from None
    except (urllib.error.URLError, TimeoutError) as error:
        raise RuntimeError(f"caretaker audit could not reach server: {error}") from None
    if not isinstance(payload, list):
        raise RuntimeError("caretaker audit returned an invalid response")
    rows = []
    for raw in payload:
        if not isinstance(raw, dict) or not isinstance(raw.get("name"), str):
            raise RuntimeError("caretaker audit returned an invalid resident")
        locas = raw.get("locas") if isinstance(raw.get("locas"), list) else []
        row = {
            "name": raw["name"],
            "kind": raw.get("kind", "agent"),
            "online": bool(raw.get("online")),
            "locas": [str(loca) for loca in locas],
            "runtime": raw.get("runtime") if isinstance(raw.get("runtime"), dict) else None,
        }
        row["state"] = _state(row)
        row["runtime_state"] = _runtime_state(row)
        row["runtime_stage"] = _runtime_stage(row["runtime"])
        rows.append(row)
    return sorted(rows, key=lambda row: row["name"].lower())


def main(argv: list[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    if args.env_file:
        os.environ["LOCA_ENV"] = args.env_file
    try:
        credentials = _load_credentials_module()
    except RuntimeError as error:
        print(str(error), file=sys.stderr)
        return 2
    try:
        credentials.load_local_env(args.name)
        server = (args.server or os.environ.get("ROOM_SERVER_URL", "")).rstrip("/")
        if not server:
            raise RuntimeError("server missing; set ROOM_SERVER_URL or pass --server")
        credentials.validate_server_scope(server)
        membership = os.environ.get("LOCA_MEMBERSHIP", "")
        if not membership:
            raise RuntimeError(
                "caretaker membership missing; run loca setup with loca-care identity"
            )
        rows = _audit(server, membership, os.environ.get("LOCA_SESSION", ""))
    except (RuntimeError, credentials.CredentialError) as error:
        print(str(error), file=sys.stderr)
        return 2

    away = [row for row in rows if not row["online"]]
    runtime_problems = [
        row
        for row in rows
        if row["online"]
        and row["kind"] == "agent"
        and row["runtime_state"] != "healthy"
    ]
    problems = [row for row in rows if row in away or row in runtime_problems]
    visible = problems if args.only_problems else rows
    if args.format == "json":
        print(
            json.dumps(
                {
                    "total": len(rows),
                    "online": len(rows) - len(away),
                    "away": len(away),
                    "runtime_degraded": sum(
                        row["online"] and row["runtime_state"] == "degraded"
                        for row in rows
                    ),
                    "runtime_unverified": sum(
                        row["online"] and row["runtime_state"] == "unverified"
                        for row in rows
                    ),
                    "residents": visible,
                },
                ensure_ascii=False,
                separators=(",", ":"),
            )
        )
    else:
        print(
            f"Building: {len(rows)} member, {len(rows) - len(away)} online, "
            f"{len(away)} away, {len(runtime_problems)} runtime problem"
        )
        for row in visible:
            where = ",".join(row["locas"]) or "lobby"
            runtime = row["runtime_state"]
            if row["runtime_stage"]:
                runtime += f"/{row['runtime_stage']}"
            print(f"{row['name']}\t{row['state']}\t{where}\truntime={runtime}")
    if args.fail_on_away and away:
        return 3
    if args.fail_on_degraded and runtime_problems:
        return 4
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
