#!/usr/bin/env python3
"""Enforce Loca's production Rust panic-surface budget."""

from __future__ import annotations

import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "crates" / "server" / "src"
MAX_UNWRAPS = 40


def production_sources() -> list[Path]:
    return sorted(
        path
        for path in SOURCE.rglob("*.rs")
        if path.name != "tests.rs" and "tests" not in path.parts
    )


def main() -> int:
    unwraps: list[tuple[Path, int]] = []
    raw_lock_panics: list[Path] = []
    missing_invariants: list[tuple[Path, int]] = []

    for path in production_sources():
        text = path.read_text(encoding="utf-8")
        if re.search(r"\.lock\(\)\s*\.(?:unwrap\(\)|expect\()", text):
            raw_lock_panics.append(path)
        lines = text.splitlines()
        for index, line in enumerate(lines):
            if ".unwrap()" not in line:
                continue
            line_number = index + 1
            unwraps.append((path, line_number))
            context = "\n".join(lines[max(0, index - 3) : index])
            if "Invariant:" not in context:
                missing_invariants.append((path, line_number))

    failures: list[str] = []
    if len(unwraps) > MAX_UNWRAPS:
        failures.append(f"production unwrap budget exceeded: {len(unwraps)} > {MAX_UNWRAPS}")
    if raw_lock_panics:
        failures.append(
            "raw poisoned-mutex panic calls: "
            + ", ".join(str(path.relative_to(ROOT)) for path in raw_lock_panics)
        )
    if missing_invariants:
        failures.append(
            "unwraps without a nearby `Invariant:` comment: "
            + ", ".join(
                f"{path.relative_to(ROOT)}:{line}" for path, line in missing_invariants
            )
        )

    if failures:
        for failure in failures:
            print(f"panic-surface check failed: {failure}")
        return 1

    print(
        f"panic surface ok: {len(unwraps)}/{MAX_UNWRAPS} production unwraps; "
        "0 raw mutex poison panics"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
