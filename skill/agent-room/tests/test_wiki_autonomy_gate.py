"""Counter-tests for the real native gate's server receipt boundary."""
import importlib.util
from pathlib import Path
import unittest

ROOT=Path(__file__).resolve().parents[3]
spec=importlib.util.spec_from_file_location("wiki_autonomy_gate",ROOT/"scripts/wiki-autonomy-e2e.py")
gate=importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


class ServerReceiptTests(unittest.TestCase):
    def receipt(self):
        return dict(attention_id="attention-1",wake="OK",ack="OK",stored=True,
                    accepted=True,first_response=True,final_response=True,
                    turn_completed=True,ready=True,seen_at=100,progress_at=100)

    def test_complete_matching_server_receipt_passes(self):
        gate.require_runtime_receipt(self.receipt(),"attention-1")

    def test_missing_or_foreign_receipt_fails(self):
        for value in (None,{},dict(self.receipt(),attention_id="old-attention")):
            with self.subTest(value=value), self.assertRaises(RuntimeError):
                gate.require_runtime_receipt(value,"attention-1")

    def test_presence_or_partial_lifecycle_is_not_a_pass(self):
        for field in ("stored","accepted","first_response","final_response","turn_completed","ready"):
            with self.subTest(field=field), self.assertRaises(RuntimeError):
                gate.require_runtime_receipt(dict(self.receipt(),**{field:False}),"attention-1")

    def test_pending_ack_and_missing_server_clock_fail(self):
        for overrides in ({"ack":"PENDING"},{"wake":"RUNNING"},{"seen_at":0},{"progress_at":0}):
            with self.subTest(overrides=overrides), self.assertRaises(RuntimeError):
                gate.require_runtime_receipt(dict(self.receipt(),**overrides),"attention-1")


if __name__=="__main__":
    unittest.main()
