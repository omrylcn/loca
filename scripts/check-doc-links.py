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
EMBEDDED_DOC = Path("web/docs/getting-started.md")
EMBEDDED_DOC_SOURCE = Path("docs/getting-started.md")
ONBOARDING_DOCS = ("README.md", "docs/self-host.md", "docs/getting-started.md")
RELEASE_PIN = re.compile(
    r"(?:git checkout v|LOCA_VERSION=|/releases/tag/v|release `v|\"version\":\")"
    r"(\d+\.\d+\.\d+(?:[+-][\w.-]+)?)"
)


def check_onboarding_versions(root: Path, version: str) -> list[str]:
    failures = []
    for name in ONBOARDING_DOCS:
        for pin in RELEASE_PIN.findall((root / name).read_text(encoding="utf-8")):
            if pin != version:
                failures.append(f"{name}: onboarding release {pin} differs from workspace {version}")
    return failures


def markdown_files() -> list[Path]:
    return sorted(
        path
        for path in ROOT.rglob("*.md")
        if not any(part in SKIP_DIRS for part in path.relative_to(ROOT).parts)
    )


def main() -> int:
    failures: list[str] = []
    package = (ROOT / "Cargo.toml").read_text().split("[workspace.package]", 1)[1].split("[", 1)[0]
    version = re.search(r'^version\s*=\s*"([^"]+)"', package, re.M).group(1)
    failures.extend(check_onboarding_versions(ROOT, version))
    embedded = ROOT / EMBEDDED_DOC
    embedded_source = ROOT / EMBEDDED_DOC_SOURCE
    if not embedded.exists():
        failures.append(f"missing required desktop asset: {EMBEDDED_DOC}")
    elif embedded.read_bytes() != embedded_source.read_bytes():
        failures.append(
            f"{EMBEDDED_DOC} must exactly match {EMBEDDED_DOC_SOURCE}"
        )

    documents = markdown_files()
    for document in documents:
        relative_document = document.relative_to(ROOT)
        if relative_document == EMBEDDED_DOC:
            # This is a packaging snapshot of the canonical guide. Its relative
            # links resolve from the canonical source tree, not the desktop
            # asset directory; the preflight above prevents missing/drifted copies.
            continue
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
