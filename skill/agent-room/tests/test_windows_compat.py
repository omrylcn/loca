import importlib.util
import sys
import unittest
from pathlib import Path
from unittest import mock


SKILL_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SKILL_DIR))
SPEC = importlib.util.spec_from_file_location("loca_listen", SKILL_DIR / "listen.py")
listen = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(listen)


class FakeStream:
    def __init__(self):
        self.calls = []

    def reconfigure(self, **kwargs):
        self.calls.append(kwargs)


class WindowsCompatibilityTests(unittest.TestCase):
    def test_listener_forces_unicode_safe_protocol_streams(self):
        stdout = FakeStream()
        stderr = FakeStream()
        with mock.patch.object(listen.sys, "stdout", stdout), mock.patch.object(
            listen.sys, "stderr", stderr
        ):
            listen.configure_protocol_streams()
        expected = [{"encoding": "utf-8", "errors": "backslashreplace"}]
        self.assertEqual(stdout.calls, expected)
        self.assertEqual(stderr.calls, expected)

    def test_shell_normalizes_jq_crlf_and_reports_missing_process_tools(self):
        script = (SKILL_DIR / "connect.sh").read_text(encoding="utf-8")
        self.assertEqual(script.count("jq -r '.locas[]?'") , 2)
        self.assertGreaterEqual(script.count("| tr -d '\\r'"), 2)
        self.assertIn("process inspection unavailable (pgrep/ps missing)", script)
        self.assertIn("listener coverage could not be inspected", script)

    def test_runtime_docs_use_platform_neutral_stdout_sink(self):
        docs = (SKILL_DIR / "references" / "runtimes.md").read_text(encoding="utf-8")
        self.assertIn("+ '- --skip-own NAME '", docs)
        self.assertIn("MSYS may rewrite", docs)


if __name__ == "__main__":
    unittest.main()
