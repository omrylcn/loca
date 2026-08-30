#!/usr/bin/env python3
"""Build deterministic, credential-safe skill bundles on every supported OS.

This is the platform-neutral implementation used by Cargo build.rs.  The shell
entry point remains as a compatibility wrapper for existing callers.
"""

import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

import credential_scan


SKILLS = {"loca": "skill/agent-room", "loca-care": "skill/loca-care"}
EXECUTABLES = {
    "skill/agent-room/connect.sh",
    "skill/agent-room/monitor_listener.py",
    "skill/agent-room/nudge.py",
    "skill/agent-room/runtime.sh",
    "skill/loca-care/scripts/audit.py",
}
DEFAULT_EPOCH = 315532800  # 1980-01-01, ZIP's timestamp floor


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(65536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def app_version(root):
    text = (root / "Cargo.toml").read_text(encoding="utf-8")
    match = re.search(
        r"(?ms)^\[workspace\.package\]\s*.*?^version\s*=\s*\"([^\"]+)\"", text
    )
    if not match:
        raise RuntimeError("workspace.package version is missing from Cargo.toml")
    return match.group(1)


def selected_files(root):
    manifest = root / "scripts" / "skill-bundle-files.txt"
    if manifest.is_file():
        return [line.strip() for line in manifest.read_text(encoding="utf-8").splitlines()
                if line.strip() and not line.lstrip().startswith("#")]
    try:
        output = subprocess.check_output(
            [sys.executable, str(root / "scripts" / "skill_bundle_files.py"), str(root)],
            text=True,
        )
    except (OSError, subprocess.CalledProcessError) as exc:
        raise RuntimeError(
            "needs scripts/skill-bundle-files.txt or a git work tree"
        ) from exc
    return [line for line in output.splitlines() if line]


def zip_datetime(epoch):
    import datetime
    value = datetime.datetime.fromtimestamp(epoch, datetime.timezone.utc)
    if value.year < 1980:
        value = value.replace(year=1980, month=1, day=1, hour=0, minute=0, second=0)
    return (value.year, value.month, value.day, value.hour, value.minute,
            value.second - value.second % 2)


def build_one(root, out, name, source_rel, files, version, epoch):
    stage = Path(tempfile.mkdtemp(prefix=".build.", dir=str(out)))
    try:
        kit = stage / name
        kit.mkdir()
        prefix = source_rel + "/"
        for path_text in files:
            if not path_text.startswith(prefix):
                continue
            rel = path_text[len(prefix):]
            if rel.endswith((".env", ".request")) or rel in (".env", ".request"):
                raise SystemExit("REFUSING: credential-container file '%s' must not be in a skill bundle" % path_text)
            source = root / Path(path_text)
            if source.is_symlink():
                raise SystemExit("REFUSING: '%s' is a symlink; skill bundles must ship only regular files" % path_text)
            if not source.is_file():
                raise SystemExit("REFUSING: '%s' is listed for the bundle but missing from the tree" % path_text)
            target = kit / Path(rel)
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, target)
            target.chmod(0o700 if path_text in EXECUTABLES else 0o644)

        hits = credential_scan.scan_tree(str(kit))
        if hits:
            for path, label in hits:
                sys.stderr.write("credential-shaped content (%s) in %s\n" % (label, path))
            raise SystemExit("REFUSING: a credential-shaped value was found in the '%s' bundle" % name)

        archive = stage / (name + ".zip")
        timestamp = zip_datetime(epoch)
        # Store rather than deflate: zlib output may vary between OS/library
        # versions, while ZIP_STORED makes the archive byte-identical everywhere.
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_STORED) as zipped:
            for source in sorted(kit.rglob("*"), key=lambda p: p.as_posix()):
                if not source.is_file():
                    continue
                arcname = (Path(name) / source.relative_to(kit)).as_posix()
                info = zipfile.ZipInfo(arcname, timestamp)
                info.create_system = 3
                source_rel_path = source.relative_to(kit).as_posix()
                original = source_rel + "/" + source_rel_path
                mode = 0o700 if original in EXECUTABLES else 0o644
                info.external_attr = (stat.S_IFREG | mode) << 16
                info.compress_type = zipfile.ZIP_STORED
                with source.open("rb") as stream:
                    zipped.writestr(info, stream.read(), compress_type=zipfile.ZIP_STORED)

        entries = []
        for source in sorted(kit.rglob("*"), key=lambda p: p.as_posix()):
            if source.is_file():
                entries.append({
                    "path": (Path(name) / source.relative_to(kit)).as_posix(),
                    "sha256": sha256(source),
                })
        skill_version_file = kit / "VERSION"
        manifest = {
            "name": name,
            "skill_version": (skill_version_file.read_text(encoding="utf-8").strip()
                              if skill_version_file.exists() else "unknown"),
            "app_version": version,
            "bundle_sha256": sha256(archive),
            "files": entries,
        }
        manifest_path = stage / (name + ".manifest.json")
        manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        os.replace(archive, out / archive.name)
        os.replace(manifest_path, out / manifest_path.name)
        print("%s -> %s" % (name, out / (name + ".zip")))
    finally:
        shutil.rmtree(stage, ignore_errors=True)


def main(argv):
    root = Path(__file__).resolve().parent.parent
    out = Path(argv[1]).resolve() if len(argv) > 1 else root / "dist" / "skill-bundles"
    raw_epoch = os.environ.get("SOURCE_DATE_EPOCH", str(DEFAULT_EPOCH))
    if not raw_epoch.isdigit():
        sys.stderr.write("SOURCE_DATE_EPOCH must be an integer Unix timestamp\n")
        return 2
    out.mkdir(parents=True, exist_ok=True)
    files = selected_files(root)
    version = app_version(root)
    for name in sorted(SKILLS):
        build_one(root, out, name, SKILLS[name], files, version, int(raw_epoch))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv))
    except SystemExit as exc:
        if isinstance(exc.code, str):
            sys.stderr.write(exc.code + "\n")
            # Preserve the legacy security-gate exit classes.
            if "credential-shaped" in exc.code:
                sys.exit(3)
            if "symlink" in exc.code:
                sys.exit(4)
            if "credential-container" in exc.code:
                sys.exit(5)
            if "missing from the tree" in exc.code:
                sys.exit(6)
        raise
