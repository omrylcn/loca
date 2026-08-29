"""The skill distribution bundles are the single source of truth the server
endpoint and the Desktop both ship. These tests lock the properties that make
that safe: deterministic (content-addressable) archives, a manifest that
faithfully describes the archive, only git-tracked runtime files (no untracked,
symlinked, or credential-container files), the onboarding files present, and a
CENTRAL credential policy that refuses every real token class."""

import concurrent.futures
import hashlib
import importlib.util
import json
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

SKILL_DIR = Path(__file__).resolve().parents[1]
ROOT = SKILL_DIR.parents[1]
SCRIPT = ROOT / "scripts" / "build-skill-bundles.sh"


def _load_policy():
    spec = importlib.util.spec_from_file_location(
        "credential_scan", ROOT / "scripts" / "credential_scan.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


@unittest.skipUnless(shutil.which("zip") and SCRIPT.exists(),
                     "needs zip and the packaging script")
class SkillBundlesTest(unittest.TestCase):
    def _build(self, outdir=None):
        args = ["bash", str(SCRIPT)] + ([outdir] if outdir else [])
        subprocess.run(args, cwd=str(ROOT), check=True, capture_output=True)
        return Path(outdir) if outdir else ROOT / "dist" / "skill-bundles"

    def test_bundles_are_deterministic_and_manifest_consistent(self):
        out = self._build()
        first = {}
        for name in ("loca", "loca-care"):
            manifest = json.loads((out / f"{name}.manifest.json").read_text())
            zip_bytes = (out / f"{name}.zip").read_bytes()
            self.assertEqual(hashlib.sha256(zip_bytes).hexdigest(),
                             manifest["bundle_sha256"],
                             f"{name}: zip bytes must match the manifest SHA-256")
            first[name] = manifest["bundle_sha256"]
            for f in manifest["files"]:
                p = f["path"]
                self.assertFalse(
                    any(x in p for x in (".env", ".request", "/tests/", "__pycache__")),
                    f"no secret/test file may ship: {p}")
        out = self._build()
        for name in ("loca", "loca-care"):
            manifest = json.loads((out / f"{name}.manifest.json").read_text())
            self.assertEqual(manifest["bundle_sha256"], first[name],
                             f"{name} archive must be deterministic")

    def test_loca_bundle_ships_the_onboarding_files(self):
        out = self._build()
        paths = [f["path"] for f in
                 json.loads((out / "loca.manifest.json").read_text())["files"]]
        for expected in ("loca/connect.sh", "loca/join_request.py",
                         "loca/stop_listener.py", "loca/SKILL.md",
                         "loca/credentials.py", "loca/listen.py"):
            self.assertIn(expected, paths, f"loca bundle must ship {expected}")

    def test_two_parallel_builds_into_separate_dirs_do_not_clobber(self):
        def build(outdir):
            subprocess.run(["bash", str(SCRIPT), outdir], cwd=str(ROOT),
                           check=True, capture_output=True)
            return json.loads(
                (Path(outdir) / "loca.manifest.json").read_text())["bundle_sha256"]

        with tempfile.TemporaryDirectory() as a, tempfile.TemporaryDirectory() as b:
            with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
                fa, fb = pool.submit(build, a), pool.submit(build, b)
                sha_a, sha_b = fa.result(timeout=120), fb.result(timeout=120)
            self.assertEqual(sha_a, sha_b, "parallel builds must be byte-identical")
            for d in (a, b):
                self.assertTrue((Path(d) / "loca.zip").exists())
                self.assertTrue((Path(d) / "loca-care.zip").exists())

    # --- central credential policy -----------------------------------------

    def test_central_policy_catches_every_class_but_not_code(self):
        policy = _load_policy()
        # Every real token class the server mints is caught.
        for prefix in policy.TOKEN_PREFIXES:
            self.assertIsNotNone(policy.scan_text(prefix + "a" * 40),
                                 f"{prefix} token must be caught")
        # A legacy bound-credential assignment (e.g. ROOM_TOKEN) is caught.
        self.assertIsNotNone(policy.scan_text("ROOM_TOKEN=" + "x" * 32))
        self.assertIsNotNone(policy.scan_text("LOCA_MEMBERSHIP=" + "y" * 40))
        # The skill's own source that merely NAMES the format is NOT a hit.
        self.assertIsNone(policy.scan_text('line.startswith("LOCA_MEMBERSHIP=")'))
        self.assertIsNone(policy.scan_text("#   LOCA_MEMBERSHIP=mb_..."))
        self.assertIsNone(policy.scan_text('secure_token("adm_", 24)'))
        self.assertIsNone(policy.scan_text("mb_[A-Za-z0-9]{16}"))

    def _build_in_fixture_repo(self, extra):
        """Set up a minimal repo, drop `extra` files into skill/agent-room, commit,
        and run the REAL packaging script. `extra` maps a relative path to either a
        string (file content) or ("symlink", target). Returns the CompletedProcess."""
        repo = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, repo, ignore_errors=True)
        r = Path(repo)
        (r / "scripts").mkdir()
        for s in ("build-skill-bundles.sh", "skill_manifest.py", "credential_scan.py"):
            shutil.copy(ROOT / "scripts" / s, r / "scripts" / s)
        (r / "scripts" / "version.sh").write_text("#!/usr/bin/env bash\necho 0.0.0\n")
        os.chmod(r / "scripts" / "version.sh", 0o755)
        for skill in ("agent-room", "loca-care"):
            (r / "skill" / skill).mkdir(parents=True)
            (r / "skill" / skill / "SKILL.md").write_text("# skill\n")
            (r / "skill" / skill / "VERSION").write_text("0.0.0\n")
        for rel, content in extra.items():
            p = r / "skill" / "agent-room" / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            if isinstance(content, tuple) and content[0] == "symlink":
                os.symlink(content[1], p)
            else:
                p.write_text(content)
        subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
        subprocess.run(["git", "add", "-A"], cwd=repo, check=True)
        subprocess.run(["git", "-c", "user.email=t@t", "-c", "user.name=t",
                        "commit", "-qm", "fixture"], cwd=repo, check=True)
        return subprocess.run(["bash", str(r / "scripts" / "build-skill-bundles.sh")],
                              cwd=repo, capture_output=True, text=True)

    def test_packaging_refuses_every_credential_class(self):
        policy = _load_policy()
        v = "z" * 32
        # A prefixed token can leak anywhere (mid-line in code)...
        cases = {p: ("leak.py", "S = '%s'\n" % (p + "a" * 40))
                 for p in policy.TOKEN_PREFIXES}
        # ...and a bound-credential assignment leaks in every common form:
        # shell/env (optional export, quoted), JSON, and YAML.
        cases["bare ="] = ("a.sh", "ROOM_TOKEN=%s\n" % v)
        cases["export ="] = ("b.sh", "export ROOM_TOKEN=%s\n" % v)
        cases["export quoted"] = ("c.sh", 'export LOCA_MEMBERSHIP="%s"\n' % v)
        cases["json"] = ("d.json", '{"ROOM_TOKEN": "%s"}\n' % v)
        cases["yaml"] = ("e.yaml", "LOCA_SESSION: %s\n" % v)
        cases["davet export"] = ("f.sh", "export DAVET_iye=%s\n" % v)
        for label, (fname, content) in cases.items():
            res = self._build_in_fixture_repo({fname: content})
            self.assertEqual(res.returncode, 3,
                             f"{label} must be refused\n{res.stdout}{res.stderr}")

    def test_packaging_refuses_a_tracked_symlink(self):
        res = self._build_in_fixture_repo({"link.py": ("symlink", "/etc/passwd")})
        self.assertEqual(res.returncode, 4, res.stdout + res.stderr)
        self.assertIn("symlink", res.stderr.lower())

    def test_packaging_refuses_a_credential_container_file_by_name(self):
        for name in ("secret.env", "agent.request"):
            res = self._build_in_fixture_repo({name: "irrelevant\n"})
            self.assertEqual(res.returncode, 5,
                             f"{name} must be refused\n{res.stdout}{res.stderr}")


class InstallCommandTest(unittest.TestCase):
    """Actually RUN the Getting Started install commands (the FS-copy shape the
    guide emits) twice in a temp HOME, and prove they are idempotent, never nest
    `loca/loca`, and land in the right runtime directory — for both Claude Code
    and Codex, for the agent (loca) and caretaker (loca + loca-care) forms.

    The command shape mirrors web/assets/getstarted.js installCmd(); the browser
    spec locks that getstarted.js produces exactly this shape."""

    def _fake_library(self):
        lib = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, lib, ignore_errors=True)
        for skill in ("loca", "loca-care"):
            os.makedirs(os.path.join(lib, skill))
            with open(os.path.join(lib, skill, "SKILL.md"), "w") as f:
                f.write("# %s\n" % skill)
        return lib

    @staticmethod
    def _fs_install_cmd(lib, dest, skills):
        parts = ['rm -rf %s/%s && cp -R "%s/%s" %s/%s' % (dest, s, lib, s, dest, s)
                 for s in skills]
        return "mkdir -p %s && %s" % (dest, " && ".join(parts))

    def _run_twice(self, cmd, home):
        for _ in range(2):  # fresh install, then update — both must succeed
            r = subprocess.run(["bash", "-c", cmd], env=dict(os.environ, HOME=home),
                               capture_output=True, text=True)
            self.assertEqual(r.returncode, 0, r.stdout + r.stderr)

    def test_agent_and_caretaker_commands_are_idempotent_no_nesting_both_runtimes(self):
        lib = self._fake_library()
        cases = [
            ("~/.claude/skills", ".claude/skills", ["loca"]),
            ("~/.codex/skills", ".codex/skills", ["loca"]),
            ("~/.claude/skills", ".claude/skills", ["loca", "loca-care"]),
            ("~/.codex/skills", ".codex/skills", ["loca", "loca-care"]),
        ]
        for dest, rel, skills in cases:
            with tempfile.TemporaryDirectory() as home:
                self._run_twice(self._fs_install_cmd(lib, dest, skills), home)
                base = os.path.join(home, rel)
                for s in skills:
                    self.assertTrue(os.path.exists(os.path.join(base, s, "SKILL.md")),
                                    "%s must be installed under %s" % (s, rel))
                    self.assertFalse(os.path.exists(os.path.join(base, s, s)),
                                     "must never nest %s/%s" % (s, s))


if __name__ == "__main__":
    unittest.main()
