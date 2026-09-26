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
        self.assertIn('if (!who.ok) return;', state)
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


if __name__ == "__main__":
    unittest.main()
