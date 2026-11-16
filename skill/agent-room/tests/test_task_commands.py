"""connect.sh task commands: the documented routes, the agent's own
credentials, and a refusal that is never data and never exit 0."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

SKILL_DIR = Path(__file__).resolve().parents[1]
CONNECT = SKILL_DIR / "connect.sh"

INVITE = "dv_reviewerTASKFIXTURE"
SESSION = "st_reviewerTASKFIXTURE"

TASKS = [
    {"id": 1, "room": "sb-dev", "title": "unassigned", "created_by": "op", "status": "open",
     "created_at": 1_800_000_000_000, "progress_at": 1_800_000_000_000},
    {"id": 2, "room": "sb-dev", "title": "mine to start", "created_by": "op", "assigned_to": "Reviewer",
     "status": "open", "created_at": 1_800_000_000_000, "progress_at": 1_800_000_000_000},
    {"id": 3, "room": "sb-dev", "title": "mine in progress", "created_by": "op", "assigned_to": "reviewer",
     "status": "taken", "created_at": 1_800_000_000_000, "progress_at": 1_800_000_000_000},
    {"id": 4, "room": "sb-dev", "title": "mine, finished", "created_by": "op", "assigned_to": "reviewer",
     "status": "done", "created_at": 1_800_000_000_000, "progress_at": 1_800_000_000_000},
    {"id": 5, "room": "sb-dev", "title": "someone else's", "created_by": "op", "assigned_to": "other",
     "status": "taken", "created_at": 1_800_000_000_000, "progress_at": 1_800_000_000_000},
]


class TaskCommandTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.patch_status = 200
        self.patch_body = b"{}"
        self.patch_401_once = False
        self.post_status = 201
        self.post_body = b"{}"
        self.seen = []
        case = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def _send(self, status, body):
                self.send_response(status)
                plain = not body.startswith((b"[", b"{"))
                self.send_header("content-type", "text/plain" if plain else "application/json")
                self.send_header("content-length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def _answer(self):
                length = int(self.headers.get("content-length") or 0)
                sent = self.rfile.read(length) if length else b""
                case.seen.append({
                    "method": self.command, "path": self.path,
                    "invite": self.headers.get("x-room-token"),
                    "session": self.headers.get("x-session-token"),
                    "body": json.loads(sent) if sent else None,
                })
                if self.command == "POST" and self.path == "/sessions":
                    return self._send(201, json.dumps({"session_token": SESSION}).encode())
                if self.command == "GET" and self.path == "/rooms/sb-dev/tasks":
                    return self._send(200, json.dumps(TASKS).encode())
                if self.command == "POST" and self.path == "/rooms/sb-dev/tasks":
                    return self._send(case.post_status, case.post_body)
                if self.command == "PATCH" and self.path.startswith("/rooms/sb-dev/tasks/"):
                    if case.patch_401_once:
                        case.patch_401_once = False
                        return self._send(401, b"invalid or missing session")
                    return self._send(case.patch_status, case.patch_body)
                return self._send(404, b"not found")

            do_GET = do_POST = do_PATCH = _answer

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.origin = f"http://127.0.0.1:{self.server.server_port}"
        self.env = {key: value for key, value in os.environ.items()
                    if not key.startswith(("LOCA_", "DAVET_", "ROOM_"))}
        self.env["HOME"] = str(self.root)
        # Another agent's default file, and this agent's own, selected by name.
        self.identity(self.root / ".loca" / "env", "other-agent", "dv_otherTASKFIXTURE")
        self.identity(self.root / ".loca" / "reviewer.env", "reviewer", INVITE)

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=2)
        self.tmp.cleanup()

    def identity(self, path, name, invite):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(f"ROOM_SERVER_URL={self.origin}\nLOCA_NAME={name}\nDAVET_sb_dev={invite}\n")
        path.chmod(0o600)

    def run_connect(self, *args):
        return subprocess.run([str(CONNECT), *args], env=self.env,
                              capture_output=True, text=True, timeout=30)

    def assert_no_secret(self, result):
        for secret in (INVITE, SESSION):
            self.assertNotIn(secret, result.stdout)
            self.assertNotIn(secret, result.stderr)

    def test_tasks_lists_every_task_and_mine_only_my_unfinished_ones(self):
        result = self.run_connect("tasks", self.origin, "sb-dev")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout), TASKS)

        result = self.run_connect("tasks", self.origin, "sb-dev", "--mine", "reviewer")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual([task["id"] for task in json.loads(result.stdout)], [2, 3])
        # --mine reads with the named agent's own invite.
        self.assertEqual(self.seen[-1]["invite"], INVITE)
        self.assert_no_secret(result)

        result = self.run_connect("tasks", self.origin, "sb-dev", "--mine")
        self.assertEqual(result.returncode, 2)
        self.assertEqual(result.stdout, "")

    def test_task_shows_one_task_and_a_missing_one_is_not_data(self):
        result = self.run_connect("task", self.origin, "sb-dev", "3")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout), TASKS[2])
        result = self.run_connect("task", self.origin, "sb-dev", "99")
        self.assertEqual(result.returncode, 8)
        self.assertEqual(result.stdout, "")
        self.assertIn("no task #99", result.stderr)

    def test_take_claims_only_an_unassigned_task_and_done_finishes(self):
        self.patch_body = json.dumps({"id": 1, "status": "taken"}).encode()
        for args, path, body in (
            (["task-take", self.origin, "sb-dev", "reviewer", "1"], "/rooms/sb-dev/tasks/1",
             {"status": "taken", "assigned_to": "reviewer", "by": "reviewer"}),
            (["task-take", self.origin, "sb-dev", "reviewer", "2"], "/rooms/sb-dev/tasks/2",
             {"status": "taken", "by": "reviewer"}),
            (["task-done", self.origin, "sb-dev", "reviewer", "3"], "/rooms/sb-dev/tasks/3",
             {"status": "done", "by": "reviewer"}),
        ):
            with self.subTest(command=args[0], task=args[-1]):
                self.seen.clear()
                result = self.run_connect(*args)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(json.loads(result.stdout), {"id": 1, "status": "taken"})
                patch = [request for request in self.seen if request["method"] == "PATCH"]
                self.assertEqual(len(patch), 1)
                self.assertEqual(patch[0]["path"], path)
                self.assertEqual(patch[0]["body"], body)
                # The author is proven by this agent's invite and session.
                self.assertEqual(patch[0]["invite"], INVITE)
                self.assertEqual(patch[0]["session"], SESSION)
                self.assert_no_secret(result)

    def test_a_refused_change_prints_nothing_and_exits_with_the_read_codes(self):
        answers = {
            401: (b"invalid or missing session", 7),
            403: (b"house rules: agents take/finish their own tasks; cancel/reopen/reassign is the operator's", 4),
            404: (b"no such task", 8),
            409: (b"this loca is closed - read-only", 8),
            429: (b"rate limit exceeded", 5),
            503: (b"could not save task - try again", 8),
        }
        for command in ("task-take", "task-done"):
            for status, (sentence, expected) in answers.items():
                with self.subTest(command=command, status=status):
                    self.patch_status, self.patch_body = status, sentence
                    result = self.run_connect(command, self.origin, "sb-dev", "reviewer", "5")
                    self.assertEqual(result.returncode, expected, result.stderr)
                    self.assertEqual(result.stdout, "")
                    self.assertIn(sentence.decode(), result.stderr)
                    self.assertIn(str(status), result.stderr)
                    self.assertIn("not changed", result.stderr.lower())
                    self.assert_no_secret(result)

    def test_a_dead_session_is_renewed_once_and_the_change_retried(self):
        self.patch_401_once = True
        self.patch_body = b'{"id": 3, "status": "done"}'
        result = self.run_connect("task-done", self.origin, "sb-dev", "reviewer", "3")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual([r["method"] for r in self.seen if r["method"] == "PATCH"], ["PATCH", "PATCH"])
        self.assertTrue(any(r["path"] == "/sessions" for r in self.seen))

    def test_a_task_number_is_required(self):
        for args in (["task", self.origin, "sb-dev", "x"],
                     ["task-take", self.origin, "sb-dev", "reviewer", ""],
                     ["task-done", self.origin, "sb-dev", "reviewer", "1;2"]):
            with self.subTest(args=args):
                result = self.run_connect(*args)
                self.assertEqual(result.returncode, 2)
                self.assertEqual(result.stdout, "")
                self.assertEqual(self.seen, [])


class LeadTaskCommandTest(TaskCommandTest):
    """task-new and task-assign: the Lead's acts, over the same routes, with
    the same refusal contract as the holder's."""

    # The same fixture server; the holder's tests run once, in their class.
    for _inherited in [name for name in vars(TaskCommandTest) if name.startswith("test_")]:
        vars()[_inherited] = None
    del _inherited

    def written(self, method):
        return [request for request in self.seen if request["method"] == method]

    def test_task_new_declares_with_and_without_an_assignee(self):
        self.post_body = json.dumps({"id": 6, "status": "open"}).encode()
        for extra, body in (
            ([], {"title": "write the upgrade note", "by": "reviewer"}),
            (["--assign", "Other"], {"title": "write the upgrade note", "by": "reviewer", "assigned_to": "Other"}),
        ):
            with self.subTest(extra=extra):
                self.seen.clear()
                result = self.run_connect("task-new", self.origin, "sb-dev", "reviewer",
                                          "write the upgrade note", *extra)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(json.loads(result.stdout), {"id": 6, "status": "open"})
                post = self.written("POST")
                post = [request for request in post if request["path"] == "/rooms/sb-dev/tasks"]
                self.assertEqual(len(post), 1)
                self.assertEqual(post[0]["body"], body)
                self.assertEqual(post[0]["invite"], INVITE)
                self.assertEqual(post[0]["session"], SESSION)
                self.assert_no_secret(result)

    def test_task_assign_gives_a_task_to_someone(self):
        self.patch_body = json.dumps({"id": 1, "assigned_to": "other"}).encode()
        result = self.run_connect("task-assign", self.origin, "sb-dev", "reviewer", "1", "other")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout), {"id": 1, "assigned_to": "other"})
        patch = self.written("PATCH")
        self.assertEqual(len(patch), 1)
        self.assertEqual(patch[0]["path"], "/rooms/sb-dev/tasks/1")
        self.assertEqual(patch[0]["body"], {"assigned_to": "other", "by": "reviewer"})
        self.assertEqual(patch[0]["invite"], INVITE)
        self.assertEqual(patch[0]["session"], SESSION)
        self.assert_no_secret(result)

    def test_a_refused_lead_act_prints_nothing_and_exits_with_the_read_codes(self):
        answers = {
            401: (b"invalid or missing session", 7),
            403: (b"declaring a task takes operator or Lead authority in this loca", 4),
            404: (b"no such task", 8),
            409: (b"this loca is closed - read-only", 8),
            429: (b"rate limit exceeded", 5),
            503: (b"could not save task - try again", 8),
        }
        commands = (
            ("task-new", ["task-new", self.origin, "sb-dev", "reviewer", "a title", "--assign", "other"]),
            ("task-assign", ["task-assign", self.origin, "sb-dev", "reviewer", "5", "reviewer"]),
        )
        for command, args in commands:
            for status, (sentence, expected) in answers.items():
                with self.subTest(command=command, status=status):
                    self.post_status = self.patch_status = status
                    self.post_body = self.patch_body = sentence
                    result = self.run_connect(*args)
                    self.assertEqual(result.returncode, expected, result.stderr)
                    self.assertEqual(result.stdout, "")
                    self.assertIn(sentence.decode(), result.stderr)
                    self.assertIn(str(status), result.stderr)
                    self.assertIn("not changed", result.stderr.lower())
                    self.assert_no_secret(result)

    def test_the_lead_acts_check_their_arguments_before_asking(self):
        for args in (["task-new", self.origin, "sb-dev", "reviewer", "  "],
                     ["task-new", self.origin, "sb-dev", "reviewer", "t", "--assign"],
                     ["task-new", self.origin, "sb-dev", "reviewer", "t", "--from", "3"],
                     ["task-assign", self.origin, "sb-dev", "reviewer", "x", "other"],
                     ["task-assign", self.origin, "sb-dev", "reviewer", "1"]):
            with self.subTest(args=args):
                result = self.run_connect(*args)
                self.assertEqual(result.returncode, 2)
                self.assertEqual(result.stdout, "")
                self.assertEqual(self.seen, [])


class BoardTaskCommandTest(LeadTaskCommandTest):
    """The Task board: task-done with a note asks for review; task-go and
    task-nogo are the review, with the same refusal contract."""

    for _inherited in [name for name in vars(LeadTaskCommandTest) if name.startswith("test_")]:
        vars()[_inherited] = None
    del _inherited

    def test_board_commands_send_the_documented_bodies(self):
        self.patch_body = json.dumps({"id": 3, "status": "review"}).encode()
        for args, body in (
            (["task-done", self.origin, "sb-dev", "reviewer", "3", "renewal fixed; run the e2e"],
             {"status": "done", "note": "renewal fixed; run the e2e", "by": "reviewer"}),
            (["task-go", self.origin, "sb-dev", "reviewer", "3"],
             {"status": "done", "by": "reviewer"}),
            (["task-nogo", self.origin, "sb-dev", "reviewer", "3", "the expiry path is untested"],
             {"status": "taken", "reason": "the expiry path is untested", "by": "reviewer"}),
        ):
            with self.subTest(command=args[0]):
                self.seen.clear()
                result = self.run_connect(*args)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(json.loads(result.stdout), {"id": 3, "status": "review"})
                patch = self.written("PATCH")
                self.assertEqual(len(patch), 1)
                self.assertEqual(patch[0]["path"], "/rooms/sb-dev/tasks/3")
                self.assertEqual(patch[0]["body"], body)
                self.assertEqual(patch[0]["invite"], INVITE)
                self.assertEqual(patch[0]["session"], SESSION)
                self.assert_no_secret(result)

    def test_a_task_in_review_is_still_mine(self):
        TASKS.append({"id": 6, "room": "sb-dev", "title": "mine in review", "created_by": "op",
                      "assigned_to": "reviewer", "status": "review",
                      "created_at": 1_800_000_000_000, "progress_at": 1_800_000_000_000})
        try:
            result = self.run_connect("tasks", self.origin, "sb-dev", "--mine", "reviewer")
        finally:
            TASKS.pop()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual([task["id"] for task in json.loads(result.stdout)], [2, 3, 6])

    def test_a_refused_review_prints_nothing_and_exits_with_the_read_codes(self):
        answers = {
            400: (b"a no-go needs a reason", 8),
            401: (b"invalid or missing session", 7),
            403: (b"house rules: agents take/finish their own tasks; ...", 4),
            404: (b"no such task", 8),
            429: (b"rate limit exceeded", 5),
        }
        for args in (["task-go", self.origin, "sb-dev", "reviewer", "5"],
                     ["task-nogo", self.origin, "sb-dev", "reviewer", "5", "why"]):
            for status, (sentence, expected) in answers.items():
                with self.subTest(command=args[0], status=status):
                    self.patch_status, self.patch_body = status, sentence
                    result = self.run_connect(*args)
                    self.assertEqual(result.returncode, expected, result.stderr)
                    self.assertEqual(result.stdout, "")
                    self.assertIn(sentence.decode(), result.stderr)
                    self.assertIn("not changed", result.stderr.lower())
                    self.assert_no_secret(result)

    def test_a_no_go_needs_a_reason_before_asking(self):
        for args in (["task-nogo", self.origin, "sb-dev", "reviewer", "3"],
                     ["task-nogo", self.origin, "sb-dev", "reviewer", "3", "  "],
                     ["task-go", self.origin, "sb-dev", "reviewer", "x"]):
            with self.subTest(args=args):
                result = self.run_connect(*args)
                self.assertEqual(result.returncode, 2)
                self.assertEqual(result.stdout, "")
                self.assertEqual(self.seen, [])


if __name__ == "__main__":
    unittest.main()
