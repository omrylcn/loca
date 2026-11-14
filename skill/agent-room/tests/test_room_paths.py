"""A loca name may hold spaces and any letter; every URL path must encode it.

An agent seated in "DOB İLE ALAKALI İŞ :D" reconnected every two seconds
because the backfill URL put the raw name in the path and urllib refused it.
"""
import pathlib
import re
import subprocess
import unittest

KIT = pathlib.Path(__file__).resolve().parents[1]


class RoomPathTests(unittest.TestCase):
    def test_no_python_url_puts_a_raw_room_in_a_path(self):
        for path in KIT.glob("*.py"):
            for n, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
                if re.search(r"/rooms/\{room\}", line):
                    self.fail(f"{path.name}:{n} puts a raw loca name in a URL: {line.strip()}")

    def test_no_shell_url_puts_a_raw_room_in_a_path(self):
        text = (KIT / "connect.sh").read_text(encoding="utf-8")
        for n, line in enumerate(text.splitlines(), 1):
            if "/rooms/$room" in line or "/rooms/${room}" in line:
                self.fail(f"connect.sh:{n} puts a raw loca name in a URL: {line.strip()}")

    def test_shell_listener_encodes_query_components(self):
        text = (KIT / "connect.sh").read_text(encoding="utf-8")
        self.assertNotIn("?room=$room&name=$name", text)
        self.assertIn('room=$(_room_path "$room")&name=$(_room_path "$name")', text)

    def test_room_path_encodes_spaces_and_letters(self):
        script = (
            'eval "$(sed -n "/^_room_path() {/,/^}/p" "$1")"; _room_path "DOB İLE ALAKALI İŞ :D"'
        )
        out = subprocess.run(
            ["bash", "-c", script, "_", str(KIT / "connect.sh")],
            capture_output=True, text=True, check=True,
        ).stdout.strip()
        self.assertEqual(out, "DOB%20%C4%B0LE%20ALAKALI%20%C4%B0%C5%9E%20%3AD")


if __name__ == "__main__":
    unittest.main()
