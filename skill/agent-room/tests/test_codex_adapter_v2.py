import json
import sys
import tempfile
import unittest
from pathlib import Path


SKILL_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SKILL_DIR))

from attention_store import AttentionStore, attention_id_for  # noqa: E402
from codex_adapter_v2 import (  # noqa: E402
    ContextProvider,
    NO_REPLY_SENTINEL,
    PersistentCodexAdapter,
    attention_prompt,
    missing_thread_error,
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
