#!/usr/bin/env python3
"""Fail when a local Markdown link points at a missing repository file."""

from __future__ import annotations

import re
import sys
from pathlib import Path
from urllib.parse import unquote


ROOT = Path(__file__).resolve().parents[1]
LINK = re.compile(r"(?<!!)\[[^\]]+\]\(([^)]+)\)")
SKIP_DIRS = {".git", "target", "dist", "node_modules"}


def markdown_files() -> list[Path]:
    return sorted(
        path
        for path in ROOT.rglob("*.md")
        if not any(part in SKIP_DIRS for part in path.relative_to(ROOT).parts)
    )


def main() -> int:
    failures: list[str] = []
    documents = markdown_files()
    for document in documents:
        for raw_target in LINK.findall(document.read_text(encoding="utf-8")):
            target = raw_target.strip().split(maxsplit=1)[0].strip("<>")
            target = unquote(target.split("#", 1)[0])
            if (
                not target
                or "://" in target
                or target.startswith(("mailto:", "/", "#"))
            ):
                continue
            resolved = (document.parent / target).resolve()
            if not resolved.exists():
                failures.append(
                    f"{document.relative_to(ROOT)} -> {raw_target}"
                )
    if failures:
        print("broken local Markdown links:", file=sys.stderr)
        for failure in failures:
            print(f"  {failure}", file=sys.stderr)
        return 1
    print(f"documentation links ok ({len(documents)} files)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
