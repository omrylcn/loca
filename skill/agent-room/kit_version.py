#!/usr/bin/env python3
"""This kit's own version (its VERSION file), as the server sees it.

The listener sends it when it connects (``?kit_version=`` on its loca and
Lobby sockets), so the Master can see which kit a connected agent runs. It is
display only: the server never reads it for authority. The status commands
compare it with the server's ``/health`` version and say so, in one line, when
this kit is older; they never fail because of it.

    kit_version.py notice SERVER_VERSION   # prints the line when older, else nothing
"""
import re
import sys
from pathlib import Path
from urllib.parse import parse_qsl, urlencode, urlsplit, urlunsplit

KIT_DIR = Path(__file__).resolve().parent
QUERY_KEY = "kit_version"
# The shape the server accepts (crates/server/src/hub/kit_version.rs).
_SHAPE = re.compile(
    r"(0|[1-9][0-9]{0,3})\.(0|[1-9][0-9]{0,3})\.(0|[1-9][0-9]{0,3})(-[0-9A-Za-z.-]{1,16})?"
)
# packaging/remote-agent/README.md "Upgrade and rollback": run from the
# unpacked remote-agent kit of the server's release; credentials stay.
UPDATE_COMMAND = "./install.sh --upgrade-only --target both"


def parse(value):
    """``(major, minor, patch)`` of a version of the accepted shape, else None."""
    if not isinstance(value, str) or len(value) > 32:
        return None
    match = _SHAPE.fullmatch(value)
    return tuple(int(part) for part in match.groups()[:3]) if match else None


def kit_version(kit_dir=KIT_DIR):
    """This kit's VERSION, when it has the accepted shape."""
    try:
        value = (Path(kit_dir) / "VERSION").read_text(encoding="utf-8").strip()
    except OSError:
        return None
    return value if parse(value) else None


# The version of the code that is running, read once when the listener
# imports this module. Re-reading VERSION at each reconnect would report a kit
# that was upgraded on disk while this process still runs the old code: the
# very case this exists to show (`install.sh --upgrade-only` does not restart
# listeners).
RUNNING_VERSION = kit_version()


def with_kit_version(url, version=None):
    """``url`` with the running kit's version in its query (unless it already
    has one)."""
    version = version if version is not None else RUNNING_VERSION
    if not version:
        return url
    parts = urlsplit(url)
    query = parse_qsl(parts.query, keep_blank_values=True)
    if any(key == QUERY_KEY for key, _ in query):
        return url
    query.append((QUERY_KEY, version))
    return urlunsplit(parts._replace(query=urlencode(query)))


def older_notice(kit, server):
    """The one line to print when ``kit`` is older than ``server``; None when
    it is equal, newer, missing or unparsable."""
    kit_core, server_core = parse(kit), parse(server)
    if not kit_core or not server_core or kit_core >= server_core:
        return None
    return (
        f"KIT OLDER THAN SERVER — kit {kit}, server {server}: it keeps working "
        "but may lack what the server added. Update it from the remote-agent "
        f"kit of the server's release: {UPDATE_COMMAND} (identities stay; "
        "restart the one runtime after)"
    )


def main(argv):
    if len(argv) >= 2 and argv[1] == "notice":
        line = older_notice(kit_version(), argv[2] if len(argv) > 2 else "")
        if line:
            print(line)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
