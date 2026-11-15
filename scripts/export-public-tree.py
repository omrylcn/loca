#!/usr/bin/env python3
"""Build the public source tree from one private commit, by allow-list.

Every file tracked at the commit must be classified: public by
packaging/public-source-paths.txt, or private by
packaging/private-source-paths.txt. A file matched by neither stops the
export and is named, so a new file can neither leak nor be dropped silently.
A file matched by both stays private.

  scripts/export-public-tree.py --check                 # classify HEAD only
  scripts/export-public-tree.py --commit SHA --output DIR

The export writes DIR/tree/ (the public candidate: committed blobs with their
modes, never the working tree) and DIR/export-manifest.json (the source
commit, the lists' hashes, each file's git blob id, SHA-256, size and mode,
and the private paths left out). DIR must not exist or be empty. It reads the
local repository only and writes nothing outside DIR: no fetch, no push, no
change to any repository.

List format: one entry per line; `#` starts a comment line. An entry with no
`*`, `?` or `[` names a file or a directory (everything under it). Otherwise
it is a glob over the whole path: `*` and `?` stay within one path segment,
`**` matches any number of segments.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys

ALLOW = "packaging/public-source-paths.txt"
DENY = "packaging/private-source-paths.txt"
EXECUTABLE, REGULAR = "100755", "100644"


class ExportError(Exception):
    pass


def git(repo, *args, data=None):
    result = subprocess.run(["git", "-C", str(repo), *args], input=data, capture_output=True, check=False)
    if result.returncode != 0:
        raise ExportError(f"git {' '.join(args)} failed: {result.stderr.decode(errors='replace').strip()}")
    return result.stdout


def compile_entry(entry):
    path = PurePosixPath(entry)
    if entry.startswith("/") or ".." in path.parts or entry != entry.strip():
        raise ExportError(f"list entry {entry!r} must be a relative path without '..'")
    if not any(char in entry for char in "*?["):
        prefix = entry.rstrip("/")
        return lambda candidate: candidate == prefix or candidate.startswith(prefix + "/")
    pattern, index = "", 0
    while index < len(entry):
        if entry.startswith("**/", index):
            pattern, index = pattern + "(?:[^/]+/)*", index + 3
        elif entry.startswith("**", index):
            pattern, index = pattern + ".*", index + 2
        elif entry[index] == "*":
            pattern, index = pattern + "[^/]*", index + 1
        elif entry[index] == "?":
            pattern, index = pattern + "[^/]", index + 1
        elif entry[index] == "[":
            end = entry.find("]", index + 1)
            if end == -1:
                raise ExportError(f"list entry {entry!r} has an unclosed '['")
            body = entry[index + 1:end]
            if body.startswith("!"):
                body = "^" + body[1:]
            pattern, index = pattern + "[" + body.replace("\\", "\\\\") + "]", end + 1
        else:
            pattern, index = pattern + re.escape(entry[index]), index + 1
    regex = re.compile(pattern + r"\Z")
    return lambda candidate: regex.match(candidate) is not None


def parse_list(text, source):
    entries = []
    for number, line in enumerate(text.splitlines(), 1):
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        try:
            entries.append((stripped, compile_entry(stripped)))
        except ExportError as error:
            raise ExportError(f"{source}:{number}: {error}") from error
    return entries


def tracked_files(repo, commit):
    files = []
    for record in git(repo, "ls-tree", "-r", "-z", "--full-tree", commit).split(b"\0"):
        if not record:
            continue
        meta, path = record.split(b"\t", 1)
        mode, kind, blob = meta.decode().split()
        files.append((path.decode("utf-8"), mode, kind, blob))
    return files


def classify(files, allow, deny):
    public, private, unclassified = [], [], []
    used = set()
    for record in files:
        path = record[0]
        denied = [entry for entry, match in deny if match(path)]
        allowed = [entry for entry, match in allow if match(path)]
        used.update(denied or allowed)
        if denied:
            private.append(path)
        elif allowed:
            public.append(record)
        else:
            unclassified.append(path)
    unused = [entry for entry, _ in allow + deny if entry not in used]
    return public, private, unclassified, unused


def read_blobs(repo, blobs):
    """Yield the content of each blob id, in order, through one git process."""
    process = subprocess.Popen(["git", "-C", str(repo), "cat-file", "--batch"],
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    try:
        for blob in blobs:
            process.stdin.write(blob.encode() + b"\n")
            process.stdin.flush()
            header = process.stdout.readline().split()
            if len(header) != 3 or header[0].decode() != blob or header[1] != b"blob":
                raise ExportError(f"cannot read blob {blob}")
            content = process.stdout.read(int(header[2]))
            process.stdout.read(1)
            yield content
    finally:
        process.stdin.close()
        process.wait()


def resolve_commit(repo, name):
    """The full id of the commit `name` names now.

    Called once, before anything is read. `HEAD` or a branch name can move
    while the export runs (a checkout or a commit in another terminal); every
    later read uses this id, so the lists and the files always come from one
    commit.
    """
    return git(repo, "rev-parse", "--verify", f"{name}^{{commit}}").decode().strip()


def export(repo, commit, output, allow_text, deny_text, check_only):
    """Export `commit`, which must be a full commit id (see resolve_commit)."""
    allow = parse_list(allow_text, ALLOW)
    deny = parse_list(deny_text, DENY)
    files = tracked_files(repo, commit)
    public, private, unclassified, unused = classify(files, allow, deny)
    print(f"source commit {commit}: {len(files)} tracked, {len(public)} public, {len(private)} private, "
          f"{len(unclassified)} unclassified")
    for entry in unused:
        print(f"warning: list entry matches no tracked file: {entry}")
    if unclassified:
        print(f"REFUSED: {len(unclassified)} tracked path(s) are in neither {ALLOW} nor {DENY}:", file=sys.stderr)
        for path in unclassified:
            print(f"  {path}", file=sys.stderr)
        print("Classify each one, commit the list, and export again. Nothing was written.", file=sys.stderr)
        return 1
    bad = [path for path, mode, kind, _ in public if kind != "blob" or mode not in (EXECUTABLE, REGULAR)]
    if bad:
        print("REFUSED: only regular files can be exported (symlink or submodule): " + ", ".join(bad),
              file=sys.stderr)
        return 1
    if check_only:
        print("classification complete; nothing was written (--check)")
        return 0
    if output.is_symlink() or (output.exists() and (not output.is_dir() or any(output.iterdir()))):
        print(f"REFUSED: --output {output} must not exist or be an empty directory", file=sys.stderr)
        return 1
    tree = output / "tree"
    tree.mkdir(parents=True)
    entries = []
    for (path, mode, _kind, blob), content in zip(public, read_blobs(repo, [r[3] for r in public])):
        target = tree / path
        target.parent.mkdir(parents=True, exist_ok=True)
        with open(target, "xb") as stream:
            stream.write(content)
        os.chmod(target, 0o755 if mode == EXECUTABLE else 0o644)
        entries.append({"path": path, "mode": mode, "git_blob": blob,
                        "sha256": hashlib.sha256(content).hexdigest(), "size": len(content)})
    manifest = {
        "format": 1,
        "source_commit": commit,
        "allow_list_sha256": hashlib.sha256(allow_text.encode()).hexdigest(),
        "deny_list_sha256": hashlib.sha256(deny_text.encode()).hexdigest(),
        "files": entries,
        "private": private,
    }
    (output / "export-manifest.json").write_text(json.dumps(manifest, indent=1, sort_keys=True) + "\n")
    total = sum(entry["size"] for entry in entries)
    print(f"exported {len(entries)} files ({total} bytes) to {tree}; left out {len(private)} private path(s)")
    print(f"manifest: {output / 'export-manifest.json'}")
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[1],
                        help="the private repository (default: the one holding this script)")
    parser.add_argument("--commit", default="HEAD", help="the approved commit to export (default: HEAD)")
    parser.add_argument("--output", type=Path, help="new or empty directory for tree/ and the manifest")
    parser.add_argument("--allow", type=Path, help=f"allow-list file (default: {ALLOW} at the commit)")
    parser.add_argument("--deny", type=Path, help=f"deny-list file (default: {DENY} at the commit)")
    parser.add_argument("--check", action="store_true", help="classify only; write nothing")
    args = parser.parse_args(argv)
    if not args.check and args.output is None:
        parser.error("--output is required unless --check is given")
    try:
        commit = resolve_commit(args.repo, args.commit)
        lists = []
        for given, default in ((args.allow, ALLOW), (args.deny, DENY)):
            if given is not None:
                lists.append(given.read_text(encoding="utf-8"))
            else:
                lists.append(git(args.repo, "show", f"{commit}:{default}").decode("utf-8"))
        output = args.output.absolute() if args.output else None
        return export(args.repo, commit, output, lists[0], lists[1], args.check)
    except (ExportError, OSError, UnicodeDecodeError) as error:
        print(f"export failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
