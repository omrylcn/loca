"""The kit's DAVET_<loca> key, pinned by the table the server also reads.

The kit keeps a loca's invite under ``DAVET_`` plus the loca's name with every
character other than ``A-Za-z0-9_`` turned into ``_``. Two locas whose names
map to one key would share one invite line and overwrite each other, so the
server refuses a new loca whose key an existing loca already has
(``crates/server/src/hub/loca_name.rs``: ``kit_key`` and ``kit_key_shell``,
tested against ``fixtures/kit_keys.json``). This test holds every spelling of
the mapping in the kit to that same table: Python's (``listen.py``,
``credentials.py``) maps per character, ``connect.sh``'s ``tr`` per byte, and
``setup.ps1`` uses the Python pattern.
"""
import json
import os
import pathlib
import subprocess
import sys
import unittest
from unittest import mock

KIT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(KIT))

import credentials  # noqa: E402
import listen  # noqa: E402

CASES = json.loads((KIT / "tests" / "fixtures" / "kit_keys.json").read_text(encoding="utf-8"))


class KitKeyTests(unittest.TestCase):
    def test_the_table_holds_the_collisions_the_server_refuses(self):
        keys = {}
        for case in CASES:
            keys.setdefault(case["python"], []).append(case["name"])
        self.assertEqual(sorted(keys["my_loca"]), ["my loca", "my-loca", "my.loca", "my_loca"])

    def test_listener_key(self):
        for case in CASES:
            self.assertEqual(listen.room_env_key(case["name"]), "DAVET_" + case["python"], case)

    def test_credentials_key(self):
        for case in CASES:
            env = {"DAVET_" + case["python"]: "dv_synthetic", "ROOM_TOKEN": ""}
            with mock.patch.dict(os.environ, env, clear=True):
                self.assertEqual(credentials.token_for_room(case["name"]), "dv_synthetic", case)

    def test_connect_sh_var(self):
        script = 'eval "$(sed -n "/^_var() {/p" "$1")"; shift; for n in "$@"; do _var "$n"; printf "\\n"; done'
        names = [case["name"] for case in CASES]
        out = subprocess.run(
            ["bash", "-c", script, "_", str(KIT / "connect.sh"), *names],
            capture_output=True, check=True,
        ).stdout.decode("utf-8").split("\n")[:-1]
        self.assertEqual(out, [case["shell"] for case in CASES])

    def test_setup_ps1_uses_the_python_pattern(self):
        text = (KIT / "setup.ps1").read_text(encoding="utf-8")
        self.assertIn("-replace '[^A-Za-z0-9_]', '_'", text)


if __name__ == "__main__":
    unittest.main()
