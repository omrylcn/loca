import json
import sys
import tempfile
import unittest
from pathlib import Path


SKILL_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SKILL_DIR))

from attention_store import AttentionStore, StaleLease, attention_id_for  # noqa: E402
from codex_adapter_v2 import (  # noqa: E402
    ContextProvider,
    NO_REPLY_SENTINEL,
    PersistentCodexAdapter,
    attention_prompt,
    missing_thread_error,
    mismatched_active_turn,
)


def delivery(
    message_id: int,
    *,
    room: str = "sb-dev",
    text: str = "ping",
    priority: str = "direct_user",
):
    return {
        # listen.py intentionally still emits the v1 delivery envelope during
        # migration. Runtime v2 consumes it into the durable v2 ledger.
        "protocol_version": 1,
        "delivery_id": f"{room}:{message_id}",
        "server": "https://loca.example",
        "room": room,
        "identity": "reviewer",
        "priority": priority,
        "received_at_ms": message_id * 1000,
        "event": {
            "id": message_id,
            "room": room,
            "sender": "operator",
            "sender_type": "user",
            "target": "reviewer",
            "text": text,
        },
    }


def append(inbox: Path, record: dict):
    with inbox.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(record) + "\n")


def control_delivery(message_id: int, command: str):
    record = delivery(message_id, priority="security_control")
    record["event"] = {
        "t": "control",
        "cmd": command,
        "room": "sb-dev",
        "id": message_id,
    }
    return record


def stop_delivery(message_id: int):
    return control_delivery(message_id, "stop")


def room_closed_delivery(message_id: int):
    return control_delivery(message_id, "room-closed")


def care_event(
    *,
    room: str = "sb-dev",
    source_room: str = "",
    subject: str = "reviewer has been waiting 15 minutes for the merge gate",
    reason: str = "wait_timeout",
    context=None,
    signal_id: str = "care-1",
):
    """Build a care delivery event: content lives inside ``signal``, never
    on a top-level ``text`` field (matches the server CareSignal frame)."""
    signal = {
        "id": signal_id,
        "attention_id": f"attention-care-{signal_id}",
        "room": room,
        "reason": reason,
        "subject": subject,
        "context": (
            context
            if context is not None
            else [
                {
                    "id": 41,
                    "sender": "reviewer",
                    "text": "still blocked on the merge gate",
                }
            ]
        ),
    }
    if source_room:
        signal["source_room"] = source_room
    return {"t": "care", "signal": signal}


def care_attention(event: dict, *, room: str = "sb-dev"):
    return {
        "attention_id": "attention:care:1",
        "room": room,
        "priority": "care_signal",
        "reply_required": 0,
        "attempts": 0,
        "event_json": json.dumps(event),
    }


class FakeAppServer:
    def __init__(self, _codex_bin: str, _timeout: float):
        self.pending = []
        self.requests = []
        self.notifications = []
        self.next_thread = 1
        self.next_turn = 1
        self.threads = {}
        self.closed = False
        self.reject_steer = False
        self.reject_steer_error = "turn compacting"
        self.before_steer_response = []

    def request(self, method, params):
        self.requests.append((method, params))
        if method == "initialize":
            return {"result": {}}
        if method == "thread/start":
            thread_id = f"thread-{self.next_thread}"
            self.next_thread += 1
            self.threads[thread_id] = []
            return {
                "result": {
                    "thread": {"id": thread_id, "turns": []}
                }
            }
        if method == "thread/resume":
            thread_id = params["threadId"]
            if thread_id not in self.threads:
                return {"error": {"message": "thread not found"}}
            return {
                "result": {
                    "thread": {
                        "id": thread_id,
                        "turns": list(self.threads[thread_id]),
                    }
                }
            }
        if method == "thread/read":
            thread_id = params["threadId"]
            if thread_id not in self.threads:
                return {"error": {"message": "thread not found"}}
            return {
                "result": {
                    "thread": {
                        "id": thread_id,
                        "status": {"type": "active" if any(
                            t.get("status") == "inProgress" for t in self.threads[thread_id]
                        ) else "idle"},
                        "turns": list(self.threads[thread_id]),
                    }
                }
            }
        if method == "turn/start":
            turn_id = f"turn-{self.next_turn}"
            self.next_turn += 1
            self.threads[params["threadId"]].append(
                {
                    "id": turn_id,
                    "status": "inProgress",
                    "items": [
                        {
                            "id": f"user-{turn_id}",
                            "type": "userMessage",
                            "clientId": params.get("clientUserMessageId"),
                            "content": params["input"],
                        }
                    ],
                }
            )
            return {
                "result": {
                    "turn": {"id": turn_id, "status": "inProgress"}
                }
            }
        if method == "turn/steer":
            if self.reject_steer:
                return {"error": {"message": self.reject_steer_error}}
            thread_id = params["threadId"]
            for turn in self.threads[thread_id]:
                if turn["id"] == params["expectedTurnId"]:
                    turn["items"].append(
                        {
                            "id": f"user-{len(turn['items']) + 1}",
                            "type": "userMessage",
                            "clientId": params.get("clientUserMessageId"),
                            "content": params["input"],
                        }
                    )
            self.pending.extend(self.before_steer_response)
            self.before_steer_response = []
            return {"result": {"turnId": params["expectedTurnId"]}}
        if method == "turn/interrupt":
            return {"result": {}}
        raise AssertionError(f"unexpected method: {method}")

    def send_notification(self, method, params):
        self.notifications.append((method, params))

    def read_message(self, _deadline):
        return self.pending.pop(0) if self.pending else None

    def close(self):
        self.closed = True


class RelayRecorder:
    def __init__(self, fail_first=False):
        self.calls = []
        self.fail_first = fail_first

    def __call__(self, relay):
        self.calls.append(dict(relay))
        if self.fail_first and len(self.calls) == 1:
            raise RuntimeError("temporary 500")


class AdapterFixture:
    def __init__(self, root: Path, *, relay=None, relay_mode="live"):
        self.root = root
        self.inbox = root / "inbox.jsonl"
        self.store = AttentionStore(root / "state.sqlite3")
        self.fake = FakeAppServer("codex", 10)
        self.relay = relay or RelayRecorder()
        self.adapter = PersistentCodexAdapter(
            store=self.store,
            inbox=self.inbox,
            identity="reviewer",
            workdir=root,
            codex_bin="codex",
            relay_mode=relay_mode,
            relay=self.relay,
            context_provider=lambda _attention: [],
            app_server_factory=lambda _bin, _timeout: self.fake,
        )
        self.adapter.initialize()

    def ingest(self, *records):
        for record in records:
            append(self.inbox, record)
        self.store.ingest_inbox(self.inbox, "reviewer")


class CodexAdapterV2Tests(unittest.TestCase):
    def test_plain_lead_room_message_wakes_idle_model_without_forcing_reply(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            record = delivery(400, priority="lead_room", text="lead, did you receive this?")
            record["event"].pop("target")
            fixture.ingest(record)
            self.assertTrue(fixture.adapter.dispatch_one())
            self.assertEqual(sum(m == "turn/start" for m, _ in fixture.fake.requests), 1)
            self.assertEqual(fixture.store.progress_summary("reviewer")["reply_required_pending"], 0)
            fixture.adapter.close()

    def test_plain_lead_message_is_steered_once_and_informational_remains_passive(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(400))
            self.assertTrue(fixture.adapter.dispatch_one())
            record = delivery(401, priority="lead_room", text="new room context")
            record["event"].pop("target")
            fixture.ingest(record, record)
            self.assertTrue(fixture.adapter.dispatch_one())
            self.assertEqual(sum(m == "turn/steer" for m, _ in fixture.fake.requests), 1)
            passive = delivery(402, priority="informational")
            passive["event"].pop("target")
            fixture.ingest(passive)
            self.assertFalse(fixture.adapter.dispatch_one())
            fixture.adapter.close()

    @staticmethod
    def final_item(item_id, text=None, phase="final_answer"):
        return {"id": item_id, "type": "agentMessage", "phase": phase,
                "text": text or item_id}

    def test_final_after_late_direct_is_not_suppressed_by_first_final(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            for mid in range(400, 410):
                fixture.ingest(delivery(mid))
                self.assertTrue(fixture.adapter.dispatch_one())
                fixture.adapter.handle_agent_item("thread-1", "turn-1", self.final_item(f"final-{mid}"))
                fixture.adapter.handle_agent_item("thread-1", "turn-1", self.final_item(f"duplicate-{mid}"))
                fixture.adapter.process_relays()
            fixture.adapter.handle_turn_completed("thread-1", {"id": "turn-1", "status": "completed"})
            self.assertEqual([r["text"] for r in fixture.relay.calls], [f"final-{mid}" for mid in range(400, 410)])
            self.assertEqual(fixture.store.progress_summary("reviewer")["reply_required_pending"], 0)
            self.assertEqual(sum(m == "turn/start" for m, _ in fixture.fake.requests), 1)
            self.assertEqual(sum(m == "turn/steer" for m, _ in fixture.fake.requests), 9)
            self.assertNotIn("turn/interrupt", [m for m, _ in fixture.fake.requests])
            fixture.adapter.close()

    def test_pre_steer_final_cannot_cover_later_accepted_call(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(400)); fixture.adapter.dispatch_one()
            fixture.ingest(delivery(401))
            fixture.fake.before_steer_response = [{"method": "item/completed", "params": {
                "threadId": "thread-1", "turnId": "turn-1", "item": self.final_item("before"),
            }}]
            fixture.adapter.dispatch_one()
            fixture.adapter.handle_agent_item("thread-1", "turn-1", self.final_item("after"))
            fixture.adapter.process_relays()
            self.assertEqual([r["text"] for r in fixture.relay.calls], ["before", "after"])
            self.assertEqual([len(json.loads(r["covered_attention_ids_json"])) for r in fixture.relay.calls], [1, 2])
            fixture.adapter.close()

    def test_replayed_final_preserves_original_coverage_and_late_call_budget(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(400)); fixture.adapter.dispatch_one()
            old = self.final_item("old")
            fixture.adapter.handle_agent_item("thread-1", "turn-1", old)
            fixture.adapter.process_relays()
            fixture.ingest(delivery(401)); fixture.adapter.dispatch_one()
            fixture.adapter.handle_agent_item("thread-1", "turn-1", old)
            fixture.adapter.process_relays()
            self.assertEqual(fixture.store.progress_summary("reviewer")["reply_required_pending"], 1)
            fixture.adapter.handle_agent_item("thread-1", "turn-1", self.final_item("new"))
            fixture.adapter.process_relays()
            self.assertEqual([r["text"] for r in fixture.relay.calls], ["old", "new"])
            self.assertEqual(len(json.loads(fixture.relay.calls[0]["covered_attention_ids_json"])), 1)
            fixture.adapter.close()

    def test_final_budget_survives_retry_restart_and_shadow(self):
        for mode in ("live", "shadow"):
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as tmp:
                fixture = AdapterFixture(Path(tmp), relay=RelayRecorder(fail_first=True), relay_mode=mode)
                fixture.ingest(delivery(400)); fixture.adapter.dispatch_one()
                fixture.adapter.handle_agent_item("thread-1", "turn-1", self.final_item("old"))
                fixture.adapter.process_relays()
                fixture.adapter.close()
                adapter = PersistentCodexAdapter(store=AttentionStore(Path(tmp) / "state.sqlite3"),
                    inbox=fixture.inbox, identity="reviewer", workdir=Path(tmp), codex_bin="codex",
                    relay_mode=mode, relay=fixture.relay, context_provider=lambda _a: [],
                    app_server_factory=lambda _b, _t: fixture.fake)
                adapter.initialize()
                adapter.handle_agent_item("thread-1", "turn-1", self.final_item("progress"))
                fixture.ingest(delivery(401)); adapter.dispatch_one()
                adapter.handle_agent_item("thread-1", "turn-1", self.final_item("new"))
                adapter.handle_agent_item("thread-1", "turn-1", self.final_item("progress-2"))
                with fixture.store.connect() as connection:
                    connection.execute("UPDATE relay_items SET next_attempt_at_ms = 0")
                    finals = connection.execute("SELECT op_id, text FROM relay_items WHERE relay_kind='final' ORDER BY rowid").fetchall()
                self.assertEqual([r["text"] for r in finals], ["old", "new"])
                adapter.process_relays()
                if mode == "live":
                    self.assertEqual([r["text"] for r in fixture.relay.calls], ["old", "old", "new"])
                    self.assertEqual(fixture.relay.calls[0]["op_id"], fixture.relay.calls[1]["op_id"])
                    self.assertEqual(fixture.store.progress_summary("reviewer")["reply_required_pending"], 0)
                else:
                    self.assertEqual(fixture.relay.calls, [])
                    self.assertEqual(fixture.store.progress_summary("reviewer")["reply_required_pending"], 2)
                adapter.close()

    def test_null_phase_after_first_final_covers_new_call(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(400)); fixture.adapter.dispatch_one()
            fixture.adapter.handle_agent_item("thread-1", "turn-1", self.final_item("old"))
            fixture.adapter.process_relays()
            fixture.ingest(delivery(401)); fixture.adapter.dispatch_one()
            fixture.adapter.handle_agent_item("thread-1", "turn-1", self.final_item("new", phase=None))
            fixture.adapter.handle_turn_completed("thread-1", {"id": "turn-1", "status": "completed"})
            fixture.adapter.process_relays()
            self.assertEqual([r["text"] for r in fixture.relay.calls], ["old", "new"])
            self.assertEqual(fixture.store.progress_summary("reviewer")["reply_required_pending"], 0)
            fixture.adapter.close()

    def test_null_phase_before_steer_never_claims_later_call(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(400)); fixture.adapter.dispatch_one()
            fixture.adapter.handle_agent_item("thread-1", "turn-1", self.final_item("old", phase=None))
            fixture.ingest(delivery(401)); fixture.adapter.dispatch_one()
            fixture.adapter.handle_turn_completed("thread-1", {"id": "turn-1", "status": "completed"})
            fixture.adapter.process_relays()
            self.assertEqual(len(fixture.relay.calls), 1)
            self.assertEqual(len(json.loads(fixture.relay.calls[0]["covered_attention_ids_json"])), 1)
            self.assertEqual(fixture.store.progress_summary("reviewer")["reply_required_pending"], 1)
            fixture.adapter.close()

    def test_pending_direct_cannot_open_final_budget_before_acceptance(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(400)); fixture.adapter.dispatch_one()
            fixture.adapter.handle_agent_item("thread-1", "turn-1", self.final_item("old"))
            fixture.ingest(delivery(401))
            fixture.adapter.handle_agent_item("thread-1", "turn-1", self.final_item("progress"))
            fixture.adapter.process_relays()
            self.assertEqual([r["text"] for r in fixture.relay.calls], ["old"])
            self.assertEqual(fixture.store.progress_summary("reviewer")["reply_required_pending"], 1)
            fixture.adapter.close()

    def test_legacy_suppressed_final_is_not_republished_or_falsely_acked_on_restart(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(400)); fixture.adapter.dispatch_one()
            fixture.adapter.handle_agent_item("thread-1", "turn-1", self.final_item("old"))
            fixture.adapter.process_relays()
            fixture.ingest(delivery(401)); fixture.adapter.dispatch_one()
            fixture.adapter.create_output(turn_id="turn-1", item_id="legacy-hidden",
                phase="final_answer", text="Historical answer", relay_kind="final_suppressed", status="suppressed")
            fixture.adapter.handle_turn_completed("thread-1", {"id": "turn-1", "status": "completed"})
            fixture.adapter.close()
            adapter = PersistentCodexAdapter(store=AttentionStore(Path(tmp) / "state.sqlite3"),
                inbox=fixture.inbox, identity="reviewer", workdir=Path(tmp), codex_bin="codex",
                relay_mode="live", relay=fixture.relay, context_provider=lambda _a: [],
                app_server_factory=lambda _b, _t: fixture.fake)
            adapter.initialize(); adapter.process_relays()
            self.assertEqual([r["text"] for r in fixture.relay.calls], ["old"])
            self.assertEqual(adapter.health["reply_required_pending"], 1)
            self.assertIsNone(fixture.store.snapshot("reviewer")["attentions"][-1]["final_response_at_ms"])
            adapter.close()

    def test_pending_audit_is_read_only_and_distinguishes_suppression_from_model_silence(self):
        import importlib.util
        script = SKILL_DIR.parents[1] / "scripts/audit-pending-replies.py"
        spec = importlib.util.spec_from_file_location("pending_reply_audit", script)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(400)); fixture.adapter.dispatch_one()
            fixture.adapter.create_output(turn_id="turn-1", item_id="legacy-hidden",
                phase="final_answer", text="PRIVATE_ROOM_TEXT", relay_kind="final_suppressed", status="suppressed")
            fixture.adapter.handle_turn_completed("thread-1", {"id": "turn-1", "status": "completed"})
            fixture.ingest(delivery(401)); fixture.adapter.dispatch_one()
            fixture.adapter.handle_agent_item("thread-1", "turn-2", self.final_item("silent", NO_REPLY_SENTINEL))
            fixture.adapter.handle_turn_completed("thread-1", {"id": "turn-2", "status": "completed"})
            fixture.ingest(delivery(402))
            before = fixture.store.snapshot("reviewer")
            report = module.audit(fixture.store.path, ["reviewer"], limit=1)
            self.assertEqual(report["identities"]["reviewer"]["reasons"], {
                "final_suppressed": 1, "required_no_reply": 1, "not_accepted": 1})
            self.assertTrue(report["identities"]["reviewer"]["records_truncated"])
            self.assertFalse(report["healthy"])
            self.assertNotIn("PRIVATE_ROOM_TEXT", json.dumps(report))
            self.assertEqual(before, fixture.store.snapshot("reviewer"))
            fixture.adapter.close()

    def test_wiki_context_is_accepted_only_with_native_turn_and_compaction_rebootstraps(self):
        from wiki_reader import WikiReader
        with tempfile.TemporaryDirectory() as directory:
            fixture=AdapterFixture(Path(directory))
            calls=[]
            snapshot={"room":"sb-dev","revision":1,"selection_version":0,"pages":[
                {"slug":"overview","title":"Overview","body":"database choice","sources":[1],"revision":1}]}
            def call(command,_server,_room,payload=None):
                calls.append(command)
                return snapshot if command=="wiki-context" else {"accepted":True}
            class Provider:
                wiki_reader=WikiReader("/unused","reviewer",call)
                def __call__(self,_attention): return []
            provider=Provider(); fixture.adapter.context_provider=provider
            fixture.ingest(delivery(1)); fixture.adapter.dispatch_one()
            self.assertEqual(calls,["wiki-context","wiki-accept"])
            inputs=[params["input"] for method,params in fixture.fake.requests if method=="turn/start"][-1]
            self.assertIn("database choice",inputs[1]["text"])
            fixture.ingest(delivery(2)); fixture.adapter.dispatch_one()
            self.assertEqual(calls,["wiki-context","wiki-accept"])
            fixture.adapter.handle_message({"method":"item/completed","params":{"threadId":"thread-1",
                "turnId":"turn-1","item":{"type":"contextCompaction","id":"compact-1"}}})
            fixture.adapter.active_turns.clear()
            fixture.ingest(delivery(3)); fixture.adapter.dispatch_one()
            inputs=[params["input"] for method,params in fixture.fake.requests if method=="turn/start"][-1]
            self.assertIn('"kind": "bootstrap"',inputs[1]["text"])
            fixture.adapter.close()

    def test_rejected_native_turn_does_not_ack_wiki_offer(self):
        from wiki_reader import WikiReader
        with tempfile.TemporaryDirectory() as directory:
            fixture=AdapterFixture(Path(directory)); calls=[]
            def call(command,*_args):
                calls.append(command)
                return {"room":"sb-dev","revision":1,"selection_version":0,"pages":[
                    {"slug":"overview","title":"Overview","body":"policy","sources":[1],"revision":1}]}
            class Provider:
                wiki_reader=WikiReader("/unused","reviewer",call)
                def __call__(self,_attention): return []
            fixture.adapter.context_provider=Provider()
            original=fixture.fake.request
            fixture.fake.request=lambda method,params: {"error":{"message":"unavailable"}} if method=="turn/start" else original(method,params)
            fixture.ingest(delivery(1)); self.assertFalse(fixture.adapter.dispatch_one())
            self.assertEqual(calls,["wiki-context"])
            self.assertFalse(next(iter(fixture.adapter.context_provider.wiki_reader.contexts.values())).injected_pages)
            fixture.adapter.close()

    def test_archived_snapshot_never_consumes_the_prompt_budget(self):
        base = {
            "status": "ready",
            "owner": "İye sahibi",
            "short_updated_at": 10,
            "long_updated_at": 20,
            "version": 3,
            "long_truncated": False,
            "long_omitted_bytes": 0,
            "long_omitted_entries": 0,
            "long_uninjectable_entries": 0,
            "long_uninjectable_entry_ids": [],
        }
        fixtures = {
            "turkish": ("ö" * (4 * 1024 // 2), "ğ" * (8 * 1024 // 2)),
            "ascii": ("s" * (4 * 1024), "l" * (8 * 1024)),
            "multiline": (
                ("short-line\n" * 400)[: 4 * 1024],
                ("long karar\n\n" * 700)[: 8 * 1024],
            ),
            "escaping_ceiling": ("\n" * (4 * 1024), "\n" * (8 * 1024)),
        }
        serialized_sizes = []
        for name, (short, long) in fixtures.items():
            memory = dict(base, short=short, long=long)
            event = dict(delivery(7)["event"], memory=memory)
            attention = {"attention_id": "a:7", "room": "sb-dev",
                         "priority": "direct_user", "reply_required": True,
                         "attempts": 0, "event_json": json.dumps(event)}
            rendered = attention_prompt("reviewer", attention, [])
            with self.subTest(name=name):
                self.assertNotIn("İye sahibi", rendered)
                self.assertIn("Loca Wiki is the only room knowledge surface", rendered)
                self.assertIn("ping", rendered)
                self.assertNotIn('"owner"', rendered)
                self.assertEqual(event["memory"], memory)
                serialized_sizes.append(len(rendered.encode("utf-8")))
        self.assertEqual(len(set(serialized_sizes)), 1)

    def test_attention_prompt_teaches_wiki_not_a_second_knowledge_role(self):
        event = {
            "id": 7,
            "sender": "operator",
            "sender_type": "user",
            "text": "wake",
            "memory_trigger": "turn",
            "memory": {
                "status": "ready",
                "short": "fact",
                "long": "decision",
                "version": 3,
                "long_omitted_entries": 1,
                "long_uninjectable_entries": 1,
            },
        }
        attention = {
            "attention_id": "attention:1",
            "room": "sb-dev",
            "priority": "direct_user",
            "reply_required": True,
            "attempts": 0,
            "event_json": json.dumps(event),
        }
        prompt = attention_prompt("reviewer", attention, [])
        self.assertNotIn("Loca memory snapshot", prompt)
        self.assertNotIn('"freshness"', prompt)
        self.assertNotIn("\nfact", prompt)
        self.assertNotIn("\ndecision", prompt)
        self.assertIn("only its assigned Wiki editor", prompt)
        self.assertIn("wake", prompt)

    def test_old_ledger_metadata_is_retired_without_dispatching_or_losing_direct(self):
        with tempfile.TemporaryDirectory() as tmp:
            case = AdapterFixture(Path(tmp))
            case.ingest(delivery(1, priority="addressed_agent"), delivery(2))
            old_id = attention_id_for("reviewer", "https://loca.example", "sb-dev:1")
            # Simulate a SQLite record written before the Wiki-only boundary.
            archive = json.dumps({"t": "memory", "owner": None, "short": "old role"})
            with case.store.connect() as conn:
                conn.execute("UPDATE attentions SET event_json = ?, priority_rank = 0 "
                             "WHERE attention_id = ?", (archive, old_id))
            self.assertTrue(case.adapter.dispatch_one())
            with case.store.connect() as conn:
                old = dict(conn.execute("SELECT * FROM attentions WHERE attention_id = ?",
                                       (old_id,)).fetchone())
            self.assertEqual(old["terminal_status"], "cancelled")
            self.assertIsNone(old["accepted_at_ms"])
            self.assertEqual(old["event_json"], archive)
            self.assertFalse(any(method.startswith("turn/") for method, _ in case.fake.requests))
            self.assertTrue(case.adapter.dispatch_one())
            with case.store.connect() as conn:
                direct = conn.execute("SELECT * FROM attentions WHERE delivery_id = 'sb-dev:2'").fetchone()
            self.assertIsNotNone(direct["accepted_at_ms"])
            case.adapter.close()

    def test_retirement_is_fenced_and_never_cancels_tasks_or_accepted_work(self):
        with tempfile.TemporaryDirectory() as tmp:
            case = AdapterFixture(Path(tmp))
            case.ingest(delivery(1, priority="explicit_task"), delivery(2))
            task_id = attention_id_for("reviewer", "https://loca.example", "sb-dev:1")
            direct_id = attention_id_for("reviewer", "https://loca.example", "sb-dev:2")
            args = ("reviewer", case.adapter.owner, case.adapter.epoch)
            with case.store.connect() as conn:
                conn.execute("UPDATE attentions SET event_json = ? WHERE attention_id = ?",
                             (json.dumps({"t": "memory"}), task_id))
            self.assertFalse(case.store.retire_legacy_context(task_id, *args))
            self.assertFalse(case.store.retire_legacy_context(direct_id, *args))
            with self.assertRaises(StaleLease):
                case.store.retire_legacy_context(task_id, "reviewer", "wrong-owner", case.adapter.epoch)
            with case.store.connect() as conn:
                conn.execute("UPDATE attentions SET priority = 'addressed_agent', accepted_at_ms = 1 "
                             "WHERE attention_id = ?", (task_id,))
            self.assertFalse(case.store.retire_legacy_context(task_id, *args))
            self.assertEqual(case.store.next_pending("reviewer")["attention_id"], direct_id)
            case.adapter.close()

    def test_reconciliation_health_marks_current_epoch_complete(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            health = fixture.adapter.health
            self.assertEqual(health["reconciliation"], "OK")
            self.assertEqual(
                health["reconciliation_epoch"], fixture.adapter.epoch
            )
            self.assertEqual(health["reconciliation_reaped_turns"], 0)
            fixture.adapter.close()

    def test_reconciliation_failure_never_reports_ok(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            store = AttentionStore(root / "state.sqlite3")
            seed = PersistentCodexAdapter(
                store=store, inbox=root / "inbox.jsonl", identity="reviewer",
                workdir=root, codex_bin="codex", relay_mode="live",
                relay=RelayRecorder(), context_provider=lambda _attention: [],
                app_server_factory=FakeAppServer,
            )
            seed.initialize()
            store.set_thread(
                "reviewer", "https://loca.example", "sb-dev", "thread-1",
                seed.owner, seed.epoch,
            )
            seed.close()

            class BrokenResume(FakeAppServer):
                def request(self, method, params):
                    if method == "thread/resume":
                        return {"error": {"message": "temporary recovery failure"}}
                    return super().request(method, params)

            restarted = PersistentCodexAdapter(
                store=AttentionStore(root / "state.sqlite3"),
                inbox=root / "inbox.jsonl", identity="reviewer",
                workdir=root, codex_bin="codex", relay_mode="live",
                relay=RelayRecorder(), context_provider=lambda _attention: [],
                app_server_factory=BrokenResume,
            )
            with self.assertRaisesRegex(RuntimeError, "recovery failure"):
                restarted.initialize()
            self.assertEqual(restarted.health["reconciliation"], "RUNNING")
            self.assertNotEqual(restarted.health.get("reconciliation"), "OK")
            restarted.close()

    def test_mismatch_hint_is_exact_and_scoped_to_expected_turn(self):
        self.assertEqual(mismatched_active_turn(
            "expected active turn id `old` but found `live`", "old"), "live")
        for error in (
            "expected active turn id `other` but found `live`",
            "expected active turn id `old` but found `old`",
            "expected active turn id `old` but found `live` extra text",
            "no active turn to steer",
        ):
            self.assertIsNone(mismatched_active_turn(error, "old"))

    def test_restart_keeps_accepted_work_when_live_history_lags(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(919))
            fixture.adapter.dispatch_one()
            with fixture.store.connect() as connection:
                connection.execute("UPDATE turns SET started_at_ms=started_at_ms-61000")
            original = fixture.fake.request
            def lagging(method, params):
                if method == "thread/read":
                    return {"result": {"thread": {
                        "id": "thread-1", "turns": [], "status": {"type": "active"}}}}
                return original(method, params)
            fixture.fake.request = lagging
            fixture.adapter.active_turns.clear()
            fixture.adapter.recover_existing_threads()
            row = fixture.store.snapshot("reviewer")["attentions"][0]
            self.assertIsNotNone(row["accepted_at_ms"])
            self.assertEqual(row["turn_id"], "turn-1")
            self.assertEqual(fixture.adapter.active_turns["thread-1"], "turn-1")
            self.assertIsNone(fixture.store.next_pending("reviewer"))
            fixture.adapter.close()

    def test_history_lag_never_replays_live_turn_and_ten_calls_are_steered(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(920, priority="care_signal"))
            fixture.adapter.dispatch_one()
            original = fixture.fake.request
            def lagging_history(method, params):
                if method == "thread/read":
                    return {"result": {"thread": {
                        "id": "thread-1", "status": {"type": "active", "activeFlags": []},
                        "turns": [],
                    }}}
                return original(method, params)
            fixture.fake.request = lagging_history
            for message_id in range(921, 931):
                fixture.ingest(delivery(message_id))
                fixture.adapter.next_turn_reconcile_ms = 0
                fixture.adapter.cycle()
            rows = fixture.store.snapshot("reviewer")["attentions"]
            self.assertEqual(len(rows), 11)
            self.assertTrue(all(row["turn_id"] == "turn-1" for row in rows))
            self.assertTrue(all(row["accepted_at_ms"] is not None for row in rows))
            self.assertTrue(all(row["attempts"] == 1 for row in rows))
            self.assertEqual(sum(m == "turn/start" for m, _ in fixture.fake.requests), 1)
            self.assertEqual(sum(m == "turn/steer" for m, _ in fixture.fake.requests), 10)
            fixture.adapter.close()

    def test_active_id_mismatch_retries_same_attention_on_reported_turn(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(940))
            fixture.adapter.dispatch_one()
            fixture.adapter.active_turns["thread-1"] = "stale-turn"
            original = fixture.fake.request
            def strict_steer(method, params):
                if method == "turn/steer" and params["expectedTurnId"] != "turn-1":
                    return {"error": {"message":
                        "expected active turn id `stale-turn` but found `turn-1`"}}
                return original(method, params)
            fixture.fake.request = strict_steer
            fixture.ingest(delivery(941))
            self.assertTrue(fixture.adapter.dispatch_one())
            row = fixture.store.snapshot("reviewer")["attentions"][-1]
            self.assertEqual(row["turn_id"], "turn-1")
            self.assertEqual(row["attempts"], 1)
            self.assertEqual(fixture.adapter.active_turns["thread-1"], "turn-1")
            methods = [m for m, _ in fixture.fake.requests]
            self.assertEqual(methods.count("turn/start"), 1)
            self.assertNotIn("turn/interrupt", methods)
            fixture.adapter.close()

    def test_unknown_live_status_does_not_authorize_missing_turn_replay(self):
        for status in (None, {"type": "notLoaded"}, {"type": "systemError"}):
            with self.subTest(status=status), tempfile.TemporaryDirectory() as tmp:
                fixture = AdapterFixture(Path(tmp))
                fixture.ingest(delivery(950))
                fixture.adapter.dispatch_one()
                original = fixture.fake.request
                def unknown(method, params):
                    if method == "thread/read":
                        return {"result": {"thread": {
                            "id": "thread-1", "status": status, "turns": []}}}
                    return original(method, params)
                fixture.fake.request = unknown
                self.assertFalse(fixture.adapter.reconcile_active_turn("thread-1", "turn-1"))
                row = fixture.store.snapshot("reviewer")["attentions"][0]
                self.assertIsNotNone(row["accepted_at_ms"])
                self.assertEqual(row["turn_id"], "turn-1")
                fixture.adapter.close()

    def test_recovery_does_not_reprocess_thousands_of_completed_turns(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp), relay_mode="shadow")
            fixture.ingest(delivery(899))
            fixture.adapter.dispatch_one()
            old = [{"id": f"old-{i}", "status": "completed", "items": []} for i in range(5000)]
            fixture.fake.threads["thread-1"] = old + fixture.fake.threads["thread-1"]
            recovered = []
            recover = fixture.adapter.recover_turn
            def record(turn_id, thread_id, turn):
                recovered.append(turn_id)
                return recover(turn_id, thread_id, turn)
            fixture.adapter.recover_turn = record
            fixture.adapter.recover_existing_threads()
            self.assertEqual(recovered, ["turn-1"])
            fixture.adapter.close()

    def test_paginated_crash_reconciliation_preserves_accepted_dispatch(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp), relay_mode="shadow")
            fixture.ingest(delivery(900))
            finalize = fixture.store.finalize_dispatch
            def crash(*args, **kwargs):
                raise RuntimeError("accepted before crash")
            fixture.store.finalize_dispatch = crash
            with self.assertRaisesRegex(RuntimeError, "accepted before crash"):
                fixture.adapter.dispatch_one()
            fixture.store.finalize_dispatch = finalize
            request = fixture.fake.request
            pages = []
            def paginated(method, params):
                if method == "thread/read":
                    return {"error": {"message": "paginated threads do not support thread/read(includeTurns=true)"}}
                if method == "thread/turns/list":
                    pages.append(params)
                    if not params.get("cursor"):
                        return {"result": {"data": [{"id": "older", "status": "completed", "itemsView": "full", "items": []}], "nextCursor": "page2"}}
                    turns = [dict(t, itemsView="full") for t in fixture.fake.threads[params["threadId"]]]
                    return {"result": {"data": turns, "nextCursor": None}}
                return request(method, params)
            fixture.fake.request = paginated
            fixture.adapter.recover_existing_threads()
            self.assertIsNone(fixture.store.next_pending("reviewer"))
            self.assertEqual(len(pages), 2)
            self.assertTrue(all(p["itemsView"] == "full" for p in pages))
            self.assertEqual(sum(m == "turn/start" for m, _ in fixture.fake.requests), 1)
            fixture.adapter.close()

    def test_paginated_incomplete_history_never_authorizes_replay(self):
        bad_pages = [
            {"error": {"message": "paginated_threads is not supported yet"}},
            {"result": {"data": [{"id": "turn", "itemsView": "summary", "items": []}], "nextCursor": None}},
            {"result": {"data": [], "nextCursor": "repeated"}},
        ]
        for page in bad_pages:
            with self.subTest(page=page), tempfile.TemporaryDirectory() as tmp:
                fixture = AdapterFixture(Path(tmp), relay_mode="shadow")
                fixture.ingest(delivery(901))
                original = fixture.fake.request
                def request(method, params):
                    if method == "thread/read":
                        return {"error": {"message": "paginated threads do not support thread/read(includeTurns=true)"}}
                    if method == "thread/turns/list":
                        return page
                    return original(method, params)
                fixture.fake.request = request
                with self.assertRaises(RuntimeError):
                    fixture.adapter.authoritative_turns("thread", {"turns": []})
                self.assertIsNotNone(fixture.store.next_pending("reviewer"))
                self.assertFalse(any(m == "turn/start" for m, _ in fixture.fake.requests))
                fixture.adapter.close()

    def test_resumed_thread_reapplies_sandbox_and_turn_policy(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inbox = root / "inbox.jsonl"
            store = AttentionStore(root / "state.sqlite3")
            owner = "old-adapter"
            epoch = store.claim_lease("reviewer", owner, 10_000)
            store.set_thread(
                "reviewer",
                "https://loca.example",
                "sb-dev",
                "thread-old",
                owner,
                epoch,
            )
            store.release_lease("reviewer", owner, epoch)

            fake = FakeAppServer("codex", 10)
            fake.threads["thread-old"] = []
            adapter = PersistentCodexAdapter(
                store=store,
                inbox=inbox,
                identity="reviewer",
                workdir=root,
                codex_bin="codex",
                relay_mode="shadow",
                relay=RelayRecorder(),
                context_provider=lambda _attention: [],
                sandbox="danger-full-access",
                app_server_factory=lambda _bin, _timeout: fake,
            )
            adapter.initialize()

            resume = next(
                params
                for method, params in fake.requests
                if method == "thread/resume"
            )
            self.assertEqual(resume["sandbox"], "danger-full-access")
            self.assertEqual(resume["cwd"], str(root))
            self.assertEqual(resume["approvalPolicy"], "never")
            self.assertIn(
                "dedicated headless Loca runtime",
                resume["developerInstructions"],
            )

            append(inbox, delivery(1))
            store.ingest_inbox(inbox, "reviewer")
            self.assertTrue(adapter.dispatch_one())
            turn = next(
                params
                for method, params in fake.requests
                if method == "turn/start"
            )
            self.assertEqual(
                turn["sandboxPolicy"], {"type": "dangerFullAccess"}
            )
            adapter.close()

    def test_goal_reminder_context_includes_bounded_goal_lead_and_completion(self):
        with tempfile.TemporaryDirectory() as tmp:
            script = Path(tmp) / "connect.sh"
            calls = Path(tmp) / "calls"
            script.write_text(
                "#!/bin/sh\n"
                f"printf '%s\\n' \"$1\" >> '{calls}'\n"
                "if [ \"$1\" = since ]; then\n"
                "  printf '%s\\n' '[{\"id\":7,\"sender\":\"operator\",\"text\":\"ship it\"}]'\n"
                "elif [ \"$1\" = goals ]; then\n"
                "  printf '%s\\n' '[{\"id\":1,\"status\":\"active\",\"outcome\":\"public release is safe\",\"checkpoint\":\"review is green\",\"completion\":\"manual\"}]'\n"
                "elif [ \"$1\" = settings ]; then\n"
                "  printf '%s\\n' '{\"lead\":\"release-lead\"}'\n"
                "fi\n",
                encoding="utf-8",
            )
            script.chmod(0o700)
            provider = ContextProvider(script)
            context = provider(
                {
                    "server": "https://loca.example",
                    "room": "sb-dev",
                    "context_from_id": 7,
                    "context_to_id": 7,
                    "attention_id": "attention:goal:1",
                    "event_json": json.dumps(
                        care_event(reason="goal_reminder", subject="Goal: public release is safe")
                    ),
                }
            )

            self.assertEqual(context[0]["text"], "ship it")
            self.assertEqual(context[-1]["sender"], "loca-goal")
            self.assertIn("public release is safe", context[-1]["text"])
            self.assertIn("@release-lead", context[-1]["text"])
            self.assertIn("review is green", context[-1]["text"])
            prompt = attention_prompt(
                "reviewer",
                care_attention(
                    care_event(reason="goal_reminder", subject="Goal: public release is safe")
                ),
                context,
            )
            self.assertIn("Goal context:", prompt)
            self.assertIn("@release-lead", prompt)
            self.assertEqual(calls.read_text(encoding="utf-8").splitlines(), ["since", "goals", "settings"])

    def test_non_goal_turn_does_not_fetch_or_inject_goal_context(self):
        with tempfile.TemporaryDirectory() as tmp:
            script = Path(tmp) / "connect.sh"
            calls = Path(tmp) / "calls"
            script.write_text(
                "#!/bin/sh\n"
                f"printf '%s\\n' \"$1\" >> '{calls}'\n"
                "if [ \"$1\" = since ]; then\n"
                "  printf '%s\\n' '[{\"id\":7,\"sender\":\"operator\",\"text\":\"ship it\"}]'\n"
                "fi\n",
                encoding="utf-8",
            )
            script.chmod(0o700)
            attention = {
                "server": "https://loca.example",
                "room": "sb-dev",
                "context_from_id": 7,
                "context_to_id": 7,
                "attention_id": "attention:normal:1",
                "event_json": json.dumps(care_event(reason="task_reminder")),
            }
            context = ContextProvider(script)(attention)
            self.assertEqual([row["text"] for row in context], ["ship it"])
            self.assertEqual(calls.read_text(encoding="utf-8").splitlines(), ["since"])

    def test_goal_reminder_without_active_goal_keeps_context_unchanged(self):
        with tempfile.TemporaryDirectory() as tmp:
            script = Path(tmp) / "connect.sh"
            calls = Path(tmp) / "calls"
            script.write_text(
                "#!/bin/sh\n"
                f"printf '%s\\n' \"$1\" >> '{calls}'\n"
                "if [ \"$1\" = since ]; then printf '%s\\n' '[{\"id\":7,\"sender\":\"operator\",\"text\":\"ship it\"}]';\n"
                "elif [ \"$1\" = goals ]; then printf '%s\\n' '[]'; fi\n",
                encoding="utf-8",
            )
            script.chmod(0o700)
            context = ContextProvider(script)(
                {
                    "server": "https://loca.example",
                    "room": "sb-dev",
                    "context_from_id": 7,
                    "context_to_id": 7,
                    "attention_id": "attention:goal:none",
                    "event_json": json.dumps(care_event(reason="goal_reminder")),
                }
            )
            self.assertEqual([row["text"] for row in context], ["ship it"])
            self.assertEqual(calls.read_text(encoding="utf-8").splitlines(), ["since", "goals"])

    def test_goal_context_is_capped(self):
        with tempfile.TemporaryDirectory() as tmp:
            script = Path(tmp) / "connect.sh"
            script.write_text(
                "#!/bin/sh\n"
                "if [ \"$1\" = since ]; then printf '%s\\n' '[]';\n"
                "elif [ \"$1\" = goals ]; then printf '[{\"id\":1,\"status\":\"active\",\"outcome\":\"'; head -c 2000 /dev/zero | tr '\\0' x; printf '\",\"completion\":\"all_tasks\",\"task_ids\":[1,2]}]\\n';\n"
                "elif [ \"$1\" = settings ]; then printf '%s\\n' '{\"lead\":\"lead\"}'; fi\n",
                encoding="utf-8",
            )
            script.chmod(0o700)
            context = ContextProvider(script)(
                {
                    "server": "https://loca.example",
                    "room": "sb-dev",
                    "attention_id": "attention:goal:bounded",
                    "event_json": json.dumps(care_event(reason="goal_reminder")),
                }
            )
            goal_text = next(row["text"] for row in context if row["sender"] == "loca-goal")
            self.assertLessEqual(len(goal_text), 640)
            self.assertIn("…", goal_text)
            self.assertIn("@lead", goal_text)
            self.assertIn("all 2 linked tasks finished", goal_text)

    def test_codex_no_rollout_error_is_a_missing_thread(self):
        self.assertTrue(
            missing_thread_error(
                "no rollout found for thread id 01a00288-dead-beef"
            )
        )

    def test_replayed_attention_warns_against_duplicate_side_effects(self):
        attention = {
            "attention_id": "attention:1",
            "room": "sb-dev",
            "priority": "direct_user",
            "reply_required": 1,
            "attempts": 1,
            "event_json": json.dumps(delivery(1)["event"]),
        }
        prompt = attention_prompt("reviewer", attention, [])
        self.assertIn("do not repeat an effect", prompt)

    def test_care_delivery_prompt_surfaces_signal_subject_reason_and_context(self):
        # RED before the fix: a care frame carries its content inside
        # event.signal (subject/reason/source_room/context) and has no
        # top-level "text". The old attention_prompt only reads message["text"]
        # and the id-range context, so it renders "(no text context)" and the
        # model wakes with a contentless care attention -> LOCA_NO_REPLY.
        event = care_event(
            room="sb-dev",
            source_room="iye",
            subject="reviewer has been waiting 15 minutes for the merge gate",
            reason="wait_timeout",
            context=[
                {
                    "id": 41,
                    "sender": "reviewer",
                    "text": "still blocked on the merge gate",
                }
            ],
        )
        attention = care_attention(event, room="sb-dev")

        # Empty id-range context, exactly like the bug: the only content is the
        # embedded signal.
        prompt = attention_prompt("reviewer", attention, [])

        # These four assertions FAIL on current code (the prompt shows none of
        # the care content) and PASS after attention_prompt renders the signal.
        self.assertIn(
            "reviewer has been waiting 15 minutes for the merge gate", prompt
        )
        self.assertIn("wait_timeout", prompt)
        self.assertIn("sb-dev", prompt)  # delivery room
        self.assertIn("still blocked on the merge gate", prompt)  # context message
        self.assertIn("resolve this stable Care signal id", prompt)
        self.assertIn("connect.sh attention-resolve", prompt)
        self.assertIn(str(event["signal"]["attention_id"]), prompt)
        # source_room is present and non-empty -> must be surfaced.
        self.assertIn("iye", prompt)
        # The old contentless rendering must be gone.
        self.assertNotIn("(no text context)", prompt)

    def test_care_prompt_omits_empty_source_room(self):
        event = care_event(source_room="")
        prompt = attention_prompt("reviewer", care_attention(event), [])
        self.assertIn("Care signal:", prompt)
        self.assertNotIn("Source room:", prompt)

    def test_care_context_does_not_double_render_overlapping_message(self):
        # signal.context and the id-range ContextProvider can return the same
        # room message; it must be rendered once, within MAX_CONTEXT_MESSAGES.
        shared = {
            "id": 41,
            "sender": "reviewer",
            "text": "still blocked on the merge gate",
        }
        event = care_event(context=[dict(shared)])
        attention = care_attention(event)
        prompt = attention_prompt("reviewer", attention, [dict(shared)])
        self.assertEqual(prompt.count("still blocked on the merge gate"), 1)

    def test_normal_message_prompt_is_unchanged_and_not_treated_as_care(self):
        # A non-care delivery must render exactly as before: the bounded room
        # context section, no care-specific framing.
        attention = {
            "attention_id": "attention:1",
            "room": "sb-dev",
            "priority": "direct_user",
            "reply_required": 1,
            "attempts": 0,
            "event_json": json.dumps(
                delivery(1, text="please review the diff")["event"]
            ),
        }
        prompt = attention_prompt("reviewer", attention, [])
        self.assertIn("Bounded room context:", prompt)
        self.assertIn("- operator: please review the diff", prompt)
        self.assertNotIn("Care signal:", prompt)
        self.assertNotIn("Source room:", prompt)

    def test_persistent_server_starts_once_and_second_direct_steers(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(101), delivery(102))

            self.assertTrue(fixture.adapter.dispatch_one())
            self.assertTrue(fixture.adapter.dispatch_one())

            methods = [method for method, _ in fixture.fake.requests]
            self.assertEqual(methods.count("initialize"), 1)
            self.assertEqual(methods.count("thread/start"), 1)
            self.assertEqual(methods.count("turn/start"), 1)
            self.assertEqual(methods.count("turn/steer"), 1)
            self.assertNotIn("turn/interrupt", methods)
            attentions = fixture.store.snapshot("reviewer")["attentions"]
            self.assertEqual(
                [attention["turn_id"] for attention in attentions],
                ["turn-1", "turn-1"],
            )
            fixture.adapter.close()

    def test_ten_direct_messages_remain_ordered_and_one_final_covers_all(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(*(delivery(mid) for mid in range(210, 220)))

            for _ in range(10):
                self.assertTrue(fixture.adapter.dispatch_one())

            fixture.fake.pending.extend(
                [
                    {
                        "method": "item/completed",
                        "params": {
                            "threadId": "thread-1",
                            "turnId": "turn-1",
                            "item": {
                                "id": "final-ten",
                                "type": "agentMessage",
                                "phase": "final_answer",
                                "text": "On çağrı birlikte işlendi.",
                            },
                        },
                    },
                    {
                        "method": "turn/completed",
                        "params": {
                            "threadId": "thread-1",
                            "turn": {"id": "turn-1", "status": "completed"},
                        },
                    },
                ]
            )
            fixture.adapter.drain_messages()
            fixture.adapter.process_relays()

            methods = [method for method, _ in fixture.fake.requests]
            self.assertEqual(methods.count("turn/start"), 1)
            self.assertEqual(methods.count("turn/steer"), 9)
            self.assertNotIn("turn/interrupt", methods)
            self.assertEqual(len(fixture.relay.calls), 1)
            self.assertEqual(
                json.loads(
                    fixture.relay.calls[0]["covered_attention_ids_json"]
                ),
                [
                    attention_id_for(
                        "reviewer", "https://loca.example", f"sb-dev:{mid}"
                    )
                    for mid in range(210, 220)
                ],
            )
            fixture.adapter.close()

    def test_private_rooms_never_share_thread_or_turn(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(110, room="sb-dev"), delivery(111, room="iye"))

            self.assertTrue(fixture.adapter.dispatch_one())
            self.assertTrue(fixture.adapter.dispatch_one())

            methods = [method for method, _ in fixture.fake.requests]
            self.assertEqual(methods.count("thread/start"), 2)
            self.assertEqual(methods.count("turn/start"), 2)
            self.assertEqual(methods.count("turn/steer"), 0)
            attentions = fixture.store.snapshot("reviewer")["attentions"]
            self.assertNotEqual(attentions[0]["turn_id"], attentions[1]["turn_id"])
            fixture.adapter.close()

    def test_first_commentary_and_final_relay_without_duplicate_commentary(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(120))
            fixture.adapter.dispatch_one()
            fixture.fake.pending.extend(
                [
                    {
                        "method": "item/completed",
                        "params": {
                            "threadId": "thread-1",
                            "turnId": "turn-1",
                            "item": {
                                "id": "comment-1",
                                "type": "agentMessage",
                                "phase": "commentary",
                                "text": "Aldım, inceliyorum.",
                            },
                        },
                    },
                    {
                        "method": "item/completed",
                        "params": {
                            "threadId": "thread-1",
                            "turnId": "turn-1",
                            "item": {
                                "id": "comment-2",
                                "type": "agentMessage",
                                "phase": "commentary",
                                "text": "Test koşuyor.",
                            },
                        },
                    },
                    {
                        "method": "item/completed",
                        "params": {
                            "threadId": "thread-1",
                            "turnId": "turn-1",
                            "item": {
                                "id": "final-1",
                                "type": "agentMessage",
                                "phase": "final_answer",
                                "text": "Tamamlandı.",
                            },
                        },
                    },
                    {
                        "method": "turn/completed",
                        "params": {
                            "threadId": "thread-1",
                            "turn": {"id": "turn-1", "status": "completed"},
                        },
                    },
                ]
            )
            fixture.adapter.drain_messages()
            fixture.adapter.process_relays()

            self.assertEqual(
                [call["text"] for call in fixture.relay.calls],
                ["Aldım, inceliyorum.", "Tamamlandı."],
            )
            attention = fixture.store.snapshot("reviewer")["attentions"][0]
            self.assertIsNotNone(attention["first_response_at_ms"])
            self.assertIsNotNone(attention["final_response_at_ms"])
            self.assertIsNotNone(attention["turn_completed_at_ms"])
            fixture.adapter.close()

    def test_late_direct_gets_one_new_commentary_in_same_turn(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(153954))
            fixture.adapter.dispatch_one()
            def comment(item_id, text):
                fixture.adapter.handle_agent_item("thread-1", "turn-1", {
                    "id": item_id, "type": "agentMessage",
                    "phase": "commentary", "text": text,
                })
            comment("first", "İlk çağrıyı aldım.")
            fixture.adapter.process_relays()
            fixture.ingest(delivery(153975))
            fixture.adapter.dispatch_one()
            comment("late", "Evet, bu son mesajın bana ulaştı.")
            comment("progress", "Hâlâ çalışıyorum.")
            fixture.adapter.process_relays()
            self.assertEqual([r["text"] for r in fixture.relay.calls], [
                "İlk çağrıyı aldım.", "Evet, bu son mesajın bana ulaştı.",
            ])
            rows = fixture.store.snapshot("reviewer")["attentions"]
            self.assertTrue(all(r["first_response_at_ms"] for r in rows))
            self.assertTrue(all(r["final_response_at_ms"] is None for r in rows))
            self.assertEqual(sum(m == "turn/start" for m, _ in fixture.fake.requests), 1)
            self.assertEqual(sum(m == "turn/steer" for m, _ in fixture.fake.requests), 1)
            self.assertNotIn("turn/interrupt", [m for m, _ in fixture.fake.requests])
            fixture.adapter.close()

    def test_commentary_budget_does_not_reset_for_broadcast_or_pending_direct(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(120))
            fixture.adapter.dispatch_one()
            for item_id in ("first", "progress"):
                if item_id == "progress":
                    fixture.ingest(delivery(121, priority="broadcast"), delivery(122))
                    # Only the broadcast is accepted; the direct is still pending.
                    pending = fixture.store.snapshot("reviewer")["attentions"]
                    broadcast = next(r for r in pending if r["priority"] == "broadcast")
                    fixture.store.mark_accepted(broadcast["attention_id"], "thread-1", "turn-1",
                                                fixture.adapter.owner, fixture.adapter.epoch)
                fixture.adapter.handle_agent_item("thread-1", "turn-1", {
                    "id": item_id, "type": "agentMessage",
                    "phase": "commentary", "text": item_id,
                })
            fixture.adapter.process_relays()
            self.assertEqual([r["text"] for r in fixture.relay.calls], ["first"])
            fixture.adapter.close()

    def test_agent_direct_also_reopens_commentary_budget(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(120))
            fixture.adapter.dispatch_one()
            fixture.adapter.handle_agent_item("thread-1", "turn-1", {
                "id": "first", "type": "agentMessage", "phase": "commentary", "text": "First",
            })
            direct = delivery(121, priority="addressed_agent")
            direct["event"]["sender_type"] = "agent"
            fixture.ingest(direct)
            fixture.adapter.dispatch_one()
            fixture.adapter.handle_agent_item("thread-1", "turn-1", {
                "id": "agent-answer", "type": "agentMessage", "phase": "commentary", "text": "Agent answer",
            })
            fixture.adapter.process_relays()
            self.assertEqual([r["text"] for r in fixture.relay.calls], ["First", "Agent answer"])
            fixture.adapter.close()

    def test_pre_steer_progress_does_not_spend_new_direct_budget(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(120))
            fixture.adapter.dispatch_one()
            fixture.adapter.handle_agent_item("thread-1", "turn-1", {
                "id": "first", "type": "agentMessage", "phase": "commentary", "text": "First",
            })
            fixture.ingest(delivery(121))
            fixture.fake.before_steer_response = [{"method": "item/completed", "params": {
                "threadId": "thread-1", "turnId": "turn-1", "item": {
                    "id": "before", "type": "agentMessage", "phase": "commentary", "text": "Before",
                },
            }}]
            fixture.adapter.dispatch_one()
            fixture.adapter.handle_agent_item("thread-1", "turn-1", {
                "id": "after", "type": "agentMessage", "phase": "commentary", "text": "After",
            })
            fixture.adapter.process_relays()
            self.assertEqual([r["text"] for r in fixture.relay.calls], ["First", "After"])
            fixture.adapter.close()

    def test_replayed_commentary_never_spends_new_direct_budget(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(120))
            fixture.adapter.dispatch_one()
            old = {"id": "old", "type": "agentMessage", "phase": "commentary", "text": "Old"}
            fixture.adapter.handle_agent_item("thread-1", "turn-1", old)
            fixture.adapter.process_relays()
            fixture.ingest(delivery(121), delivery(122))
            fixture.adapter.dispatch_one()
            fixture.adapter.dispatch_one()
            fixture.adapter.handle_agent_item("thread-1", "turn-1", old)
            fixture.adapter.handle_agent_item("thread-1", "turn-1", {
                "id": "new", "type": "agentMessage", "phase": "commentary", "text": "New",
            })
            fixture.adapter.process_relays()
            self.assertEqual([r["text"] for r in fixture.relay.calls], ["Old", "New"])
            self.assertEqual(len(json.loads(fixture.relay.calls[0]["covered_attention_ids_json"])), 1)
            self.assertEqual(len(json.loads(fixture.relay.calls[1]["covered_attention_ids_json"])), 3)
            fixture.adapter.close()

    def test_commentary_retry_reserves_budget_durably_across_restart(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp), relay=RelayRecorder(fail_first=True))
            fixture.ingest(delivery(120))
            fixture.adapter.dispatch_one()
            old = {"id": "old", "type": "agentMessage", "phase": "commentary", "text": "Old"}
            fixture.adapter.handle_agent_item("thread-1", "turn-1", old)
            self.assertEqual(fixture.adapter.process_relays(), 0)
            original_op_id = fixture.relay.calls[0]["op_id"]
            fixture.adapter.close()
            adapter = PersistentCodexAdapter(
                store=AttentionStore(Path(tmp) / "state.sqlite3"), inbox=fixture.inbox,
                identity="reviewer", workdir=Path(tmp), codex_bin="codex",
                relay_mode="live", relay=fixture.relay, context_provider=lambda _a: [],
                app_server_factory=lambda _b, _t: fixture.fake,
            )
            adapter.initialize()
            adapter.handle_agent_item("thread-1", "turn-1", {
                "id": "progress", "type": "agentMessage", "phase": "commentary", "text": "Progress",
            })
            fixture.ingest(delivery(121))
            adapter.dispatch_one()
            adapter.handle_agent_item("thread-1", "turn-1", old)
            adapter.handle_agent_item("thread-1", "turn-1", {
                "id": "new", "type": "agentMessage", "phase": "commentary", "text": "New",
            })
            with fixture.store.connect() as connection:
                connection.execute("UPDATE relay_items SET next_attempt_at_ms = 0")
            self.assertEqual(adapter.process_relays(), 2)
            self.assertEqual([r["text"] for r in fixture.relay.calls], ["Old", "Old", "New"])
            self.assertEqual(fixture.relay.calls[1]["op_id"], original_op_id)
            adapter.close()

    def test_ten_late_directs_each_get_one_update_and_final_still_covers_all(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            for mid in range(300, 310):
                fixture.ingest(delivery(mid))
                fixture.adapter.dispatch_one()
                for suffix in ("reply", "progress"):
                    fixture.adapter.handle_agent_item("thread-1", "turn-1", {
                        "id": f"{mid}-{suffix}", "type": "agentMessage",
                        "phase": "commentary", "text": f"{mid}-{suffix}",
                    })
                fixture.adapter.process_relays()
            fixture.adapter.handle_agent_item("thread-1", "turn-1", {
                "id": "final", "type": "agentMessage", "phase": "final_answer", "text": "Done",
            })
            fixture.adapter.handle_turn_completed("thread-1", {"id": "turn-1", "status": "completed"})
            fixture.adapter.process_relays()
            self.assertEqual([r["text"] for r in fixture.relay.calls],
                             [f"{mid}-reply" for mid in range(300, 310)] + ["Done"])
            rows = fixture.store.snapshot("reviewer")["attentions"]
            self.assertTrue(all(r["first_response_at_ms"] and r["final_response_at_ms"] and
                                r["turn_completed_at_ms"] for r in rows))
            fixture.adapter.close()

    def test_shadow_commentary_budget_is_reserved_without_response_claim(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp), relay_mode="shadow")
            for mid in (120, 121):
                fixture.ingest(delivery(mid))
                fixture.adapter.dispatch_one()
                for suffix in ("reply", "progress"):
                    fixture.adapter.handle_agent_item("thread-1", "turn-1", {
                        "id": f"{mid}-{suffix}", "type": "agentMessage",
                        "phase": "commentary", "text": suffix,
                    })
            with fixture.store.connect() as connection:
                statuses = [tuple(r) for r in connection.execute(
                    "SELECT relay_kind, status FROM relay_items ORDER BY rowid")]
            self.assertEqual(statuses, [("commentary", "shadow"), ("commentary_suppressed", "suppressed")] * 2)
            self.assertEqual(fixture.adapter.process_relays(), 0)
            self.assertTrue(all(r["first_response_at_ms"] is None for r in fixture.store.snapshot("reviewer")["attentions"]))
            fixture.adapter.close()

    def test_null_phase_falls_back_to_final_on_turn_completed(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(130))
            fixture.adapter.dispatch_one()
            fixture.fake.pending.extend(
                [
                    {
                        "method": "item/completed",
                        "params": {
                            "threadId": "thread-1",
                            "turnId": "turn-1",
                            "item": {
                                "id": "unknown-1",
                                "type": "agentMessage",
                                "phase": None,
                                "text": "Phase olmadan sonuç.",
                            },
                        },
                    },
                    {
                        "method": "turn/completed",
                        "params": {
                            "threadId": "thread-1",
                            "turn": {"id": "turn-1", "status": "completed"},
                        },
                    },
                ]
            )
            fixture.adapter.drain_messages()
            fixture.adapter.process_relays()
            self.assertEqual(len(fixture.relay.calls), 1)
            self.assertEqual(fixture.relay.calls[0]["text"], "Phase olmadan sonuç.")
            attention = fixture.store.snapshot("reviewer")["attentions"][0]
            self.assertIsNotNone(attention["final_response_at_ms"])
            fixture.adapter.close()

    def test_no_reply_sentinel_is_stored_never_relayed_and_closes_attention(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(135, priority="broadcast"))
            fixture.adapter.dispatch_one()
            fixture.fake.pending.extend(
                [
                    {
                        "method": "item/completed",
                        "params": {
                            "threadId": "thread-1",
                            "turnId": "turn-1",
                            "item": {
                                "id": "silent-final",
                                "type": "agentMessage",
                                "phase": "final_answer",
                                "text": NO_REPLY_SENTINEL,
                            },
                        },
                    },
                    {
                        "method": "turn/completed",
                        "params": {
                            "threadId": "thread-1",
                            "turn": {"id": "turn-1", "status": "completed"},
                        },
                    },
                ]
            )
            fixture.adapter.drain_messages()
            fixture.adapter.process_relays()
            self.assertEqual(fixture.relay.calls, [])
            with fixture.store.connect() as connection:
                output = connection.execute(
                    "SELECT relay_kind, status FROM relay_items"
                ).fetchone()
            self.assertEqual(dict(output), {
                "relay_kind": "no_reply",
                "status": "suppressed",
            })
            attention = fixture.store.snapshot("reviewer")["attentions"][0]
            self.assertEqual(attention["terminal_status"], "cancelled")
            self.assertEqual(
                attention["terminal_reason"],
                "model explicitly chose no reply",
            )
            self.assertIsNone(attention["first_response_at_ms"])
            self.assertIsNone(attention["final_response_at_ms"])
            self.assertEqual(
                fixture.store.progress_summary("reviewer")[
                    "reply_required_pending"
                ],
                0,
            )
            fixture.adapter.close()

    def test_required_direct_cannot_be_cancelled_by_no_reply(self):
        # ADR: no-reply is legal only for optional attention. A mixed turn
        # must not let a broadcast silence an operator's direct question.
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(136, text="@reviewer cevap ver!"))
            fixture.adapter.dispatch_one()
            required = fixture.store.snapshot("reviewer")["attentions"][0]
            prompt = attention_prompt("reviewer", required, [])
            self.assertNotIn("If no useful room reply is warranted", prompt)
            self.assertIn("must not", prompt)
            self.assertIn("automatically dispatched headless Loca", prompt)
            fixture.adapter.handle_agent_item("thread-1", "turn-1", {
                "id": "silent-required", "type": "agentMessage",
                "phase": "final_answer", "text": NO_REPLY_SENTINEL,
            })
            fixture.store.mark_turn_completed(
                "turn-1", "completed", "", fixture.adapter.owner,
                fixture.adapter.epoch,
            )
            fixture.adapter.process_relays()
            row = fixture.store.snapshot("reviewer")["attentions"][0]
            self.assertIsNone(row["terminal_status"])
            self.assertIsNone(row["final_response_at_ms"])
            self.assertEqual(fixture.store.progress_summary("reviewer")["reply_required_pending"], 1)
            self.assertEqual(fixture.relay.calls, [])
            fixture.adapter.close()

    def test_mixed_turn_silence_only_closes_optional_attention(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(137, text="status?"))
            fixture.adapter.dispatch_one()
            fixture.ingest(delivery(138, priority="broadcast"))
            fixture.adapter.dispatch_one()
            fixture.adapter.handle_agent_item("thread-1", "turn-1", {
                "id": "silence", "type": "agentMessage",
                "phase": None, "text": NO_REPLY_SENTINEL,
            })
            rows = fixture.store.snapshot("reviewer")["attentions"]
            direct = next(r for r in rows if r["reply_required"])
            optional = next(r for r in rows if not r["reply_required"])
            self.assertIsNone(direct["terminal_status"])
            self.assertEqual(optional["terminal_status"], "cancelled")
            # Later real output can still fulfill the direct obligation.
            fixture.adapter.handle_agent_item("thread-1", "turn-1", {
                "id": "real-final", "type": "agentMessage",
                "phase": "final_answer", "text": "The build is still running.",
            })
            fixture.adapter.process_relays()
            direct = next(r for r in fixture.store.snapshot("reviewer")["attentions"] if r["reply_required"])
            self.assertIsNone(direct["terminal_status"])
            self.assertIsNotNone(direct["final_response_at_ms"])
            self.assertEqual(len(fixture.relay.calls), 1)
            fixture.adapter.close()

    def test_agent_mention_to_lead_reaches_dispatch_and_relay_once(self):
        from test_listener_delivery import LISTENER

        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            record = LISTENER.make_delivery("sb-dev", {
                "id": 145433, "room": "sb-dev", "sender": "another-agent",
                "sender_type": "agent", "text": "@reviewer neredesin?",
            }, "reviewer", "https://loca.example", is_lead=True)
            fixture.ingest(record)
            self.assertTrue(fixture.adapter.dispatch_one())
            fixture.adapter.handle_agent_item("thread-1", "turn-1", {
                "id": "lead-answer", "type": "agentMessage",
                "phase": "final_answer", "text": "Review is running.",
            })
            fixture.adapter.handle_turn_completed("thread-1", {"id": "turn-1", "status": "completed"})
            fixture.adapter.process_relays()
            fixture.ingest(record)
            self.assertFalse(fixture.adapter.dispatch_one())
            rows = fixture.store.snapshot("reviewer")["attentions"]
            self.assertEqual(len(rows), 1)
            self.assertIsNotNone(rows[0]["accepted_at_ms"])
            self.assertIsNotNone(rows[0]["final_response_at_ms"])
            self.assertEqual(len(fixture.relay.calls), 1)
            self.assertEqual(sum(m == "turn/start" for m, _ in fixture.fake.requests), 1)
            fixture.adapter.close()

    def test_relay_retry_keeps_same_operation_id_after_turn_completion(self):
        with tempfile.TemporaryDirectory() as tmp:
            relay = RelayRecorder(fail_first=True)
            fixture = AdapterFixture(Path(tmp), relay=relay)
            fixture.ingest(delivery(140))
            fixture.adapter.dispatch_one()
            fixture.fake.pending.extend(
                [
                    {
                        "method": "item/completed",
                        "params": {
                            "threadId": "thread-1",
                            "turnId": "turn-1",
                            "item": {
                                "id": "final-retry",
                                "type": "agentMessage",
                                "phase": "final_answer",
                                "text": "Retry sonucu.",
                            },
                        },
                    },
                    {
                        "method": "turn/completed",
                        "params": {
                            "threadId": "thread-1",
                            "turn": {"id": "turn-1", "status": "completed"},
                        },
                    },
                ]
            )
            fixture.adapter.drain_messages()
            self.assertEqual(fixture.adapter.process_relays(), 0)
            with fixture.store.connect() as connection:
                connection.execute(
                    "UPDATE relay_items SET next_attempt_at_ms = 0"
                )
            self.assertEqual(fixture.adapter.process_relays(), 1)
            self.assertEqual(relay.calls[0]["op_id"], relay.calls[1]["op_id"])
            attention = fixture.store.snapshot("reviewer")["attentions"][0]
            self.assertIsNotNone(attention["turn_completed_at_ms"])
            self.assertIsNotNone(attention["final_response_at_ms"])
            fixture.adapter.close()

    def test_shadow_mode_never_sends_or_claims_response(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp), relay_mode="shadow")
            fixture.ingest(delivery(150))
            fixture.adapter.dispatch_one()
            fixture.fake.pending.append(
                {
                    "method": "item/completed",
                    "params": {
                        "threadId": "thread-1",
                        "turnId": "turn-1",
                        "item": {
                            "id": "final-shadow",
                            "type": "agentMessage",
                            "phase": "final_answer",
                            "text": "Shadow sonucu.",
                        },
                    },
                }
            )
            fixture.adapter.drain_messages()
            self.assertEqual(fixture.adapter.process_relays(), 0)
            self.assertEqual(fixture.relay.calls, [])
            attention = fixture.store.snapshot("reviewer")["attentions"][0]
            self.assertIsNone(attention["final_response_at_ms"])
            fixture.adapter.close()

    def test_security_control_then_direct_never_livelocks(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(
                delivery(160, priority="security_control"),
                delivery(161, priority="direct_user"),
            )
            self.assertTrue(fixture.adapter.dispatch_one())
            self.assertTrue(fixture.adapter.dispatch_one())
            methods = [method for method, _ in fixture.fake.requests]
            self.assertEqual(methods.count("turn/start"), 1)
            self.assertEqual(methods.count("turn/steer"), 1)
            self.assertNotIn("turn/interrupt", methods)
            attentions = fixture.store.snapshot("reviewer")["attentions"]
            self.assertTrue(all(row["accepted_at_ms"] for row in attentions))
            fixture.adapter.close()

    def test_explicit_stop_interrupts_active_turn_and_cancels_older_pending(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(165))
            self.assertTrue(fixture.adapter.dispatch_one())
            fixture.ingest(delivery(166), stop_delivery(167))

            self.assertTrue(fixture.adapter.dispatch_one())
            methods = [method for method, _ in fixture.fake.requests]
            self.assertEqual(methods.count("turn/interrupt"), 1)
            self.assertEqual(methods.count("turn/steer"), 0)
            rows = {
                row["delivery_id"]: row
                for row in fixture.store.snapshot("reviewer")["attentions"]
            }
            self.assertEqual(rows["sb-dev:165"]["terminal_status"], "interrupted")
            self.assertEqual(rows["sb-dev:166"]["terminal_status"], "cancelled")
            self.assertEqual(rows["sb-dev:167"]["terminal_status"], "cancelled")

            fixture.fake.pending.append(
                {
                    "method": "turn/completed",
                    "params": {
                        "threadId": "thread-1",
                        "turn": {"id": "turn-1", "status": "interrupted"},
                    },
                }
            )
            fixture.adapter.drain_messages()
            rows = {
                row["delivery_id"]: row
                for row in fixture.store.snapshot("reviewer")["attentions"]
            }
            self.assertEqual(rows["sb-dev:165"]["terminal_status"], "interrupted")
            self.assertEqual(fixture.store.pending_relays("reviewer"), [])
            fixture.adapter.close()

    def test_stop_preserves_pending_task_care_and_security_records(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(162, priority="direct_user"))
            self.assertTrue(fixture.adapter.dispatch_one())
            fixture.ingest(
                delivery(163, priority="care_signal"),
                delivery(164, priority="explicit_task"),
                delivery(165, priority="addressed_agent"),
                delivery(166, priority="security_control"),
                stop_delivery(167),
            )

            # The older security record is itself processed first (same
            # highest priority); the following stop must then requeue it.
            self.assertTrue(fixture.adapter.dispatch_one())
            self.assertTrue(fixture.adapter.dispatch_one())
            rows = {
                row["delivery_id"]: row
                for row in fixture.store.snapshot("reviewer")["attentions"]
            }
            self.assertEqual(rows["sb-dev:162"]["terminal_status"], "interrupted")
            self.assertEqual(rows["sb-dev:165"]["terminal_status"], "cancelled")
            self.assertEqual(rows["sb-dev:167"]["terminal_status"], "cancelled")
            for delivery_id in ("sb-dev:163", "sb-dev:164", "sb-dev:166"):
                self.assertIsNone(rows[delivery_id]["terminal_status"])
                self.assertIsNone(rows[delivery_id]["accepted_at_ms"])

            self.assertEqual(
                fixture.store.next_pending("reviewer")["delivery_id"],
                "sb-dev:166",
            )
            fixture.adapter.close()

    def test_stop_requeues_an_active_protected_task_instead_of_erasing_it(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(169, priority="explicit_task"))
            self.assertTrue(fixture.adapter.dispatch_one())
            fixture.ingest(stop_delivery(170))

            self.assertTrue(fixture.adapter.dispatch_one())
            task = {
                row["delivery_id"]: row
                for row in fixture.store.snapshot("reviewer")["attentions"]
            }["sb-dev:169"]
            self.assertIsNone(task["terminal_status"])
            self.assertIsNone(task["accepted_at_ms"])
            self.assertIsNone(task["turn_id"])
            self.assertEqual(
                fixture.store.next_pending("reviewer")["delivery_id"],
                "sb-dev:169",
            )

            self.assertTrue(fixture.adapter.dispatch_one())
            task = {
                row["delivery_id"]: row
                for row in fixture.store.snapshot("reviewer")["attentions"]
            }["sb-dev:169"]
            self.assertEqual(task["attempts"], 2)
            self.assertEqual(task["turn_id"], "turn-2")
            fixture.adapter.close()

    def test_room_closed_terminally_cancels_protected_work_and_forgets_thread(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(171, priority="explicit_task"))
            self.assertTrue(fixture.adapter.dispatch_one())
            fixture.ingest(
                delivery(172, priority="care_signal"),
                delivery(173, priority="explicit_task"),
                room_closed_delivery(174),
            )

            self.assertTrue(fixture.adapter.dispatch_one())
            rows = {
                row["delivery_id"]: row
                for row in fixture.store.snapshot("reviewer")["attentions"]
            }
            for delivery_id in (
                "sb-dev:171",
                "sb-dev:172",
                "sb-dev:173",
                "sb-dev:174",
            ):
                self.assertEqual(rows[delivery_id]["terminal_status"], "cancelled")
                self.assertIn("permanently closed", rows[delivery_id]["terminal_reason"])
            self.assertIsNone(fixture.store.next_pending("reviewer"))
            self.assertEqual(
                fixture.store.get_thread(
                    "reviewer", "https://loca.example", "sb-dev"
                ),
                "",
            )
            self.assertNotIn(
                ("https://loca.example", "sb-dev"),
                fixture.adapter.scope_threads,
            )
            fixture.adapter.close()

    def test_room_closed_ignores_late_output_from_cancelled_turn(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(175, priority="explicit_task"))
            self.assertTrue(fixture.adapter.dispatch_one())
            fixture.ingest(room_closed_delivery(176))
            self.assertTrue(fixture.adapter.dispatch_one())

            fixture.fake.pending.extend(
                [
                    {
                        "method": "item/completed",
                        "params": {
                            "threadId": "thread-1",
                            "turnId": "turn-1",
                            "item": {
                                "id": "late-room-output",
                                "type": "agentMessage",
                                "phase": "final_answer",
                                "text": "must not relay",
                            },
                        },
                    },
                    {
                        "method": "turn/completed",
                        "params": {
                            "threadId": "thread-1",
                            "turn": {"id": "turn-1", "status": "interrupted"},
                        },
                    },
                ]
            )
            fixture.adapter.drain_messages()
            self.assertEqual(fixture.adapter.process_relays(), 0)
            self.assertEqual(fixture.relay.calls, [])
            fixture.adapter.close()

    def test_room_closed_cancels_completed_turn_awaiting_final_relay(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(177, priority="direct_user"))
            self.assertTrue(fixture.adapter.dispatch_one())
            fixture.fake.pending.extend(
                [
                    {
                        "method": "item/completed",
                        "params": {
                            "threadId": "thread-1",
                            "turnId": "turn-1",
                            "item": {
                                "id": "final-awaiting-relay",
                                "type": "agentMessage",
                                "phase": "final_answer",
                                "text": "reply waiting for Loca",
                            },
                        },
                    },
                    {
                        "method": "turn/completed",
                        "params": {
                            "threadId": "thread-1",
                            "turn": {"id": "turn-1", "status": "completed"},
                        },
                    },
                ]
            )
            fixture.adapter.drain_messages()
            before = fixture.store.snapshot("reviewer")["attentions"][0]
            self.assertIsNotNone(before["turn_completed_at_ms"])
            self.assertIsNone(before["final_response_at_ms"])
            self.assertEqual(len(fixture.store.pending_relays("reviewer")), 1)
            self.assertEqual(
                fixture.store.progress_summary("reviewer")["reply_required_pending"],
                1,
            )

            fixture.ingest(room_closed_delivery(178))
            self.assertTrue(fixture.adapter.dispatch_one())

            rows = {
                row["delivery_id"]: row
                for row in fixture.store.snapshot("reviewer")["attentions"]
            }
            self.assertEqual(rows["sb-dev:177"]["terminal_status"], "cancelled")
            self.assertEqual(rows["sb-dev:178"]["terminal_status"], "cancelled")
            self.assertEqual(fixture.store.pending_relays("reviewer"), [])
            self.assertEqual(
                fixture.store.progress_summary("reviewer")["reply_required_pending"],
                0,
            )
            fixture.adapter.close()

    def test_restart_room_closed_fences_recovered_final_before_relay(self):
        """A close already in the inbox wins over an old pending final."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fixture = AdapterFixture(root)
            fixture.ingest(delivery(179, priority="direct_user"))
            self.assertTrue(fixture.adapter.dispatch_one())
            fixture.fake.pending.extend(
                [
                    {
                        "method": "item/completed",
                        "params": {
                            "threadId": "thread-1",
                            "turnId": "turn-1",
                            "item": {
                                "id": "restart-final-awaiting-relay",
                                "type": "agentMessage",
                                "phase": "final_answer",
                                "text": "must be fenced during restart",
                            },
                        },
                    },
                    {
                        "method": "turn/completed",
                        "params": {
                            "threadId": "thread-1",
                            "turn": {"id": "turn-1", "status": "completed"},
                        },
                    },
                ]
            )
            fixture.adapter.drain_messages()
            self.assertEqual(len(fixture.store.pending_relays("reviewer")), 1)
            append(fixture.inbox, room_closed_delivery(180))
            fixture.adapter.close()

            restarted = PersistentCodexAdapter(
                store=AttentionStore(root / "state.sqlite3"),
                inbox=fixture.inbox,
                identity="reviewer",
                workdir=root,
                codex_bin="codex",
                relay_mode="live",
                relay=fixture.relay,
                context_provider=lambda _attention: [],
                app_server_factory=lambda _bin, _timeout: fixture.fake,
            )
            restarted.initialize()
            result = restarted.cycle()

            self.assertEqual(result["relayed"], 0)
            self.assertEqual(fixture.relay.calls, [])
            self.assertEqual(restarted.store.pending_relays("reviewer"), [])
            rows = {
                row["delivery_id"]: row
                for row in restarted.store.snapshot("reviewer")["attentions"]
            }
            self.assertEqual(rows["sb-dev:179"]["terminal_status"], "cancelled")
            self.assertEqual(rows["sb-dev:180"]["terminal_status"], "cancelled")
            restarted.close()

    def test_interrupted_turn_does_not_promote_phase_null_output(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(168))
            self.assertTrue(fixture.adapter.dispatch_one())
            fixture.fake.pending.extend(
                [
                    {
                        "method": "item/completed",
                        "params": {
                            "threadId": "thread-1",
                            "turnId": "turn-1",
                            "item": {
                                "id": "partial-168",
                                "type": "agentMessage",
                                "text": "partial answer",
                                "phase": None,
                            },
                        },
                    },
                    {
                        "method": "turn/completed",
                        "params": {
                            "threadId": "thread-1",
                            "turn": {"id": "turn-1", "status": "interrupted"},
                        },
                    },
                ]
            )
            fixture.adapter.drain_messages()
            with fixture.store.connect() as connection:
                relays = [
                    dict(row)
                    for row in connection.execute(
                        "SELECT relay_kind, status FROM relay_items "
                        "ORDER BY created_at_ms, op_id"
                    ).fetchall()
                ]
            self.assertEqual([row["relay_kind"] for row in relays], ["candidate"])
            self.assertEqual(relays[0]["status"], "suppressed")
            fixture.adapter.close()

    def test_rejected_steer_keeps_direct_attention_pending(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(170), delivery(171))
            self.assertTrue(fixture.adapter.dispatch_one())
            fixture.fake.reject_steer = True
            self.assertFalse(fixture.adapter.dispatch_one())
            attentions = fixture.store.snapshot("reviewer")["attentions"]
            self.assertIsNotNone(attentions[0]["accepted_at_ms"])
            self.assertIsNone(attentions[1]["accepted_at_ms"])
            self.assertIn("turn/steer rejected", attentions[1]["terminal_reason"])
            fixture.adapter.close()

    def test_no_active_turn_requeues_missing_accepted_turn_then_recovers_fifo(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(172))
            self.assertTrue(fixture.adapter.dispatch_one())

            # Reproduce the real crash window: turn/start returned an id, but
            # app-server no longer has that turn in durable thread history.
            fixture.fake.threads["thread-1"] = []
            fixture.ingest(delivery(173))
            fixture.fake.reject_steer = True
            fixture.fake.reject_steer_error = "no active turn to steer"

            self.assertFalse(fixture.adapter.dispatch_one())
            rows = {
                row["delivery_id"]: row
                for row in fixture.store.snapshot("reviewer")["attentions"]
            }
            self.assertIsNone(rows["sb-dev:172"]["accepted_at_ms"])
            self.assertIsNone(rows["sb-dev:173"]["accepted_at_ms"])
            self.assertEqual(fixture.adapter.active_turns, {})

            fixture.fake.reject_steer = False
            self.assertTrue(fixture.adapter.dispatch_one())
            self.assertTrue(fixture.adapter.dispatch_one())
            rows = {
                row["delivery_id"]: row
                for row in fixture.store.snapshot("reviewer")["attentions"]
            }
            self.assertEqual(rows["sb-dev:172"]["turn_id"], "turn-2")
            self.assertEqual(rows["sb-dev:173"]["turn_id"], "turn-2")
            self.assertGreaterEqual(rows["sb-dev:172"]["attempts"], 2)
            fixture.adapter.close()

    def test_periodic_reconciliation_requeues_missing_turn_without_new_message(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(174))
            self.assertTrue(fixture.adapter.dispatch_one())
            fixture.fake.threads["thread-1"] = []
            fixture.adapter.next_turn_reconcile_ms = 0

            result = fixture.adapter.cycle()

            self.assertEqual(result["reconciled"], 1)
            row = fixture.store.snapshot("reviewer")["attentions"][0]
            # The same cycle may immediately dispatch the recovered record;
            # either way it must no longer be bound to the vanished turn.
            self.assertNotEqual(row["turn_id"], "turn-1")
            methods = [method for method, _ in fixture.fake.requests]
            self.assertEqual(methods.count("turn/start"), 2)
            fixture.adapter.close()

    def test_restart_requeues_accepted_turn_absent_from_durable_history(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fixture = AdapterFixture(root)
            fixture.ingest(delivery(175))
            self.assertTrue(fixture.adapter.dispatch_one())
            fixture.fake.threads["thread-1"] = []
            with fixture.store.connect() as connection:
                connection.execute(
                    "UPDATE turns SET started_at_ms = started_at_ms - 61000"
                )
            fixture.adapter.close()

            restarted = PersistentCodexAdapter(
                store=AttentionStore(root / "state.sqlite3"),
                inbox=fixture.inbox,
                identity="reviewer",
                workdir=root,
                codex_bin="codex",
                relay_mode="live",
                relay=fixture.relay,
                context_provider=lambda _attention: [],
                app_server_factory=lambda _bin, _timeout: fixture.fake,
            )
            restarted.initialize()

            row = restarted.store.snapshot("reviewer")["attentions"][0]
            self.assertIsNone(row["accepted_at_ms"])
            self.assertIsNone(row["turn_id"])
            self.assertIn("absent from durable thread history", row["terminal_reason"])
            self.assertTrue(restarted.dispatch_one())
            row = restarted.store.snapshot("reviewer")["attentions"][0]
            self.assertEqual(row["turn_id"], "turn-2")
            restarted.close()

    def test_restart_recovers_completed_turn_and_relays_unknown_phase(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inbox = root / "inbox.jsonl"
            store = AttentionStore(root / "state.sqlite3")
            append(inbox, delivery(180))
            store.ingest_inbox(inbox, "reviewer")
            attention = store.next_pending("reviewer")
            owner = "old-adapter"
            epoch = store.claim_lease("reviewer", owner, 10_000)
            store.set_thread(
                "reviewer",
                "https://loca.example",
                "sb-dev",
                "thread-old",
                owner,
                epoch,
            )
            store.mark_accepted(
                attention["attention_id"],
                "thread-old",
                "turn-old",
                owner,
                epoch,
            )
            store.release_lease("reviewer", owner, epoch)

            fake = FakeAppServer("codex", 10)
            fake.threads["thread-old"] = [
                {
                    "id": "turn-old",
                    "status": "completed",
                    "items": [
                        {
                            "id": "recovered-item",
                            "type": "agentMessage",
                            "phase": None,
                            "text": "Restart sonrası sonuç.",
                        }
                    ],
                }
            ]
            relay = RelayRecorder()
            adapter = PersistentCodexAdapter(
                store=store,
                inbox=inbox,
                identity="reviewer",
                workdir=root,
                codex_bin="codex",
                relay_mode="live",
                relay=relay,
                context_provider=lambda _attention: [],
                app_server_factory=lambda _bin, _timeout: fake,
            )
            adapter.initialize()
            self.assertEqual(adapter.process_relays(), 1)
            self.assertEqual(relay.calls[0]["text"], "Restart sonrası sonuç.")
            row = store.snapshot("reviewer")["attentions"][0]
            self.assertIsNotNone(row["turn_completed_at_ms"])
            self.assertIsNotNone(row["final_response_at_ms"])
            adapter.close()

    def test_notification_before_steer_acceptance_does_not_cover_new_attention(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = AdapterFixture(Path(tmp))
            fixture.ingest(delivery(190))
            fixture.adapter.dispatch_one()
            fixture.ingest(delivery(191))
            fixture.fake.before_steer_response = [
                {
                    "method": "item/completed",
                    "params": {
                        "threadId": "thread-1",
                        "turnId": "turn-1",
                        "item": {
                            "id": "old-output",
                            "type": "agentMessage",
                            "phase": "commentary",
                            "text": "İlk çağrıyı aldım.",
                        },
                    },
                }
            ]

            self.assertTrue(fixture.adapter.dispatch_one())
            fixture.adapter.process_relays()

            self.assertEqual(len(fixture.relay.calls), 1)
            covered = json.loads(
                fixture.relay.calls[0]["covered_attention_ids_json"]
            )
            self.assertEqual(
                covered,
                [
                    attention_id_for(
                        "reviewer", "https://loca.example", "sb-dev:190"
                    )
                ],
            )
            fixture.adapter.close()

    def test_missing_persisted_thread_requeues_and_starts_fresh_turn(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inbox = root / "inbox.jsonl"
            store = AttentionStore(root / "state.sqlite3")
            append(inbox, delivery(200))
            store.ingest_inbox(inbox, "reviewer")
            attention = store.next_pending("reviewer")
            owner = "old-adapter"
            epoch = store.claim_lease("reviewer", owner, 10_000)
            store.set_thread(
                "reviewer",
                "https://loca.example",
                "sb-dev",
                "gone-thread",
                owner,
                epoch,
            )
            store.mark_accepted(
                attention["attention_id"],
                "gone-thread",
                "gone-turn",
                owner,
                epoch,
            )
            store.release_lease("reviewer", owner, epoch)

            fake = FakeAppServer("codex", 10)
            adapter = PersistentCodexAdapter(
                store=store,
                inbox=inbox,
                identity="reviewer",
                workdir=root,
                codex_bin="codex",
                relay_mode="shadow",
                relay=RelayRecorder(),
                context_provider=lambda _attention: [],
                app_server_factory=lambda _bin, _timeout: fake,
            )
            adapter.initialize()
            self.assertTrue(adapter.dispatch_one())

            row = store.snapshot("reviewer")["attentions"][0]
            self.assertEqual(row["turn_id"], "turn-1")
            methods = [method for method, _ in fake.requests]
            self.assertEqual(methods.count("thread/resume"), 1)
            self.assertEqual(methods.count("thread/start"), 1)
            adapter.close()

    def test_restart_rehydrates_completed_lifecycle_from_ledger(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            health_file = root / "health.json"
            fixture = AdapterFixture(root)
            fixture.ingest(delivery(205))
            self.assertTrue(fixture.adapter.dispatch_one())
            fixture.fake.pending.extend(
                [
                    {
                        "method": "item/completed",
                        "params": {
                            "threadId": "thread-1",
                            "turnId": "turn-1",
                            "item": {
                                "id": "restart-health-final",
                                "type": "agentMessage",
                                "phase": "final_answer",
                                "text": "durable result",
                            },
                        },
                    },
                    {
                        "method": "turn/completed",
                        "params": {
                            "threadId": "thread-1",
                            "turn": {"id": "turn-1", "status": "completed"},
                        },
                    },
                ]
            )
            fixture.adapter.drain_messages()
            self.assertEqual(fixture.adapter.process_relays(), 1)
            expected = fixture.store.snapshot("reviewer")["attentions"][0]
            fixture.adapter.close()

            restarted = PersistentCodexAdapter(
                store=AttentionStore(root / "state.sqlite3"),
                inbox=fixture.inbox,
                identity="reviewer",
                workdir=root,
                codex_bin="codex",
                relay_mode="live",
                relay=fixture.relay,
                context_provider=lambda _attention: [],
                health_file=health_file,
                app_server_factory=lambda _bin, _timeout: fixture.fake,
            )
            restarted.initialize()

            persisted_health = json.loads(health_file.read_text(encoding="utf-8"))
            self.assertEqual(
                persisted_health["last_attention_id"], expected["attention_id"]
            )
            self.assertEqual(
                persisted_health["last_accepted_attention_id"],
                expected["attention_id"],
            )
            for milestone in (
                "stored",
                "accepted",
                "first_response",
                "final_response",
                "turn_completed",
            ):
                self.assertTrue(persisted_health[milestone], milestone)
            self.assertEqual(persisted_health["reply"], "FINAL")
            restarted.close()

    def test_restart_reports_newer_stored_work_without_stale_acceptance(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fixture = AdapterFixture(root)
            fixture.ingest(delivery(206))
            self.assertTrue(fixture.adapter.dispatch_one())
            clock_skewed = delivery(207)
            clock_skewed["received_at_ms"] = 1
            fixture.ingest(clock_skewed)
            rows = fixture.store.snapshot("reviewer")["attentions"]
            newest = {row["delivery_id"]: row for row in rows}["sb-dev:207"]
            fixture.adapter.close()

            restarted = PersistentCodexAdapter(
                store=AttentionStore(root / "state.sqlite3"),
                inbox=fixture.inbox,
                identity="reviewer",
                workdir=root,
                codex_bin="codex",
                relay_mode="shadow",
                relay=RelayRecorder(),
                context_provider=lambda _attention: [],
                app_server_factory=lambda _bin, _timeout: fixture.fake,
            )
            restarted.initialize()

            self.assertEqual(
                restarted.health["last_attention_id"], newest["attention_id"]
            )
            self.assertIsNone(restarted.health["last_accepted_attention_id"])
            self.assertTrue(restarted.health["stored"])
            for milestone in (
                "accepted",
                "first_response",
                "final_response",
                "turn_completed",
            ):
                self.assertFalse(restarted.health[milestone], milestone)
            self.assertEqual(restarted.health["reply"], "PENDING")
            restarted.close()

    def test_rpc_accepted_crash_reconciles_without_duplicate_turn(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fixture = AdapterFixture(root, relay_mode="shadow")
            fixture.ingest(delivery(210))
            original_finalize = fixture.store.finalize_dispatch

            def crash_before_ledger_commit(*_args, **_kwargs):
                raise RuntimeError("injected crash after accepted RPC")

            fixture.store.finalize_dispatch = crash_before_ledger_commit
            with self.assertRaisesRegex(RuntimeError, "injected crash"):
                fixture.adapter.dispatch_one()
            pending = fixture.store.next_pending("reviewer")
            self.assertIsNotNone(pending)
            self.assertEqual(
                len(fixture.store.unresolved_dispatches("reviewer")), 1
            )
            fixture.store.finalize_dispatch = original_finalize
            fixture.adapter.close()

            restarted_store = AttentionStore(root / "state.sqlite3")
            restarted = PersistentCodexAdapter(
                store=restarted_store,
                inbox=fixture.inbox,
                identity="reviewer",
                workdir=root,
                codex_bin="codex",
                relay_mode="shadow",
                relay=RelayRecorder(),
                context_provider=lambda _attention: [],
                app_server_factory=lambda _bin, _timeout: fixture.fake,
            )
            restarted.initialize()

            row = restarted_store.snapshot("reviewer")["attentions"][0]
            self.assertEqual(row["turn_id"], "turn-1")
            self.assertIsNotNone(row["accepted_at_ms"])
            self.assertIsNone(restarted_store.next_pending("reviewer"))
            with restarted_store.connect() as connection:
                intent = connection.execute(
                    "SELECT * FROM dispatch_intents WHERE attention_id = ?",
                    (row["attention_id"],),
                ).fetchone()
            self.assertEqual(intent["status"], "reconciled")
            methods = [method for method, _ in fixture.fake.requests]
            self.assertEqual(methods.count("turn/start"), 1)
            restarted.close()

    def test_direct_restart_and_relay_retry_accelerated_soak(self):
        """Exercise days-worth of queue edges without wall-clock sleeps."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inbox = root / "inbox.jsonl"
            state_db = root / "state.sqlite3"
            fake = FakeAppServer("codex", 10)
            relay = RelayRecorder(fail_first=True)

            def start_adapter():
                adapter = PersistentCodexAdapter(
                    store=AttentionStore(state_db),
                    inbox=inbox,
                    identity="reviewer",
                    workdir=root,
                    codex_bin="codex",
                    relay_mode="live",
                    relay=relay,
                    context_provider=lambda _attention: [],
                    app_server_factory=lambda _bin, _timeout: fake,
                )
                adapter.initialize()
                return adapter

            adapter = start_adapter()
            for message_id in range(300, 364):
                append(inbox, delivery(message_id))
            adapter.store.ingest_inbox(inbox, "reviewer")

            for index in range(64):
                if index == 31:
                    def crash_after_rpc(*_args, **_kwargs):
                        raise RuntimeError("soak crash after accepted RPC")

                    adapter.store.finalize_dispatch = crash_after_rpc
                    with self.assertRaisesRegex(RuntimeError, "soak crash"):
                        adapter.dispatch_one()
                    adapter.close()
                    adapter = start_adapter()
                else:
                    self.assertTrue(adapter.dispatch_one())
                if index in {7, 15, 23, 39, 47, 55}:
                    adapter.close()
                    adapter = start_adapter()

            self.assertIsNone(adapter.store.next_pending("reviewer"))
            methods = [method for method, _ in fake.requests]
            self.assertEqual(methods.count("turn/start"), 1)
            self.assertEqual(methods.count("turn/steer"), 63)

            fake.pending.extend(
                [
                    {
                        "method": "item/completed",
                        "params": {
                            "threadId": "thread-1",
                            "turnId": "turn-1",
                            "item": {
                                "id": "soak-final",
                                "type": "agentMessage",
                                "phase": "final_answer",
                                "text": "all direct calls processed",
                            },
                        },
                    },
                    {
                        "method": "turn/completed",
                        "params": {
                            "threadId": "thread-1",
                            "turn": {"id": "turn-1", "status": "completed"},
                        },
                    },
                ]
            )
            adapter.drain_messages()
            self.assertEqual(adapter.process_relays(), 0)
            with adapter.store.connect() as connection:
                relay_row = connection.execute(
                    "SELECT op_id FROM relay_items WHERE relay_kind = 'final'"
                ).fetchone()
                connection.execute(
                    "UPDATE relay_items SET next_attempt_at_ms = 0 WHERE op_id = ?",
                    (relay_row["op_id"],),
                )
            self.assertEqual(adapter.process_relays(), 1)
            self.assertEqual(len(relay.calls), 2)
            self.assertEqual(relay.calls[0]["op_id"], relay.calls[1]["op_id"])
            covered = json.loads(relay.calls[1]["covered_attention_ids_json"])
            self.assertEqual(len(covered), 64)
            self.assertEqual(
                [value.rsplit(":", 1)[-1] for value in covered],
                [str(value) for value in range(300, 364)],
            )
            progress = adapter.store.progress_summary("reviewer")
            self.assertEqual(progress["reply_required_pending"], 0)
            adapter.close()


if __name__ == "__main__":
    unittest.main()
