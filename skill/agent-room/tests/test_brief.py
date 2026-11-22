"""`connect.sh brief`: a loca's state in a few lines, each one traceable to a
task number, a wait or a journal entry; a section whose extension is off
says so instead of failing."""
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SKILL_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SKILL_DIR))

import brief  # noqa: E402

NOW = 1_800_000_000_000
HOUR = 3_600_000


class BriefTests(unittest.TestCase):
    def test_a_full_loca_in_sections_with_their_sources(self):
        tasks = [
            {"id": 3, "room": "proj", "title": "fix login", "status": "review", "assigned_to": "bob",
             "assigned_by": "ada", "created_by": "ada", "review_note": "see the test"},
            {"id": 4, "room": "proj", "title": "docs", "status": "taken", "assigned_to": "cem",
             "progress_at": NOW - 2 * HOUR, "review_reason": "missing example"},
            {"id": 5, "room": "proj", "title": "release", "status": "open"},
            {"id": 6, "room": "proj", "title": "old", "status": "done", "closed_at": NOW - 3 * 24 * HOUR},
            {"id": 7, "room": "proj", "title": "today", "status": "done", "assigned_to": "bob",
             "closed_at": NOW - HOUR},
        ]
        waits = [{"room": "proj", "waiter": "cem", "waiting_for": "bob", "reason": "the API",
                  "since": NOW - 30 * 60_000}]
        goals = [{"status": "active", "outcome": "login works", "checkpoint": "e2e green"}]
        journal = [{"by": "bob", "text": "deployed 1.2", "at": NOW - HOUR},
                   {"by": "ada", "text": "last week", "at": NOW - 9 * 24 * HOUR}]
        text = "\n".join(brief.brief("proj", tasks, waits, goals, journal, now=NOW))
        self.assertIn("Goal: login works (next checkpoint: e2e green)", text)
        self.assertIn("#3 fix login - bob asks ada to review - note: see the test", text)
        self.assertIn("#4 docs - cem, moved 2 h ago - sent back: missing example", text)
        self.assertIn("#5 release - nobody yet", text)
        self.assertIn("Done in the last 24 h (1):", text)
        self.assertIn("#7 today", text)
        self.assertNotIn("#6 old", text)
        self.assertIn("cem waits for bob since 30 min ago: the API", text)
        self.assertIn("bob: deployed 1.2 (1 h ago)", text)
        self.assertNotIn("last week", text)

    def test_extensions_that_are_off_are_named_not_failed(self):
        text = "\n".join(brief.brief("lounge", None, [], None, None, now=NOW))
        self.assertIn("Goal: Focus is not on here", text)
        self.assertIn("Tasks: the Loca Board is not on here", text)
        self.assertIn("Landed: the Journal is not on here", text)
        self.assertIn("Blocked (0):", text)

    def test_the_command_reads_files_and_an_empty_file_counts_as_unreadable(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "tasks.json").write_text(json.dumps([{"id": 1, "title": "t", "status": "open"}]))
            (root / "waits.json").write_text("[]")
            (root / "goals.json").write_text("")
            (root / "journal.json").write_text("")
            result = subprocess.run(
                [sys.executable, str(SKILL_DIR / "brief.py"), "proj",
                 *(str(root / f"{part}.json") for part in ("tasks", "waits", "goals", "journal"))],
                capture_output=True, text=True, timeout=10,
            )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("#1 t - nobody yet", result.stdout)
        self.assertIn("Goal: Focus is not on here", result.stdout)


if __name__ == "__main__":
    unittest.main()
