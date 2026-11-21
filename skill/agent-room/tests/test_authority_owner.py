"""An agent's user (references/authority.md): `connect.sh owner` shows, sets
and clears LOCA_OWNER in that identity's own file, and nothing else's."""
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

SKILL_DIR = Path(__file__).resolve().parents[1]
SERVER = "http://127.0.0.1:9"


class OwnerTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.home = Path(self.tmp.name)
        (self.home / ".loca").mkdir(mode=0o700)
        self.env_file = self.home / ".loca" / "bot.env"
        self.env_file.write_text(
            f"ROOM_SERVER_URL={SERVER}\nLOCA_NAME=bot\nLOCA_MEMBERSHIP=mb_bot\n",
            encoding="utf-8",
        )
        self.env_file.chmod(0o600)

    def tearDown(self):
        self.tmp.cleanup()

    def owner(self, *args, extra_env=None):
        env = {k: v for k, v in os.environ.items() if not k.startswith(("LOCA_", "DAVET_"))}
        env.pop("ROOM_SERVER_URL", None)
        env["HOME"] = str(self.home)
        env.update(extra_env or {})
        return subprocess.run(
            [str(SKILL_DIR / "connect.sh"), "owner", SERVER, *args],
            env=env, capture_output=True, text=True, timeout=30,
        )

    def stored(self):
        return [line for line in self.env_file.read_text().splitlines() if line.startswith("LOCA_OWNER=")]

    def test_shows_sets_and_clears_the_user(self):
        shown = self.owner("bot")
        self.assertEqual(shown.returncode, 0, shown.stderr)
        self.assertIn("OWNER: not set", shown.stdout)

        set_ = self.owner("bot", "@omrylcn")
        self.assertEqual(set_.returncode, 0, set_.stderr)
        self.assertIn("OWNER: @omrylcn", set_.stdout)
        self.assertEqual(self.stored(), ["LOCA_OWNER=omrylcn"])
        self.assertIn("OWNER: @omrylcn", self.owner("bot").stdout)
        # The identity's credentials are untouched.
        self.assertIn("LOCA_MEMBERSHIP=mb_bot", self.env_file.read_text())

        cleared = self.owner("bot", "--clear")
        self.assertEqual(cleared.returncode, 0, cleared.stderr)
        self.assertEqual(self.stored(), [])
        self.assertIn("OWNER: not set", self.owner("bot").stdout)

    def test_refuses_what_is_not_a_users_name(self):
        for bad in ["two words", "_ops", "a@b", "x" * 65]:
            result = self.owner("bot", bad)
            self.assertEqual(result.returncode, 2, bad)
        self.assertEqual(self.owner("bot", "bot").returncode, 2, "an agent is not its own user")
        self.assertEqual(self.stored(), [])

    def test_an_identity_with_no_file_has_no_user_to_set(self):
        result = self.owner("someone-else", "omrylcn")
        self.assertEqual(result.returncode, 3, result.stdout)

    def test_a_user_set_in_the_shell_is_not_this_identitys(self):
        # Another agent's LOCA_OWNER in the parent shell does not become
        # this identity's user: the identity file is the only source.
        result = self.owner("bot", extra_env={"LOCA_OWNER": "intruder"})
        self.assertIn("OWNER: not set", result.stdout)


if __name__ == "__main__":
    unittest.main()
