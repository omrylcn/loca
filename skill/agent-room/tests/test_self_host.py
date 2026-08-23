import stat
import subprocess
import tempfile
import unittest
from pathlib import Path


REPO_DIR = Path(__file__).resolve().parents[3]
INIT = REPO_DIR / "scripts" / "init-self-host.sh"


class SelfHostInitTests(unittest.TestCase):
    def test_initializer_writes_private_random_environment_and_refuses_overwrite(self):
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / ".env"
            first = subprocess.run(
                [
                    str(INIT),
                    "--server-url",
                    "https://loca.example.com",
                    "--output",
                    str(output),
                ],
                text=True,
                capture_output=True,
                timeout=5,
                check=False,
            )
            self.assertEqual(first.returncode, 0, first.stderr)
            values = dict(
                line.split("=", 1)
                for line in output.read_text(encoding="utf-8").splitlines()
            )
            self.assertRegex(values["ADMIN_TOKEN"], r"^adm_[0-9a-f]{64}$")
            self.assertEqual(
                values["PUBLIC_SERVER_URL"], "https://loca.example.com"
            )
            self.assertEqual(stat.S_IMODE(output.stat().st_mode), 0o600)

            second = subprocess.run(
                [
                    str(INIT),
                    "--server-url",
                    "https://loca.example.com",
                    "--output",
                    str(output),
                ],
                text=True,
                capture_output=True,
                timeout=5,
                check=False,
            )
            self.assertEqual(second.returncode, 2)
            self.assertIn("refusing to overwrite", second.stderr)

    def test_initializer_rejects_non_https_or_path_origins(self):
        for server in (
            "http://loca.example.com",
            "https://loca.example.com/path",
            "https://user:pass@loca.example.com",
        ):
            with self.subTest(server=server), tempfile.TemporaryDirectory() as tmp:
                output = Path(tmp) / ".env"
                result = subprocess.run(
                    [
                        str(INIT),
                        "--server-url",
                        server,
                        "--output",
                        str(output),
                    ],
                    text=True,
                    capture_output=True,
                    timeout=5,
                    check=False,
                )
                self.assertEqual(result.returncode, 2)
                self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
