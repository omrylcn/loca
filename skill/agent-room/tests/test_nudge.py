import importlib.util
import json
import os
import stat
import subprocess
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock


SKILL_DIR = Path(__file__).resolve().parents[1]
NUDGE = SKILL_DIR / "nudge.py"
SPEC = importlib.util.spec_from_file_location("loca_nudge", NUDGE)
assert SPEC and SPEC.loader
NUDGE_MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(NUDGE_MODULE)


class NudgeTests(unittest.TestCase):
    def test_turn_timeout_is_renewed_by_progress_events(self):
        client = NUDGE_MODULE.AppServer.__new__(NUDGE_MODULE.AppServer)
        client.pending = []
        client.proc = mock.Mock()
        client.proc.poll.return_value = None
        messages = [
            {"method": "item/started", "params": {"threadId": "thread-test"}},
            {"method": "item/completed", "params": {"threadId": "thread-test"}},
            {
                "method": "turn/completed",
                "params": {
                    "threadId": "thread-test",
                    "turn": {"id": "turn-test", "status": "completed"},
                },
            },
        ]

        def delayed_message(_deadline):
            time.sleep(0.06)
            return messages.pop(0)

        client.read_message = delayed_message
        completed = client.wait_for_turn("thread-test", "turn-test", 0.1)
        self.assertEqual(completed["status"], "completed")

    def test_empty_app_server_error_is_not_treated_as_success(self):
        self.assertEqual(
            NUDGE_MODULE.response_error({"id": 1, "error": {}}),
            "unknown app-server error",
        )

    def test_codex_binary_resolves_from_bun_when_path_is_minimal(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fake_codex = root / ".bun/bin/codex"
            fake_codex.parent.mkdir(parents=True)
            fake_codex.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            fake_codex.chmod(fake_codex.stat().st_mode | stat.S_IXUSR)
            previous_home = os.environ.get("HOME")
            previous_path = os.environ.get("PATH")
            try:
                os.environ["HOME"] = str(root)
                os.environ["PATH"] = "/usr/bin:/bin"
                resolved = NUDGE_MODULE.resolve_codex_bin("codex")
            finally:
                if previous_home is None:
                    os.environ.pop("HOME", None)
                else:
                    os.environ["HOME"] = previous_home
                if previous_path is None:
                    os.environ.pop("PATH", None)
                else:
                    os.environ["PATH"] = previous_path
            self.assertEqual(resolved, str(fake_codex.resolve()))

    def test_codex_binary_reports_actionable_error_when_missing(self):
        with tempfile.TemporaryDirectory() as tmp:
            previous_home = os.environ.get("HOME")
            previous_path = os.environ.get("PATH")
            try:
                os.environ["HOME"] = tmp
                os.environ["PATH"] = "/nonexistent"
                with self.assertRaisesRegex(FileNotFoundError, "set CODEX_BIN"):
                    NUDGE_MODULE.resolve_codex_bin("codex")
            finally:
                if previous_home is None:
                    os.environ.pop("HOME", None)
                else:
                    os.environ["HOME"] = previous_home
                if previous_path is None:
                    os.environ.pop("PATH", None)
                else:
                    os.environ["PATH"] = previous_path

    def test_prompt_identifies_loca_sender_room_and_target(self):
        text = NUDGE_MODULE.nudge_text(
            {
                "room": "sb-dev",
                "sender": "operator",
                "target": "codex-dev",
                "text": "testleri kontrol et",
            }
        )
        self.assertIn("$loca", text)
        self.assertIn("operator", text)
        self.assertIn("sb-dev", text)
        self.assertIn("codex-dev", text)
        self.assertIn("testleri kontrol et", text)

    def test_prompt_combines_a_server_turn_without_losing_message_order(self):
        text = NUDGE_MODULE.nudge_text(
            {
                "t": "turn",
                "room": "sb-dev",
                "messages": [
                    {"sender": "operator", "text": "önce bunu yap"},
                    {"sender": "operator", "text": "sonra testi çalıştır"},
                    {"sender": "debug", "text": "şu loga da bak"},
                ],
            }
        )
        self.assertIn("queued 3 messages into one turn", text)
        self.assertLess(text.index("önce bunu yap"), text.index("sonra testi çalıştır"))
        self.assertLess(text.index("sonra testi çalıştır"), text.index("şu loga da bak"))

    def test_care_prompt_is_bounded_and_allows_one_direct_nudge(self):
        text = NUDGE_MODULE.nudge_text(
            {
                "t": "care",
                "signal": {
                    "room": "sb-dev",
                    "reason": "wait_cycle",
                    "target": "reviewer",
                    "subject": "worker waits for reviewer",
                    "context": [
                        {"sender": "worker", "text": "contract bekliyorum"},
                        {"sender": "reviewer", "text": "migration bekliyorum"},
                    ],
                },
            }
        )
        self.assertIn("wait_cycle", text)
        self.assertIn("reviewer", text)
        self.assertIn("contract bekliyorum", text)
        self.assertIn("exactly one direct message", text)

    def test_reaction_prompt_is_low_noise_and_does_not_invite_a_reply(self):
        text = NUDGE_MODULE.nudge_text(
            {
                "t": "reaction",
                "room": "sb-dev",
                "reaction": {
                    "message_id": 9,
                    "reactor": "operator",
                    "emoji": "✦",
                    "active": True,
                },
            }
        )
        self.assertIn("operator", text)
        self.assertIn("✦", text)
        self.assertIn("not a request for a reply", text)

    def test_codex_adapter_resumes_thread_and_starts_turn(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            requests = root / "requests.jsonl"
            fake_codex = root / "codex"
            fake_codex.write_text(
                """#!/usr/bin/env python3
import json
import os
import sys

log = os.environ["FAKE_CODEX_LOG"]
for line in sys.stdin:
    request = json.loads(line)
    with open(log, "a", encoding="utf-8") as out:
        out.write(json.dumps(request) + "\\n")
    if "id" not in request:
        continue
    method = request["method"]
    if method == "initialize":
        result = {"userAgent": "fake"}
    elif method == "thread/resume":
        result = {"thread": {"id": request["params"]["threadId"]}}
    elif method == "turn/start":
        result = {"turn": {"id": "turn-test", "status": "inProgress"}}
    else:
        print(json.dumps({"id": request["id"], "error": {"message": "unknown"}}), flush=True)
        continue
    print(json.dumps({"id": request["id"], "result": result}), flush=True)
    if method == "turn/start":
        print(json.dumps({
            "method": "turn/completed",
            "params": {
                "threadId": request["params"]["threadId"],
                "turn": {"id": "turn-test", "status": "completed", "items": []},
            },
        }), flush=True)
""",
                encoding="utf-8",
            )
            fake_codex.chmod(fake_codex.stat().st_mode | stat.S_IXUSR)
            env = dict(os.environ)
            env["FAKE_CODEX_LOG"] = str(requests)
            event = {
                "room": "iye",
                "sender": "operator",
                "target": "loca-dev",
                "text": "buraya bak",
            }
            result = subprocess.run(
                [
                    "python3",
                    str(NUDGE),
                    "codex",
                    "--thread-id",
                    "thread-test",
                    "--codex-bin",
                    str(fake_codex),
                ],
                input=json.dumps(event),
                text=True,
                capture_output=True,
                env=env,
                timeout=5,
                check=False,
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("Codex turn completed", result.stdout)
            recorded = [
                json.loads(line)
                for line in requests.read_text(encoding="utf-8").splitlines()
            ]
            methods = [request["method"] for request in recorded]
            self.assertEqual(
                methods,
                ["initialize", "initialized", "thread/resume", "turn/start"],
            )
            self.assertTrue(
                all("jsonrpc" not in request for request in recorded),
                "Codex app-server intentionally omits the JSON-RPC header on wire",
            )
            turn = recorded[-1]
            self.assertEqual(turn["params"]["threadId"], "thread-test")
            self.assertEqual(turn["params"]["approvalPolicy"], "never")
            self.assertNotIn("sandboxPolicy", turn["params"])
            self.assertEqual(turn["params"]["input"][1]["type"], "skill")
            self.assertEqual(turn["params"]["input"][1]["name"], "loca")
            self.assertIn("buraya bak", turn["params"]["input"][0]["text"])

    def test_codex_adapter_does_not_claim_success_before_turn_completed(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fake_codex = root / "codex"
            fake_codex.write_text(
                """#!/usr/bin/env python3
import json
import sys

for line in sys.stdin:
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request["method"]
    if method == "initialize":
        result = {"userAgent": "fake"}
    elif method == "thread/resume":
        result = {"thread": {"id": request["params"]["threadId"]}}
    elif method == "turn/start":
        result = {"turn": {"id": "never-completes", "status": "inProgress"}}
    else:
        result = {}
    print(json.dumps({"id": request["id"], "result": result}), flush=True)
""",
                encoding="utf-8",
            )
            fake_codex.chmod(fake_codex.stat().st_mode | stat.S_IXUSR)
            result = subprocess.run(
                [
                    "python3",
                    str(NUDGE),
                    "codex",
                    "--thread-id",
                    "thread-test",
                    "--codex-bin",
                    str(fake_codex),
                    "--turn-timeout",
                    "0.2",
                ],
                input='{"room":"iye","sender":"operator","text":"ping"}',
                text=True,
                capture_output=True,
                timeout=5,
                check=False,
            )

            self.assertEqual(result.returncode, 1)
            self.assertNotIn("Codex turn completed", result.stdout)
            self.assertIn("waiting for turn/completed", result.stderr)

    def test_codex_adapter_surfaces_child_stderr_when_initialize_dies(self):
        with tempfile.TemporaryDirectory() as tmp:
            fake_codex = Path(tmp) / "codex"
            fake_codex.write_text(
                """#!/bin/sh
read ignored
echo 'initialize rejected: protocol mismatch' >&2
exit 23
""",
                encoding="utf-8",
            )
            fake_codex.chmod(fake_codex.stat().st_mode | stat.S_IXUSR)
            result = subprocess.run(
                [
                    "python3",
                    str(NUDGE),
                    "codex",
                    "--thread-id",
                    "thread-test",
                    "--codex-bin",
                    str(fake_codex),
                    "--timeout",
                    "1",
                ],
                input='{"room":"iye","sender":"operator","text":"ping"}',
                text=True,
                capture_output=True,
                timeout=5,
                check=False,
            )
            self.assertEqual(result.returncode, 1)
            self.assertIn("exit 23", result.stderr)
            self.assertIn("initialize rejected", result.stderr)

    def test_codex_adapter_acks_steered_turn_only_after_completion(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            requests = root / "requests.jsonl"
            fake_codex = root / "codex"
            fake_codex.write_text(
                """#!/usr/bin/env python3
import json
import os
import sys

for line in sys.stdin:
    request = json.loads(line)
    with open(os.environ["FAKE_CODEX_LOG"], "a", encoding="utf-8") as out:
        out.write(json.dumps(request) + "\\n")
    if "id" not in request:
        continue
    method = request["method"]
    if method == "initialize":
        result = {}
    elif method == "thread/resume":
        result = {
            "thread": {
                "id": request["params"]["threadId"],
                "turns": [
                    {"id": "active-turn", "status": "inProgress", "items": []}
                ],
            }
        }
    elif method == "turn/steer":
        result = {"turnId": request["params"]["expectedTurnId"]}
    else:
        print(json.dumps({"id": request["id"], "error": {"message": "unexpected"}}), flush=True)
        continue
    print(json.dumps({"id": request["id"], "result": result}), flush=True)
    if method == "turn/steer":
        print(json.dumps({
            "method": "turn/completed",
            "params": {
                "threadId": request["params"]["threadId"],
                "turn": {"id": "active-turn", "status": "completed"},
            },
        }), flush=True)
""",
                encoding="utf-8",
            )
            fake_codex.chmod(fake_codex.stat().st_mode | stat.S_IXUSR)
            env = dict(os.environ)
            env["FAKE_CODEX_LOG"] = str(requests)
            env["LOCA_DELIVERY_ID"] = "sb-dev:42"
            result = subprocess.run(
                [
                    "python3",
                    str(NUDGE),
                    "codex",
                    "--thread-id",
                    "thread-test",
                    "--codex-bin",
                    str(fake_codex),
                    "--turn-timeout",
                    "0.2",
                ],
                input='{"room":"sb-dev","sender":"operator","text":"hemen bak"}',
                text=True,
                capture_output=True,
                env=env,
                timeout=5,
                check=False,
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("Codex active turn steered", result.stdout)
            self.assertIn("Codex steered turn completed", result.stdout)
            recorded = [
                json.loads(line)
                for line in requests.read_text(encoding="utf-8").splitlines()
            ]
            methods = [request["method"] for request in recorded]
            self.assertEqual(
                methods,
                ["initialize", "initialized", "thread/resume", "turn/steer"],
            )
            steer = recorded[-1]
            self.assertEqual(steer["params"]["expectedTurnId"], "active-turn")
            self.assertEqual(
                steer["params"]["clientUserMessageId"],
                "loca:sb-dev:42",
            )
            self.assertIn("hemen bak", steer["params"]["input"][0]["text"])

    def test_direct_operator_call_preempts_active_turn(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            requests = root / "requests.jsonl"
            fake_codex = root / "codex"
            fake_codex.write_text(
                """#!/usr/bin/env python3
import json
import os
import sys

resumes = 0
for line in sys.stdin:
    request = json.loads(line)
    with open(os.environ["FAKE_CODEX_LOG"], "a", encoding="utf-8") as out:
        out.write(json.dumps(request) + "\\n")
    if "id" not in request:
        continue
    method = request["method"]
    if method == "initialize":
        result = {}
    elif method == "thread/resume":
        resumes += 1
        turns = ([{"id":"old-work","status":"inProgress"}]
                 if resumes == 1 else [])
        result = {"thread":{"id":request["params"]["threadId"],"turns":turns}}
    elif method == "turn/interrupt":
        result = {}
    elif method == "turn/start":
        result = {"turn":{"id":"operator-turn","status":"inProgress"}}
    else:
        result = {}
    print(json.dumps({"id":request["id"],"result":result}), flush=True)
    if method == "turn/start":
        print(json.dumps({
            "method":"turn/completed",
            "params":{
                "threadId":request["params"]["threadId"],
                "turn":{"id":"operator-turn","status":"completed"},
            },
        }), flush=True)
""",
                encoding="utf-8",
            )
            fake_codex.chmod(fake_codex.stat().st_mode | stat.S_IXUSR)
            env = dict(os.environ)
            env.update(
                {
                    "FAKE_CODEX_LOG": str(requests),
                    "LOCA_PRIORITY": "direct_user",
                    "LOCA_FROM": "operator",
                    "LOCA_DELIVERY_ID": "sb-dev:99",
                }
            )
            result = subprocess.run(
                [
                    "python3", str(NUDGE), "codex", "--thread-id", "thread-test",
                    "--codex-bin", str(fake_codex), "--turn-timeout", "0.2",
                ],
                input='{"room":"sb-dev","sender":"operator","text":"hey"}',
                text=True,
                capture_output=True,
                env=env,
                timeout=5,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            recorded = [
                json.loads(line)
                for line in requests.read_text(encoding="utf-8").splitlines()
            ]
            methods = [request["method"] for request in recorded]
            self.assertEqual(
                methods,
                [
                    "initialize", "initialized", "thread/resume",
                    "turn/interrupt", "thread/resume", "turn/start",
                ],
            )
            interrupt = next(
                item for item in recorded if item.get("method") == "turn/interrupt"
            )
            self.assertEqual(
                interrupt["params"],
                {"threadId": "thread-test", "turnId": "old-work"},
            )
            self.assertNotIn("turn/steer", methods)
            self.assertIn("Codex turn completed", result.stdout)

    def test_codex_adapter_does_not_ack_a_steer_that_never_completes(self):
        with tempfile.TemporaryDirectory() as tmp:
            fake_codex = Path(tmp) / "codex"
            fake_codex.write_text(
                """#!/usr/bin/env python3
import json, sys
for line in sys.stdin:
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request["method"]
    if method == "initialize": result = {}
    elif method == "thread/resume":
        result = {"thread":{"turns":[{"id":"busy","status":"inProgress"}]}}
    elif method == "turn/steer": result = {"turnId":"busy"}
    else: result = {}
    print(json.dumps({"id":request["id"],"result":result}), flush=True)
""",
                encoding="utf-8",
            )
            fake_codex.chmod(fake_codex.stat().st_mode | stat.S_IXUSR)
            result = subprocess.run(
                [
                    "python3", str(NUDGE), "codex", "--thread-id", "thread-test",
                    "--codex-bin", str(fake_codex), "--turn-timeout", "0.15",
                ],
                input='{"room":"sb-dev","sender":"operator","text":"hey"}',
                text=True,
                capture_output=True,
                timeout=5,
                check=False,
            )
            self.assertEqual(result.returncode, 1)
            self.assertIn("waiting for turn/completed", result.stderr)

    def test_codex_adapter_can_explicitly_disable_inner_sandbox(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            requests = root / "requests.jsonl"
            fake_codex = root / "codex"
            fake_codex.write_text(
                """#!/usr/bin/env python3
import json
import os
import sys
for line in sys.stdin:
    request = json.loads(line)
    with open(os.environ["FAKE_CODEX_LOG"], "a", encoding="utf-8") as out:
        out.write(json.dumps(request) + "\\n")
    if "id" not in request:
        continue
    method = request["method"]
    if method == "initialize":
        result = {}
    elif method == "thread/resume":
        result = {"thread": {"id": request["params"]["threadId"]}}
    elif method == "turn/start":
        result = {"turn": {"id": "turn-test", "status": "inProgress"}}
    else:
        result = {}
    print(json.dumps({"id": request["id"], "result": result}), flush=True)
    if method == "turn/start":
        print(json.dumps({"method":"turn/completed","params":{"threadId":"thread-test","turn":{"id":"turn-test","status":"completed"}}}), flush=True)
""",
                encoding="utf-8",
            )
            fake_codex.chmod(fake_codex.stat().st_mode | stat.S_IXUSR)
            env = dict(os.environ)
            env["FAKE_CODEX_LOG"] = str(requests)
            result = subprocess.run(
                [
                    "python3",
                    str(NUDGE),
                    "codex",
                    "--thread-id",
                    "thread-test",
                    "--codex-bin",
                    str(fake_codex),
                    "--sandbox-policy",
                    "danger-full-access",
                ],
                input='{"room":"iye","sender":"operator","text":"ping"}',
                text=True,
                capture_output=True,
                env=env,
                timeout=5,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            recorded = [
                json.loads(line)
                for line in requests.read_text(encoding="utf-8").splitlines()
            ]
            turn = next(item for item in recorded if item.get("method") == "turn/start")
            self.assertEqual(
                turn["params"]["sandboxPolicy"],
                {"type": "dangerFullAccess"},
            )

    def test_codex_adapter_requires_a_bound_thread(self):
        result = subprocess.run(
            ["python3", str(NUDGE), "codex"],
            input='{"room":"iye","sender":"operator","text":"ping"}',
            text=True,
            capture_output=True,
            env={key: value for key, value in os.environ.items() if key != "CODEX_THREAD_ID"},
            timeout=5,
            check=False,
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("invoke $loca once", result.stderr)

    # FIX 4: the recoverable Codex sandbox netns denial must be named, not
    # buried in an opaque stderr dump.
    def test_classify_sandbox_failure_names_the_netns_denial(self):
        for sample in (
            "RTM_NEWADDR: Operation not permitted",
            "bwrap: setting up uid map: Operation not permitted",
            "failed to create user namespace",
        ):
            typed = NUDGE_MODULE.classify_sandbox_failure(sample)
            self.assertIsNotNone(typed, sample)
            self.assertIn("danger-full-access", typed)
            self.assertIn("netns", typed)

    def test_classify_sandbox_failure_ignores_unrelated_stderr(self):
        self.assertIsNone(NUDGE_MODULE.classify_sandbox_failure(""))
        self.assertIsNone(
            NUDGE_MODULE.classify_sandbox_failure("initialize rejected: bad json")
        )

    def test_failure_detail_prepends_typed_sandbox_cause(self):
        from collections import deque

        client = NUDGE_MODULE.AppServer.__new__(NUDGE_MODULE.AppServer)
        client.proc = mock.Mock()
        client.proc.poll.return_value = 1
        client.stderr_reader = mock.Mock()
        client.stderr_tail = deque(
            ["codex bwrap error", "RTM_NEWADDR: Operation not permitted"]
        )
        detail = client.failure_detail()
        # Typed, actionable cause comes first; the raw tail is still included.
        self.assertTrue(detail.startswith("Codex sandbox netns unavailable"))
        self.assertIn("danger-full-access", detail)
        self.assertIn("RTM_NEWADDR", detail)

    def test_sandbox_netns_failure_reaches_stderr_end_to_end(self):
        with tempfile.TemporaryDirectory() as tmp:
            fake_codex = Path(tmp) / "codex"
            fake_codex.write_text(
                """#!/bin/sh
read ignored
echo 'RTM_NEWADDR: Operation not permitted' >&2
exit 71
""",
                encoding="utf-8",
            )
            fake_codex.chmod(fake_codex.stat().st_mode | stat.S_IXUSR)
            result = subprocess.run(
                [
                    "python3",
                    str(NUDGE),
                    "codex",
                    "--thread-id",
                    "thread-test",
                    "--codex-bin",
                    str(fake_codex),
                    "--timeout",
                    "1",
                ],
                input='{"room":"iye","sender":"operator","text":"ping"}',
                text=True,
                capture_output=True,
                timeout=5,
                check=False,
            )
            self.assertEqual(result.returncode, 1)
            self.assertIn("Codex sandbox netns unavailable", result.stderr)
            self.assertIn("danger-full-access", result.stderr)


if __name__ == "__main__":
    unittest.main()
