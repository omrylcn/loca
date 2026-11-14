import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]


class WebAuthContractTests(unittest.TestCase):
    def test_auth_unknown_is_not_rendered_as_logged_out(self):
        state = (ROOT / "web/assets/state.js").read_text(encoding="utf-8")
        app = (ROOT / "web/assets/app.js").read_text(encoding="utf-8")
        self.assertIn('authStatus: "unknown"', state)
        self.assertIn('if (state.authStatus === "unknown") return;', state)
        self.assertIn('who.status === 401', state)
        # A 5xx from /whoami leaves the status unknown. The attempt is still
        # this page's own (`true`); it just has no verdict.
        self.assertIn('if (!who.ok) return true;', state)
        self.assertIn("showAuthUnavailable()", app)
        self.assertIn("setTimeout(bootstrapIdentity, 2000)", app)

    def test_room_navigation_never_writes_a_fake_login_history_entry(self):
        sources = "\n".join(
            path.read_text(encoding="utf-8")
            for path in (ROOT / "web/assets").glob("*.js")
        )
        self.assertNotIn("history.pushState", sources)
        self.assertNotIn("location.hash", sources)

    def test_people_have_no_runtime_wake_verdict(self):
        people = (ROOT / "web/assets/people.js").read_text(encoding="utf-8")
        self.assertIn('p?.kind !== "agent"', people)
        self.assertIn('>n/a</span>', people)

    def test_browser_tests_and_web_assets_name_no_local_machine_path(self):
        # These files are published. A path under somebody's home or an
        # agent's session directory names a person and a machine, and a test
        # that writes there only works on that machine. Screenshots belong in
        # the test's own output directory (test.info().outputPath). The
        # placeholder home "/home/user/" is nobody's and stays allowed.
        import re
        local = re.compile(r"/tmp/claude-|/home/(?!user/)[A-Za-z0-9_.-]+/|/Users/[A-Za-z0-9_.-]+/")
        offenders = []
        for folder, pattern in (("tests/browser", "*.js"), ("web/assets", "*.js"), ("web", "*.html")):
            for path in sorted((ROOT / folder).rglob(pattern)):
                for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
                    if local.search(line):
                        offenders.append(f"{path.relative_to(ROOT)}:{number}")
        self.assertEqual(offenders, [])


if __name__ == "__main__":
    unittest.main()
