"""scripts/export-public-tree.py on a fixture repository."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[3]
SCRIPT = REPO / "scripts" / "export-public-tree.py"

spec = importlib.util.spec_from_file_location("export_public_tree", SCRIPT)
export_tree = importlib.util.module_from_spec(spec)
spec.loader.exec_module(export_tree)

GIT_ENV = {
    "GIT_AUTHOR_NAME": "Fixture Author", "GIT_AUTHOR_EMAIL": "fixture@example.invalid",
    "GIT_COMMITTER_NAME": "Fixture Author", "GIT_COMMITTER_EMAIL": "fixture@example.invalid",
    "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1",
}


class ExportPublicTreeTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.repo = self.root / "private"
        self.repo.mkdir()
        self.git("init", "-q", "-b", "main")
        self.write("README.md", "public readme\n")
        self.write("scripts/tool.sh", "#!/bin/sh\necho tool\n", executable=True)
        self.write("docs/guide.md", "public guide\n")
        self.write("docs/review-2026-01-02.md", "a dated internal review\n")
        self.write("packaging/public-source-paths.txt", "README.md\nscripts\ndocs/*.md\npackaging/public-source-paths.txt\n")
        self.write("packaging/private-source-paths.txt", "docs/*-20??-??-??.md\npackaging/private-source-paths.txt\n")
        self.commit("fixture")

    def git(self, *args):
        env = dict(os.environ, **GIT_ENV)
        return subprocess.run(["git", "-C", str(self.repo), *args], env=env, check=True,
                              capture_output=True, text=True).stdout

    def write(self, path, text, executable=False):
        target = self.repo / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text)
        if executable:
            target.chmod(0o755)

    def commit(self, message):
        self.git("add", "-A")
        self.git("commit", "-q", "-m", message)
        return self.git("rev-parse", "HEAD").strip()

    def run_export(self, *args):
        return subprocess.run([sys.executable, str(SCRIPT), "--repo", str(self.repo), *args],
                              capture_output=True, text=True, timeout=60, check=False)

    def test_unclassified_file_fails_and_is_named_without_writing(self):
        self.write("NOTES.md", "is this public?\n")
        self.commit("add an unclassified file")
        output = self.root / "out"

        result = self.run_export("--output", str(output))

        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("1 tracked path(s) are in neither", result.stderr)
        self.assertIn("  NOTES.md", result.stderr)
        self.assertFalse(output.exists())
        check = self.run_export("--check")
        self.assertEqual(check.returncode, 1)
        self.assertIn("NOTES.md", check.stderr)

    def test_export_copies_only_allowed_committed_files_with_a_hash_manifest(self):
        commit = self.git("rev-parse", "HEAD").strip()
        # An uncommitted change is not what was approved: the commit is exported.
        self.write("README.md", "uncommitted edit\n")
        output = self.root / "out"

        result = self.run_export("--commit", commit, "--output", str(output))

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        tree = output / "tree"
        exported = sorted(str(p.relative_to(tree)) for p in tree.rglob("*") if p.is_file())
        self.assertEqual(exported, ["README.md", "docs/guide.md", "packaging/public-source-paths.txt",
                                    "scripts/tool.sh"])
        self.assertEqual((tree / "README.md").read_text(), "public readme\n")
        self.assertTrue(stat.S_IMODE((tree / "scripts/tool.sh").stat().st_mode) & 0o100)
        self.assertFalse(stat.S_IMODE((tree / "README.md").stat().st_mode) & 0o111)
        manifest = json.loads((output / "export-manifest.json").read_text())
        self.assertEqual(manifest["source_commit"], commit)
        self.assertEqual(sorted(manifest["private"]), ["docs/review-2026-01-02.md",
                                                       "packaging/private-source-paths.txt"])
        for entry in manifest["files"]:
            content = (tree / entry["path"]).read_bytes()
            self.assertEqual(entry["sha256"], hashlib.sha256(content).hexdigest())
            self.assertEqual(entry["size"], len(content))
            blob = self.git("rev-parse", f"{commit}:{entry['path']}").strip()
            self.assertEqual(entry["git_blob"], blob)
        self.assertIn("left out 2 private path(s)", result.stdout)

    def test_head_moving_during_the_export_never_mixes_two_commits(self):
        # `--commit HEAD` (the default) names a moving thing. Between two of
        # the export's reads someone commits: `docs/guide.md` gains private
        # text and is moved to the private list. Lists of the first commit
        # with files of the second would publish that text.
        first = self.git("rev-parse", "HEAD").strip()
        real_git = export_tree.git
        moved = []

        def git_then_move_head(repo, *args, data=None):
            answer = real_git(repo, *args, data=data)
            if not moved and args[:1] == ("show",):
                moved.append(True)
                self.write("docs/guide.md", "public guide\nnow with a private paragraph\n")
                self.write("packaging/public-source-paths.txt",
                           "README.md\nscripts\npackaging/public-source-paths.txt\n")
                self.write("packaging/private-source-paths.txt",
                           "docs/*.md\npackaging/private-source-paths.txt\n")
                self.commit("the guide becomes private")
            return answer

        export_tree.git = git_then_move_head
        self.addCleanup(setattr, export_tree, "git", real_git)
        out = self.root / "out"
        code = export_tree.main(["--repo", str(self.repo), "--output", str(out)])

        self.assertTrue(moved, "the fixture did move HEAD during the export")
        self.assertEqual(code, 0)
        manifest = json.loads((out / "export-manifest.json").read_text())
        self.assertEqual(manifest["source_commit"], first)
        # Everything is the first commit's: its lists and its content.
        self.assertEqual((out / "tree" / "docs" / "guide.md").read_text(), "public guide\n")
        self.assertNotIn("private paragraph", "".join(
            path.read_text() for path in (out / "tree").rglob("*") if path.is_file()))

    def test_output_that_is_not_empty_is_refused(self):
        output = self.root / "out"
        output.mkdir()
        (output / "left-over").write_text("x")
        result = self.run_export("--output", str(output))
        self.assertEqual(result.returncode, 1)
        self.assertIn("must not exist or be an empty directory", result.stderr)
        self.assertEqual([p.name for p in output.iterdir()], ["left-over"])

    def test_deny_wins_over_allow_and_globs_stay_within_a_segment(self):
        allow = export_tree.parse_list("docs/*.md\nsrc\nweb/**/*.js\n", "allow")
        deny = export_tree.parse_list("docs/*-20??-??-??.md\n", "deny")
        files = [(path, "100644", "blob", "0" * 40) for path in (
            "docs/guide.md", "docs/review-2026-01-02.md", "docs/sub/deep.md",
            "src/main.rs", "srcx/other.rs", "web/a.js", "web/x/y/b.js")]
        public, private, unclassified, unused = export_tree.classify(files, allow, deny)
        self.assertEqual([r[0] for r in public], ["docs/guide.md", "src/main.rs", "web/a.js", "web/x/y/b.js"])
        self.assertEqual(private, ["docs/review-2026-01-02.md"])
        self.assertEqual(unclassified, ["docs/sub/deep.md", "srcx/other.rs"])
        self.assertEqual(unused, [])
        with self.assertRaises(export_tree.ExportError):
            export_tree.parse_list("../outside\n", "allow")


if __name__ == "__main__":
    unittest.main()
