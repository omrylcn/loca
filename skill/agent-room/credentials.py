#!/usr/bin/env python3
"""Machine-local loca credential selection and server scoping.

One machine may host several agents.  Each identity therefore gets its own
``~/.loca/<name>.env`` file, while ``~/.loca/env`` remains the compatibility
default for a single-agent machine.  Credentials are also bound to the server
recorded in that file: a production davet must never be sent to localhost (or
to any other origin) because a caller supplied the wrong URL.
"""

import argparse
import json
import os
import re
import sys
import tempfile
from urllib.parse import urlparse

from portable_lock import lock_file


class CredentialError(RuntimeError):
    """A local identity cannot safely be used for this invocation."""


_BOUND_KEYS = {
    "LOCA_NAME",
    "LOCA_SESSION",
    "LOCA_MEMBERSHIP",
    "ROOM_SERVER_URL",
    "ROOM_TOKEN",
}


def _is_bound_key(key):
    return key in _BOUND_KEYS or bool(re.fullmatch(r"DAVET_[A-Za-z0-9_]+", key))


def _validate_write_value(value):
    if value is None:
        return
    if not isinstance(value, str):
        raise CredentialError("credential values must be strings or null")
    if "\n" in value or "\r" in value or any(ord(char) == 0 for char in value):
        raise CredentialError("credential values must fit on one env-file line")


def _literal_env_value(raw_value, path, line_number):
    """Parse one value without interpreting shell syntax or escapes."""
    value = raw_value.strip()
    if len(value) >= 2 and value[0] == value[-1] and value[0] in "\"'":
        value = value[1:-1]
    if any(ord(char) < 32 and char != "\t" for char in value):
        raise CredentialError(
            f"invalid control character in {path}:{line_number}"
        )
    return value


def identity_filename(name):
    """Map an agent display name to a safe, stable env-file stem."""
    return re.sub(r"[^A-Za-z0-9_-]", "_", (name or "").strip())


def update_env_values(path, updates):
    """Atomically update one identity file under a cross-process lock.

    ``None`` deletes a key.  Every Python listener thread and every shell
    helper goes through this function, so a Lobby call, session renewal, and
    release cannot overwrite each other's credentials with stale snapshots.
    """
    if not isinstance(updates, dict) or not updates:
        return
    for key, value in updates.items():
        if not _is_bound_key(key):
            raise CredentialError(f"invalid credential key {key!r}")
        _validate_write_value(value)

    path = os.path.abspath(os.path.expanduser(path))
    parent = os.path.dirname(path)
    os.makedirs(parent, mode=0o700, exist_ok=True)
    lock_path = path + ".lock"
    lock_fd = os.open(lock_path, os.O_RDWR | os.O_CREAT, 0o600)
    temp_path = None
    try:
        os.chmod(lock_path, 0o600)
        lock_file(lock_fd)
        try:
            with open(path, encoding="utf-8") as handle:
                existing = handle.read().splitlines()
        except FileNotFoundError:
            existing = []

        kept = []
        for line_number, line in enumerate(existing, start=1):
            stripped = line.strip()
            if not stripped or stripped.startswith("#"):
                kept.append(line)
                continue
            if "=" not in line:
                raise CredentialError(
                    f"invalid credential line in {path}:{line_number}"
                )
            key = line.split("=", 1)[0].strip()
            if not _is_bound_key(key):
                raise CredentialError(
                    f"invalid credential key {key!r} in {path}:{line_number}"
                )
            if key not in updates:
                kept.append(line)

        for key, value in updates.items():
            if value is not None and value != "":
                kept.append(f"{key}={value}")

        fd, temp_path = tempfile.mkstemp(
            prefix=f".{os.path.basename(path)}.", suffix=".tmp", dir=parent
        )
        # fchmod is Unix-only. Windows keeps this file inside the user's
        # profile; the atomic replace and local ACL remain authoritative.
        if hasattr(os, "fchmod"):
            os.fchmod(fd, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            handle.write("\n".join(kept))
            if kept:
                handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temp_path, path)
        temp_path = None
        os.chmod(path, 0o600)
        # POSIX permits opening/fsyncing a directory to make the rename durable.
        # Windows rejects directory file descriptors even after the file was
        # successfully replaced, which previously reported a false permission
        # failure after writing a complete identity.
        if os.name != "nt":
            directory_fd = os.open(parent, os.O_RDONLY)
            try:
                os.fsync(directory_fd)
            finally:
                os.close(directory_fd)
    finally:
        if temp_path:
            try:
                os.unlink(temp_path)
            except FileNotFoundError:
                pass
        os.close(lock_fd)


def _main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "command",
        choices=("update", "update-json"),
        help="read NUL-separated key/value pairs or one JSON object",
    )
    parser.add_argument("path", help="identity env file to update")
    args = parser.parse_args()
    try:
        if args.command == "update-json":
            updates = json.load(sys.stdin)
            if not isinstance(updates, dict):
                raise CredentialError("update-json input must be one object")
        else:
            raw = sys.stdin.buffer.read().split(b"\0")
            if raw and raw[-1] == b"":
                raw.pop()
            if len(raw) % 2:
                parser.error("update input must contain NUL-separated key/value pairs")
            updates = {}
            for index in range(0, len(raw), 2):
                key = raw[index].decode("utf-8")
                value = raw[index + 1].decode("utf-8")
                updates[key] = value if value else None
        update_env_values(args.path, updates)
    except (CredentialError, UnicodeDecodeError, json.JSONDecodeError, OSError) as error:
        print(f"credential update failed: {error}", file=sys.stderr)
        return 2
    return 0


def token_for_room(room):
    """Return this loca's davet, falling back to the legacy building key."""
    suffix = re.sub(r"[^A-Za-z0-9_]", "_", room)
    return os.environ.get("DAVET_" + suffix) or os.environ.get("ROOM_TOKEN", "")


def load_local_env(name=None):
    """Load the requested identity's credential file.

    ``$LOCA_ENV`` is an explicit caller choice.  Otherwise a name-specific file
    wins over the compatibility default.  An explicit/named identity is
    authoritative: inherited loca variables from another agent are cleared
    before the file is read.
    """
    explicit = os.environ.get("LOCA_ENV")
    loca_dir = os.path.join(os.path.expanduser("~"), ".loca")
    stem = identity_filename(name)
    named = os.path.join(loca_dir, stem + ".env") if stem else ""
    use_named = bool(named and os.path.isfile(named))
    path = explicit or (named if use_named else os.path.join(loca_dir, "env"))
    authoritative = bool(explicit or use_named)

    if authoritative:
        for key in list(os.environ):
            if _is_bound_key(key):
                os.environ.pop(key, None)

    loaded = False
    try:
        with open(path, encoding="utf-8") as fh:
            for line_number, raw in enumerate(fh, start=1):
                line = raw.strip()
                if not line or line.startswith("#"):
                    continue
                if "=" not in line:
                    raise CredentialError(
                        f"invalid credential line in {path}:{line_number}"
                    )
                key, value = line.split("=", 1)
                key = key.strip()
                if not _is_bound_key(key):
                    raise CredentialError(
                        f"invalid credential key {key!r} in {path}:{line_number}"
                    )
                value = _literal_env_value(value, path, line_number)
                if authoritative:
                    os.environ[key] = value
                else:
                    os.environ.setdefault(key, value)
        loaded = True
        os.environ["LOCA_ENV"] = path
    except OSError:
        pass

    loaded_name = os.environ.get("LOCA_NAME", "").strip()
    if loaded and name and loaded_name and loaded_name != name:
        raise CredentialError(
            f"credential identity mismatch: requested '{name}', "
            f"but {path} belongs to '{loaded_name}'; set LOCA_ENV explicitly"
        )
    return path if loaded else None


def _origin(value):
    parsed = urlparse((value or "").strip().rstrip("/"))
    scheme = {"ws": "http", "wss": "https"}.get(parsed.scheme, parsed.scheme)
    if scheme not in ("http", "https") or not parsed.hostname:
        raise CredentialError(f"invalid loca server URL: {value!r}")
    port = parsed.port or (443 if scheme == "https" else 80)
    return scheme, parsed.hostname.lower(), port


def validate_server_scope(target_url):
    """Refuse to send loaded credentials to a different server origin."""
    expected = os.environ.get("ROOM_SERVER_URL", "").strip()
    if not expected:
        return
    if _origin(expected) != _origin(target_url):
        raise CredentialError(
            f"credential server mismatch: identity belongs to {expected}, "
            f"refusing target {target_url}"
        )


if __name__ == "__main__":
    raise SystemExit(_main())
