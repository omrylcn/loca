#!/usr/bin/env python3
"""Emit a skill bundle manifest with a SAFE JSON serializer (no hand-built JSON,
so a filename with a quote/backslash can never corrupt the output).

usage: skill_manifest.py <name> <app_version> <staged_dir> <zip_path>

`staged_dir` is the per-skill staging root (e.g. .../loca); manifest paths are
reported WITH that leading component (`loca/...`) so they match the archive
entries exactly. Prints the manifest JSON to stdout.
"""

import hashlib
import json
import os
import sys


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def main(argv):
    name, app_version, staged_dir, zip_path = argv[1:5]
    skill_version = "unknown"
    version_file = os.path.join(staged_dir, "VERSION")
    if os.path.exists(version_file):
        with open(version_file, encoding="utf-8") as f:
            skill_version = f.read().strip()

    base = os.path.dirname(os.path.abspath(staged_dir))  # so paths keep the name/ prefix
    files = []
    for dirpath, _, filenames in os.walk(staged_dir):
        for filename in filenames:
            full = os.path.join(dirpath, filename)
            files.append({
                "path": os.path.relpath(full, base),
                "sha256": sha256_file(full),
            })
    files.sort(key=lambda entry: entry["path"])

    manifest = {
        "name": name,
        "skill_version": skill_version,
        "app_version": app_version,
        "bundle_sha256": sha256_file(zip_path),
        "files": files,
    }
    print(json.dumps(manifest, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
