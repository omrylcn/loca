#!/usr/bin/env python3
"""Central credential policy for skill bundle safety.

One place lists every credential class the server actually mints, plus the
bound-credential env assignments (legacy ROOM_TOKEN and friends). The packaging
script scans a staged bundle with this; the tests import the same policy so the
gate and its fixtures can never drift apart.

usage: credential_scan.py <dir>   -> exits 3 (and names the offending FILE, never
                                     the value) if any credential-shaped content
                                     is found; 0 otherwise.
"""

import os
import re
import sys

# Every real token prefix `Hub::secure_token` / the store mint with a lowercase
# hex body (>= 16 bytes = 32 hex; 16+ here catches any shorter future token
# without matching the skill's own code, which only NAMES these as prefixes).
TOKEN_PREFIXES = ("mb_", "dv_", "jrs_", "st_", "sm_", "ak_", "pair_", "adm_")

# Bound credential keys whose assignment to a REAL value (a legacy ROOM_TOKEN, a
# hardcoded membership/session, a davet) is itself a leak — regardless of prefix.
ASSIGN_KEYS = ("ROOM_TOKEN", "LOCA_MEMBERSHIP", "LOCA_SESSION", r"DAVET_[A-Za-z0-9_]+")

_PREFIX_RE = re.compile(
    r"(?:%s)[a-f0-9]{16,}" % "|".join(re.escape(p) for p in TOKEN_PREFIXES)
)
# Catch a bound credential assigned a REAL value in any common form: a shell/env
# line (optional `export`, `=`, optional quotes) or a JSON/YAML entry (quoted or
# bare key, `:` separator). The value must be a realistic secret (16+ chars), so
# the skill's own `^KEY=` sed anchors and `"KEY"` list entries never match.
_ASSIGN_RE = re.compile(
    r"""(?x)
    ["']?(?:%s)["']?        # the bound key, optionally quoted (JSON/YAML)
    \s*[:=]\s*              # `=` (shell/env) or `:` (JSON/YAML)
    ["']?[^\s"',{}]{16,}    # a real value, unwrapping one optional quote
    """ % "|".join(ASSIGN_KEYS)
)


def scan_text(text):
    """Return a short CLASS label (never the value) for the first hit, else None."""
    m = _PREFIX_RE.search(text)
    if m:
        return m.group(0).split("_", 1)[0] + "_ token"
    m = _ASSIGN_RE.search(text)
    if m:
        key = re.split(r"[:=]", m.group(0), 1)[0].strip().strip("\"'")
        return key + " assignment"
    return None


def scan_tree(root):
    """List (relative_path, class_label) for every file that carries a credential."""
    hits = []
    for dirpath, _, filenames in os.walk(root):
        for filename in filenames:
            full = os.path.join(dirpath, filename)
            try:
                with open(full, "r", encoding="utf-8", errors="replace") as f:
                    text = f.read()
            except OSError:
                continue
            label = scan_text(text)
            if label:
                hits.append((os.path.relpath(full, root), label))
    return hits


def main(argv):
    if len(argv) != 2:
        sys.stderr.write("usage: credential_scan.py <dir>\n")
        return 2
    hits = scan_tree(argv[1])
    for path, label in hits:
        # Name the file and the credential CLASS — never the value itself.
        sys.stderr.write("credential-shaped content (%s) in %s\n" % (label, path))
    return 3 if hits else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
