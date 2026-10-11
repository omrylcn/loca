"""Actual shell helper boundary; HTTP responses below are explicit fixtures."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


SKILL = Path(__file__).resolve().parents[1]


class WikiTransportTests(unittest.TestCase):
    def invoke(self, command, replies, extra=()):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / ".loca").mkdir()
            identity = root / ".loca/editor.env"
            identity.write_text("ROOM_SERVER_URL=https://loca.example\nLOCA_NAME=editor\n"
                                "LOCA_SESSION=st_fixture_old\nDAVET_general=dv_fixture\n")
            fixture = root / "responses.json"
            fixture.write_text(json.dumps(replies))
            bindir = root / "bin"
            bindir.mkdir()
            fake = bindir / "curl"
            fake.write_text("""#!/usr/bin/env python3
import json, os, pathlib, sys
p = pathlib.Path(os.environ['WIKI_FIXTURE'])
values = json.loads(p.read_text())
reply = values.pop(0)
p.write_text(json.dumps(values))
with open(os.environ['WIKI_CALLS'], 'a') as out:
    out.write(json.dumps(sys.argv[1:]) + '\\n')
print(reply['body'], end='\\n' + str(reply['status']))
""")
            fake.chmod(0o755)
            payload = root / "review.json"
            payload.write_text(json.dumps({"lease":{"batch_id":1,"epoch":2},
                                          "review":{"pages":[]}}))
            arguments = [str(payload)] if command == "wiki-finish" else list(extra)
            env = {"HOME":str(root), "PATH":str(bindir)+":"+os.environ.get("PATH", ""),
                   "WIKI_FIXTURE":str(fixture),"WIKI_CALLS":str(root / "calls")}
            result = subprocess.run(["bash",str(SKILL / "connect.sh"),command,
                                     "https://loca.example","general","editor",*arguments],
                                    env=env,capture_output=True,text=True,timeout=8)
            calls = [json.loads(line) for line in (root / "calls").read_text().splitlines()]
            return result, calls, identity.read_text()

    def test_http_rejection_is_not_success_and_does_not_echo_error_body(self):
        for status in (403,409,500):
            result, calls, _ = self.invoke("wiki-finish", [{"status":status,"body":"do not echo st_PRIVATE"}])
            self.assertNotEqual(result.returncode,0)
            self.assertIn("HTTP " + str(status),result.stderr)
            self.assertNotIn("st_PRIVATE",result.stdout+result.stderr)
            self.assertEqual(len(calls),1)

    def test_valid_null_claim_is_idle_and_invalid_json_not_accepted(self):
        result, _, _ = self.invoke("wiki-claim", [{"status":200,"body":"null"}])
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertIsNone(json.loads(result.stdout))
        result, _, _ = self.invoke("wiki-claim", [{"status":200,"body":"gateway failure"}])
        self.assertNotEqual(result.returncode,0)
        self.assertIn("unverified",result.stderr)

    def test_expired_session_renews_once_with_same_fenced_request(self):
        result,calls,identity = self.invoke("wiki-finish",[
            {"status":401,"body":"expired"},
            {"status":201,"body":json.dumps({"session_token":"st_fixture_new"})},
            {"status":200,"body":json.dumps({"revision":7})}])
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(json.loads(result.stdout),{"revision":7})
        self.assertIn("LOCA_SESSION=st_fixture_new",identity)
        self.assertEqual(calls[0][-1],calls[2][-1])
        self.assertTrue(any("st_fixture_new" in argument for argument in calls[2]))
        self.assertNotIn("st_fixture",result.stdout+result.stderr)

    def test_rejected_session_renewal_is_bounded(self):
        result,calls,_ = self.invoke("wiki-claim",[
            {"status":401,"body":"expired"},{"status":401,"body":"revoked"}])
        self.assertNotEqual(result.returncode,0)
        self.assertEqual(len(calls),2)

    def test_page_read_uses_single_page_endpoint_and_checks_room(self):
        value = {"room":"general","revision":1,"page":{"slug":"working","body":"selected"}}
        result,calls,_ = self.invoke("wiki-page",[{"status":200,"body":json.dumps(value)}],("working",))
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(json.loads(result.stdout),value)
        self.assertEqual(calls[0][-1],"https://loca.example/rooms/general/wiki/pages/working")
        value["room"] = "other"
        result,_,_ = self.invoke("wiki-page",[{"status":200,"body":json.dumps(value)}],("working",))
        self.assertNotEqual(result.returncode,0)
        self.assertFalse(result.stdout.strip())


if __name__ == "__main__":
    unittest.main()
