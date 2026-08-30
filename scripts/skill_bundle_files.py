#!/usr/bin/env python3
"""Single source of truth for WHICH files each skill bundle ships.

The bundle file set is the git-tracked runtime files of each skill tree, minus
tests and Python caches. Centralising that selection here keeps three consumers
in lockstep:

  * ``--write`` regenerates the committed manifest ``scripts/skill-bundle-files.txt``;
  * ``build-skill-bundles.sh`` reads that committed manifest, so it needs NO
    ``.git`` and works inside a Docker build context with no repository; and
  * the packaging test asserts the committed manifest still equals what
    ``git ls-files`` yields, so a tracked skill file added or removed without
    regenerating the manifest fails CI (no silent drift, and the Docker build
    can never ship a stale set).

Usage:  skill_bundle_files.py <repo-root> [--write | --check]
  (no flag)  print the git-derived bundle file list (used by the git fallback)
  --write    rewrite the committed manifest from git and print it
  --check    exit nonzero if the committed manifest has drifted from git
"""
import os
import subprocess
import sys

# bundle name -> source tree, ROOT-relative.
SKILL_SRC = {
    "loca": "skill/agent-room",
    "loca-care": "skill/loca-care",
}

MANIFEST_REL = "scripts/skill-bundle-files.txt"

_HEADER = [
    "# GENERATED — the exact tracked runtime files each skill bundle ships.",
    "# Regenerate with:  python3 scripts/skill_bundle_files.py . --write",
    "# The packaging test asserts this equals `git ls-files` (minus tests/caches);",
    "# build-skill-bundles.sh reads THIS file so a Docker build needs no .git.",
]


def is_excluded(rel):
    """`rel` is relative to the skill tree (e.g. ``tests/x.py``). Tests and Python
    caches never ship. `.env`/`.request` files and symlinks are REFUSED loudly at
    build time (exit 5 / 4), not silently excluded, so they are deliberately NOT
    filtered here — a credential container tracked in a skill tree must fail the
    build, never quietly drop out of it."""
    parts = rel.split("/")
    if "tests" in parts or "__pycache__" in parts:
        return True
    return parts[-1].endswith(".pyc")


def tracked_files(root):
    """The sorted, ROOT-relative bundle file set derived from git. Requires a git
    work tree; used only where one exists (manifest generation and the drift
    test), never inside a Docker build."""
    files = []
    for src in SKILL_SRC.values():
        result = subprocess.run(
            ["git", "-C", root, "ls-files", "-z", "--", src],
            check=True, stdout=subprocess.PIPE,
        )
        for raw in result.stdout.split(b"\0"):
            if not raw:
                continue
            path = raw.decode()
            rel = path[len(src) + 1:]
            if not is_excluded(rel):
                files.append(path)
    return sorted(files)


def manifest_path(root):
    return os.path.join(root, MANIFEST_REL)


def read_manifest(root):
    """The committed bundle file set — one path per line, blanks and ``#``-comments
    ignored. Needs no git; this is what the Docker build reads."""
    files = []
    with open(manifest_path(root), encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if line and not line.startswith("#"):
                files.append(line)
    return sorted(files)


def write_manifest(root):
    files = tracked_files(root)
    with open(manifest_path(root), "w", encoding="utf-8") as fh:
        fh.write("\n".join(_HEADER) + "\n")
        fh.write("\n".join(files) + "\n")
    return files


def main(argv):
    root = argv[1] if len(argv) > 1 else "."
    flag = argv[2] if len(argv) > 2 else ""
    if flag == "--write":
        for path in write_manifest(root):
            print(path)
        return 0
    if flag == "--check":
        want, have = tracked_files(root), read_manifest(root)
        if want != have:
            sys.stderr.write(
                "skill bundle manifest is STALE — run: "
                "python3 scripts/skill_bundle_files.py . --write\n")
            return 1
        return 0
    for path in tracked_files(root):
        print(path)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
