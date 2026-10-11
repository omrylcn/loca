import json
import sys
import unittest
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from wiki_reader import WikiReader


class WikiReaderTests(unittest.TestCase):
    def setUp(self):
        self.calls=[]
        self.snapshot={"room":"a","revision":1,"selection_version":0,"pages":[
            {"slug":"overview","title":"Overview","body":"old\nunchanged\n","sources":[1],"revision":1}]}
        def call(command,server,room,payload=None):
            self.calls.append((command,payload))
            return json.loads(json.dumps(self.snapshot)) if command=="wiki-context" else {"accepted":True}
        self.reader=WikiReader("/unused","agent",call)
        self.attention={"server":"https://example.test","room":"a"}

    def test_offer_without_native_acceptance_never_sets_baseline_or_receipt(self):
        offer=self.reader.prepare(self.attention,"thread")
        self.assertEqual(offer[1]["pages"][0]["kind"],"bootstrap")
        self.assertEqual([c[0] for c in self.calls],["wiki-context"])
        self.assertFalse(self.reader.contexts[offer[0]].injected_pages)

    def test_accepted_native_turn_records_receipt_and_only_changes_next_time(self):
        offer=self.reader.prepare(self.attention,"thread")
        self.reader.accepted(offer)
        self.assertEqual(self.calls[-1][0],"wiki-accept")
        self.assertEqual(self.reader.status,"RECORDED")
        self.snapshot["revision"]=2
        self.snapshot["pages"][0].update(body="new\nunchanged\n",revision=2)
        second=self.reader.prepare(self.attention,"thread")
        self.assertEqual(second[1]["pages"][0]["sections"][0]["inserted"],"new\n")
        self.assertNotIn("body",second[1]["pages"][0])
        self.assertNotIn("unchanged",WikiReader.text(second))

    def test_same_thread_compaction_and_process_restart_rebootstrap(self):
        offer=self.reader.prepare(self.attention,"thread"); self.reader.accepted(offer)
        self.reader.reset("thread")
        new=self.reader.prepare(self.attention,"thread")
        self.assertEqual(new[1]["pages"][0]["kind"],"bootstrap")
        self.assertNotEqual(offer[1]["packet_id"],new[1]["packet_id"])
        restarted=WikiReader("/unused","agent",self.reader.call)
        last=restarted.prepare(self.attention,"thread")
        self.assertNotEqual(last[1]["context_id"],new[1]["context_id"])

    def test_lost_receipt_does_not_falsely_ack_or_strand_newer_snapshot(self):
        call=self.reader.call
        def fail(command,*args):
            if command=="wiki-accept": raise OSError("lost")
            return call(command,*args)
        self.reader.call=fail
        offer=self.reader.prepare(self.attention,"thread"); self.reader.accepted(offer)
        self.assertEqual(self.reader.status,"MODEL_ACCEPTED_RECEIPT_PENDING")
        self.snapshot["revision"]=2; self.snapshot["pages"][0].update(body="fresh",revision=2)
        self.assertIsNotNone(self.reader.prepare(self.attention,"thread"))
        self.assertEqual(len(self.reader.receipts),1)

    def test_wrong_room_and_oversized_snapshot_are_unavailable_not_injected(self):
        self.snapshot["room"]="secret-other-room"
        self.assertIsNone(self.reader.prepare(self.attention,"thread"))
        self.assertEqual(self.reader.status,"UNAVAILABLE")
        self.snapshot["room"]="a"; self.snapshot["pages"][0]["body"]="x"*9000
        self.assertIsNone(self.reader.prepare(self.attention,"thread"))
        self.assertFalse(self.reader.receipts)

    def test_rejected_receipt_is_pending_not_recorded(self):
        call=self.reader.call
        self.reader.call=lambda command,*args: ({"accepted":False} if command=="wiki-accept" else call(command,*args))
        offer=self.reader.prepare(self.attention,"thread")
        self.reader.accepted(offer)
        self.assertEqual(self.reader.status,"MODEL_ACCEPTED_RECEIPT_PENDING")
        self.reader.prepare(self.attention,"thread")
        self.assertEqual(len(self.reader.receipts),1)


if __name__=="__main__": unittest.main()
