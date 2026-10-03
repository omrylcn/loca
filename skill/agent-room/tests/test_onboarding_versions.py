import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("check_doc_links", Path(__file__).resolve().parents[3] / "scripts/check-doc-links.py")
checker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(checker)


class OnboardingVersionTests(unittest.TestCase):
    def test_stale_install_commands_fail_but_historical_prose_is_not_a_pin(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "docs").mkdir()
            for doc in checker.ONBOARDING_DOCS:
                (root / doc).write_text("History: 0.7.0 was an earlier release.\n")
            self.assertEqual(checker.check_onboarding_versions(root, "0.9.29"), [])
            (root / "docs/self-host.md").write_text("git checkout v0.7.0\nLOCA_VERSION=0.7.0\n")
            self.assertEqual(len(checker.check_onboarding_versions(root, "0.9.29")), 2)
            (root / "docs/self-host.md").write_text("git checkout v0.9.29\nLOCA_VERSION=0.9.29\n")
            self.assertEqual(checker.check_onboarding_versions(root, "0.9.29"), [])
