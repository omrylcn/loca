"""Durable production-shaped Wiki runner; no inference or live identities."""
from pathlib import Path
import sys
import tempfile
import unittest
import sqlite3
import argparse
from contextlib import redirect_stdout
import io
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from wiki_runtime import WikiRunner, WikiJournal, WorkerUncertain
import wiki_runtime


class Transport:
    def __init__(self):
        self.calls = []
        self.batch = {"room":"a", "after_id":0, "through_id":1,
            "lease":{"batch_id":8,"epoch":1}, "lease_expires_at":9999999999999,
            "messages":[{"id":1,"room":"a","text":"decision"}]}
        self.snapshot = {"room":"a","revision":0,"reviewed_through":0,"pages":[]}
        self.receipt = {"revision":1}
        self.lost = False

    def claim(self):
        self.calls.append("claim")
        return self.batch

    def read(self):
        self.calls.append("read")
        return self.snapshot

    def renew(self, _lease):
        self.calls.append("renew")
        return {"lease_expires_at":9999999999999}

    def finish(self, payload):
        self.calls.append(("finish",payload))
        if self.lost:
            raise OSError("connection lost after acceptance")
        return self.receipt


class WikiRuntimeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.path = Path(self.temp.name)/"state.sqlite3"
        self.transport = Transport()
        self.invocations = 0
        self.journal = WikiJournal(self.path, "https://example.test", "editor")

    def tearDown(self):
        self.journal.close()
        self.temp.cleanup()

    def worker(self, payload, renew):
        self.invocations += 1
        renew()
        self.assertNotIn("lease",payload)
        self.assertNotIn("credentials",payload)
        return {"expected_revision":0,"reviewed_through":1,"reason":"sourced",
            "pages":[{"slug":"overview","title":"Overview","body":"decision","sources":[1]}]}

    def runner(self, worker=None):
        return WikiRunner("a",self.journal,self.transport,worker or self.worker)

    def test_complete_intent_result_and_publication(self):
        self.assertEqual(self.runner().run_once()["status"],"accepted")
        self.assertEqual(self.invocations,1)
        self.assertEqual(self.journal.latest("a")["status"],"accepted")

    def test_accepted_history_is_bounded_without_duplicate_private_inputs(self):
        for batch_id in range(1,131):
            self.transport.batch["lease"]["batch_id"] = batch_id
            job = self.journal.intent("a",self.transport.batch,self.transport.snapshot)
            self.journal.result(job,{"private":"draft"})
            job = self.journal.latest("a")
            self.journal.accepted(job,{"revision":batch_id})
        self.assertEqual(self.journal.connection.execute("SELECT COUNT(*) FROM jobs").fetchone()[0],128)
        latest = self.journal.latest("a")
        self.assertEqual(latest["batch"],{"lease":self.transport.batch["lease"]})
        self.assertEqual(latest["snapshot"],{})
        self.assertIsNone(latest["review"])
        self.assertEqual(latest["receipt"],{"revision":130})

    def test_unresolved_journal_budget_preserves_uncertainty_and_rejects_new_work(self):
        for count in range(16):
            job = self.journal.intent(f"room-{count}",self.transport.batch,self.transport.snapshot)
            self.journal.block(job,"unknown")
        with self.assertRaisesRegex(ValueError,"reconciliation"):
            self.journal.intent("new-room",self.transport.batch,self.transport.snapshot)
        self.assertEqual(self.journal.connection.execute("SELECT COUNT(*) FROM jobs").fetchone()[0],16)
        self.assertEqual(self.journal.latest("room-0")["reason"],"unknown")

    def test_oversized_durable_input_is_not_truncated_or_dispatched(self):
        snapshot = {"text":"x"*(2*1024*1024)}
        with self.assertRaisesRegex(ValueError,"durable budget"):
            self.journal.intent("a",self.transport.batch,snapshot)
        self.assertIsNone(self.journal.latest("a"))

    def test_publication_retry_across_restart_never_repeats_model_or_claim(self):
        self.transport.lost = True
        with self.assertRaises(OSError): self.runner().run_once()
        self.assertEqual(self.journal.latest("a")["status"],"publication_pending")
        first = self.transport.calls[-1][1]
        self.journal.close()
        self.journal = WikiJournal(self.path,"https://example.test","editor")
        self.transport.lost = False
        self.transport.claim = lambda: self.fail("retry took another lease")
        self.assertEqual(self.runner().run_once()["status"],"accepted")
        self.assertEqual(first,self.transport.calls[-1][1])
        self.assertEqual(self.invocations,1)
        self.assertEqual(self.journal.latest("a")["status"],"accepted")

    def test_second_writer_cannot_claim_or_corrupt_the_journal(self):
        with self.assertRaises((BlockingIOError,OSError)):
            WikiJournal(self.path,"https://example.test","editor")
        self.assertEqual(self.runner().run_once()["status"],"accepted")

    def test_crash_after_dispatch_intent_is_blocked_not_reexecuted(self):
        def killed(_payload,_renew): raise WorkerUncertain("native process killed")
        self.assertEqual(self.runner(killed).run_once()["status"],"blocked")
        self.assertEqual(self.runner().run_once()["status"],"blocked")
        self.assertEqual(self.invocations,0)

    def test_expired_or_reassigned_lease_does_not_run_worker(self):
        def rejected(_lease): raise PermissionError("reassigned")
        self.transport.renew = rejected
        with self.assertRaises(PermissionError): self.runner().run_once()
        self.assertEqual(self.invocations,0)

    def test_stale_result_is_not_republished_under_new_epoch(self):
        self.transport.lost = True
        with self.assertRaises(OSError): self.runner().run_once()
        self.transport.batch["lease"]["epoch"] = 2
        self.transport.lost = False
        self.runner().run_once()
        self.assertEqual(self.transport.calls[-1][1]["lease"]["epoch"],1)

    def test_invalid_worker_result_is_blocked_not_published(self):
        def invalid(_payload,_renew): return {"pages":[]}
        self.assertEqual(self.runner(invalid).run_once()["status"],"blocked")
        self.assertFalse(any(isinstance(call,tuple) for call in self.transport.calls))

    def test_missing_worker_does_not_claim(self):
        self.transport.claim = lambda: self.fail("unsupported worker claimed")
        runner = WikiRunner("a",self.journal,self.transport,None)
        self.assertEqual(runner.run_once()["status"],"blocked")

    def test_no_input_does_not_dispatch(self):
        self.transport.batch = None
        self.assertEqual(self.runner().run_once()["status"],"idle")
        self.assertEqual(self.invocations,0)

    def test_foreign_journal_identity_or_origin_is_rejected(self):
        self.journal.close()
        for server,name in (("https://foreign.test","editor"),("https://example.test","other")):
            with self.assertRaises(ValueError): WikiJournal(self.path,server,name)

    def test_database_private_and_symlink_rejected(self):
        self.assertEqual(self.path.stat().st_mode & 0o777,0o600)
        link = Path(self.temp.name)/"link"
        link.symlink_to(self.path)
        with self.assertRaises(ValueError): WikiJournal(link,"https://example.test","editor")

    def test_unrelated_database_is_not_mutated_or_chmodded(self):
        path = Path(self.temp.name)/"building.sqlite3"
        with sqlite3.connect(path) as connection:
            connection.execute("CREATE TABLE private_data(value TEXT)")
            connection.execute("INSERT INTO private_data VALUES('keep')")
        path.chmod(0o640)
        original = path.read_bytes()
        with self.assertRaises(ValueError): WikiJournal(path,"https://example.test","editor")
        self.assertEqual(path.read_bytes(),original)
        self.assertEqual(path.stat().st_mode & 0o777,0o640)

    def main_args(self, rooms):
        return argparse.Namespace(server="https://example.test",name="editor",rooms=rooms,
            journal=Path(self.temp.name)/"main.sqlite3",runtime="codex",binary=None,node=None,once=False)

    def test_blocked_room_does_not_starve_another_room(self):
        args = self.main_args("a,b"); args.once = True
        called = []
        class Runner:
            def __init__(self, room, *_args):
                self.room = room
                self.transport=type("Transport",(),{"ready":lambda _self,_runtime:None,
                    "read":lambda _self:{"editor_name":"editor","enabled":True}})()
            def run_once(self):
                called.append(self.room)
                return {"status":"blocked" if self.room=="a" else "idle"}
        with patch.object(argparse.ArgumentParser,"parse_args",return_value=args), \
                patch.object(wiki_runtime,"ProcessWorker"),patch.object(wiki_runtime,"WikiRunner",Runner), \
                patch.object(wiki_runtime.signal,"signal"),redirect_stdout(io.StringIO()):
            self.assertEqual(wiki_runtime.main(),1)
        self.assertEqual(called,["a","b"])

    def test_no_input_polling_does_not_log_the_same_idle_receipt_repeatedly(self):
        args = self.main_args("a")
        class Stop:
            ticks = 0
            def is_set(self): return self.ticks>=3
            def wait(self,_seconds): self.ticks += 1
        class Runner:
            def __init__(self,room,*_args):
                self.room = room
                self.transport=type("Transport",(),{"ready":lambda _self,_runtime:None,
                    "read":lambda _self:{"editor_name":"editor","enabled":True}})()
            def run_once(self): return {"status":"idle"}
        output = io.StringIO()
        with patch.object(argparse.ArgumentParser,"parse_args",return_value=args), \
                patch.object(wiki_runtime,"ProcessWorker"),patch.object(wiki_runtime,"WikiRunner",Runner), \
                patch.object(wiki_runtime.threading,"Event",return_value=Stop()), \
                patch.object(wiki_runtime.signal,"signal"),redirect_stdout(output):
            self.assertEqual(wiki_runtime.main(),0)
        self.assertEqual(len(output.getvalue().splitlines()),1)


if __name__ == "__main__": unittest.main()
