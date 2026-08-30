#!/usr/bin/env python3
"""Stop exactly ONE identity's listener/Monitor — never a broad match.

A broad `pkill -f listen.py` (or any pattern kill) is FORBIDDEN: it kills every
agent's listener on the machine, and a pattern can even match the calling shell
itself. This helper instead targets only the processes whose WebSocket URL names
THIS identity on THIS server, and it guards every signal against PID reuse.

Matching (all three required, so isolation is exact):
  * the process is a loca listener/Monitor — its argv mentions `listen.py` or
    `monitor_listener.py`;
  * its argv carries a ws(s):// URL whose `name` query field EQUALS the target
    name (exact, not a prefix — `mihenk` never matches `mihenk-2`);
  * that URL's host matches the target server's host.

PID-reuse guard: for each candidate we record the kernel start-time from
/proc/<pid>/stat (field 22, after the parenthesised comm) — the identity marker.
Immediately before signalling we RE-READ it; if the process is gone or its
start-time changed, the PID was recycled into an unrelated process, so we signal
NOTHING and skip. A stale PID can therefore never cause us to kill a bystander.

Usage:
  stop_listener.py <host> <name> [--signal TERM|KILL] [--dry-run]
Exit codes:
  0  matched processes were signalled, or none were found (idempotent)
  2  usage error
"""

import os
import re
import signal
import sys
from urllib.parse import parse_qs, urlsplit

WS_RE = re.compile(r"wss?://[^\s\0]+")


def read_cmdline(pid):
    try:
        with open("/proc/%d/cmdline" % pid, "rb") as f:
            return f.read().split(b"\0")
    except (FileNotFoundError, ProcessLookupError, PermissionError, OSError):
        return None


def ppid_of(pid):
    """Parent PID from /proc/<pid>/stat field 4 (after the parenthesised comm)."""
    try:
        with open("/proc/%d/stat" % pid, "r") as f:
            data = f.read()
    except (FileNotFoundError, ProcessLookupError, PermissionError, OSError):
        return None
    rparen = data.rfind(")")
    if rparen == -1:
        return None
    rest = data[rparen + 2:].split()
    # rest[0]=state(3), rest[1]=ppid(4).
    if len(rest) < 2:
        return None
    try:
        return int(rest[1])
    except ValueError:
        return None


def ancestors():
    """The helper's own PID plus its whole parent chain up to init.

    stop must NEVER signal the process that invoked it (the connect.sh shell, the
    operator's shell, a supervising Monitor). Even though we match on a listener's
    ws-URL name, a caller's argv can COINCIDENTALLY contain that marker (a test
    harness, a copy-pasted command). Excluding the ancestry makes it impossible to
    kill the invoking chain regardless of what its command line happens to hold.
    """
    seen = set()
    pid = os.getpid()
    while pid and pid > 0 and pid not in seen:
        seen.add(pid)
        parent = ppid_of(pid)
        if parent is None or parent == pid:
            break
        pid = parent
    return seen


def starttime(pid):
    """Kernel start-time (clock ticks since boot) from /proc/<pid>/stat field 22.

    The comm field (2) is parenthesised and may contain spaces/parens, so we cut
    everything up to the LAST ')' and index the remainder, where field 22 becomes
    the 20th token.
    """
    try:
        with open("/proc/%d/stat" % pid, "r") as f:
            data = f.read()
    except (FileNotFoundError, ProcessLookupError, PermissionError, OSError):
        return None
    rparen = data.rfind(")")
    if rparen == -1:
        return None
    rest = data[rparen + 2:].split()
    # rest[0] is field 3 (state); field 22 is rest[19].
    if len(rest) < 20:
        return None
    return rest[19]


def ws_name_and_host(args):
    """Return (name, host) from the first ws(s):// URL in argv, else (None, None)."""
    for a in args:
        try:
            s = a.decode("utf-8", "replace")
        except Exception:  # noqa: BLE001
            continue
        m = WS_RE.search(s)
        if not m:
            continue
        u = urlsplit(m.group(0))
        q = parse_qs(u.query)
        names = q.get("name")
        if names:
            return names[0], u.hostname
    return None, None


def is_listener(args):
    for a in args:
        try:
            s = a.decode("utf-8", "replace")
        except Exception:  # noqa: BLE001
            continue
        if "listen.py" in s or "monitor_listener.py" in s:
            return True
    return False


def candidates(host, name):
    """PIDs whose ws URL names exactly `name` on `host`, with their start-time."""
    out = []
    skip = ancestors()  # never signal ourselves or any process that invoked us
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        pid = int(entry)
        if pid in skip:
            continue
        args = read_cmdline(pid)
        if not args or not is_listener(args):
            continue
        ws_name, ws_host = ws_name_and_host(args)
        if ws_name != name:
            continue
        # Host must match; tolerate host-only vs host:port by comparing the
        # bare hostname the server host resolves to.
        if ws_host is None or ws_host != host:
            continue
        st = starttime(pid)
        if st is None:
            continue
        out.append((pid, st))
    return out


def main(argv):
    if len(argv) < 3:
        sys.stderr.write("usage: stop_listener.py <host> <name> [--signal TERM|KILL] [--dry-run]\n")
        return 2
    host = argv[1]
    name = argv[2]
    sig = signal.SIGTERM
    dry = False
    i = 3
    while i < len(argv):
        if argv[i] == "--signal" and i + 1 < len(argv):
            sig = signal.SIGKILL if argv[i + 1].upper() in ("KILL", "SIGKILL", "9") else signal.SIGTERM
            i += 2
        elif argv[i] == "--dry-run":
            dry = True
            i += 1
        else:
            sys.stderr.write("unknown argument: %s\n" % argv[i])
            return 2
    # Bare hostname only (strip any :port the caller passed).
    host = host.split("/")[0].split(":")[0]

    cands = candidates(host, name)
    if not cands:
        print("no listener/Monitor for '%s' on '%s' — nothing to stop." % (name, host))
        return 0

    stopped, skipped = [], []
    for pid, st_before in cands:
        # PID-reuse guard: re-read the start-time immediately before signalling.
        st_now = starttime(pid)
        if st_now is None:
            skipped.append((pid, "exited before signal"))
            continue
        if st_now != st_before:
            skipped.append((pid, "start-time changed (PID reused) — NOT signalled"))
            continue
        if dry:
            stopped.append((pid, "would signal %s" % sig.name if hasattr(sig, "name") else str(sig)))
            continue
        try:
            os.kill(pid, sig)
            stopped.append((pid, "signalled %s" % (getattr(sig, "name", str(sig)))))
        except ProcessLookupError:
            skipped.append((pid, "exited before signal"))
        except PermissionError:
            skipped.append((pid, "not permitted to signal"))

    for pid, why in stopped:
        print("stopped pid=%d (%s) for '%s'" % (pid, why, name))
    for pid, why in skipped:
        print("skipped pid=%d: %s" % (pid, why))
    # Success as long as we did not fail to signal a still-matching live process.
    return 0 if not any("not permitted" in w for _, w in skipped) else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
