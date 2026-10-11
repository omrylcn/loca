"""Native drafting capability boundaries; test inference remains opt-in."""
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
import wiki_worker as worker
from wiki_runtime import ProcessWorker


class WikiWorkerTests(unittest.TestCase):
    def test_loca_credentials_and_api_provider_secrets_are_not_inherited(self):
        with patch.dict(os.environ,{"LOCA_ENV":"private","LOCA_SESSION":"secret",
                "DAVET_general":"secret","OPENAI_API_KEY":"secret","ANTHROPIC_API_KEY":"secret",
                "CLAUDECODE":"parent","BASH_ENV":"hook","PYTHONPATH":"hook"}):
            env = worker.safe_parent_environment()
        for key in ("LOCA_ENV","LOCA_SESSION","DAVET_general","OPENAI_API_KEY",
                    "ANTHROPIC_API_KEY","CLAUDECODE","BASH_ENV","PYTHONPATH"):
            self.assertNotIn(key,env)

    def test_private_home_copies_only_login_not_project_hooks(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source,home = root/"original",root/"worker"
            source.mkdir(); home.mkdir()
            (source/"auth.json").write_text('{"fixture":"native-login"}')
            (source/"config.toml").write_text("[hooks]\ncommand='forbidden'\n")
            with patch.dict(os.environ,{"CODEX_HOME":str(source),"CLAUDE_CONFIG_DIR":"foreign"}):
                env = worker.private_environment("codex",home)
            target = home/".codex/auth.json"
            self.assertEqual(target.stat().st_mode & 0o777,0o600)
            self.assertEqual(env["CODEX_HOME"],str(home/".codex"))
            self.assertNotIn("CLAUDE_CONFIG_DIR",env)
            self.assertFalse((home/".codex/config.toml").exists())

    def test_schema_fences_source_boundary_and_revision(self):
        schema = worker.schema_for({"wiki":{"revision":7},"reviewed_through":19})
        self.assertEqual(schema["properties"]["expected_revision"]["const"],7)
        self.assertEqual(schema["properties"]["reviewed_through"]["const"],19)
        self.assertEqual(schema["properties"]["pages"]["maxItems"],32)

    def test_preflight_rejects_missing_binary_login_or_generic_runtime(self):
        with self.assertRaises(ValueError): worker.preflight("generic")
        with patch.object(worker.shutil,"which",return_value=None):
            with self.assertRaisesRegex(RuntimeError,"executable"): worker.preflight("codex")
        with tempfile.TemporaryDirectory() as directory:
            with patch.dict(os.environ,{"CODEX_HOME":directory}):
                with patch.object(worker.shutil,"which",return_value="/bin/native"):
                    with self.assertRaisesRegex(RuntimeError,"login"): worker.preflight("codex")

    def test_custom_provider_is_not_silently_replaced(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory)
            (source/"auth.json").write_text("{}")
            (source/"config.toml").write_text("model_provider='custom'\n")
            with patch.dict(os.environ,{"CODEX_HOME":directory}):
                with patch.object(worker.shutil,"which",return_value="/bin/native"):
                    with self.assertRaisesRegex(RuntimeError,"Custom"): worker.preflight("codex")

    def test_process_worker_has_no_credential_env_and_cleans_owned_group(self):
        class Child:
            returncode = 0
            def communicate(self, data):
                self.input = json.loads(data)
                return ('{"expected_revision":0}',None)
        child = Child()
        with patch("wiki_worker.preflight"),patch("wiki_runtime.subprocess.Popen",return_value=child) as spawn:
            with patch("wiki_worker.stop_process") as stop:
                with patch.dict(os.environ,{"LOCA_SESSION":"secret","LOCA_ENV":"private"}):
                    result = ProcessWorker("codex")({"room":"a"},lambda:None)
                self.assertEqual(result,{"expected_revision":0})
                self.assertNotIn("LOCA_SESSION",spawn.call_args.kwargs["env"])
                self.assertNotIn("LOCA_ENV",spawn.call_args.kwargs["env"])
                self.assertTrue(spawn.call_args.kwargs["start_new_session"])
                stop.assert_called_once_with(child,group=True)


if __name__ == "__main__": unittest.main()
