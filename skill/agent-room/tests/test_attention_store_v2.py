import json
import os
import sqlite3
import sys
import tempfile
import unittest
from pathlib import Path


SKILL_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SKILL_DIR))

from attention_store import AttentionStore, LeaseBusy, StaleLease  # noqa: E402


def delivery(
    message_id: int,
    *,
    priority: str = "direct_user",
    identity: str = "reviewer",
    text: str = "ping",
):
    return {
        "protocol_version": 2,
        "delivery_id": f"sb-dev:{message_id}",
        "server": "https://loca.example",
        "room": "sb-dev",
        "identity": identity,
        "priority": priority,
        "received_at_ms": message_id * 1000,
        "event": {
            "id": message_id,
            "room": "sb-dev",
            "sender": "operator",
            "sender_type": "user",
            "target": identity,
            "text": text,
        },
    }


def append(inbox: Path, record: dict):
    with inbox.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(record) + "\n")


class AttentionStoreV2Tests(unittest.TestCase):
    def test_first_runtime_start_skips_historical_inbox_then_reads_new_work(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inbox = root / "inbox.v2.jsonl"
            store = AttentionStore(root / "state.sqlite3")
            append(inbox, delivery(8, text="historical"))

            initial = store.initialize_ingestion_source(inbox, "reviewer")
            self.assertEqual(initial, inbox.stat().st_size)
            self.assertEqual(
                store.ingest_inbox(inbox, "reviewer")["inserted"], 0
            )

            append(inbox, delivery(9, text="new"))
            self.assertEqual(
                store.ingest_inbox(inbox, "reviewer")["inserted"], 1
            )
            rows = store.snapshot("reviewer")["attentions"]
            self.assertEqual([row["delivery_id"] for row in rows], ["sb-dev:9"])

    def test_explicit_replay_ingests_a_preexisting_inbox(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inbox = root / "inbox.v2.jsonl"
            store = AttentionStore(root / "state.sqlite3")
            append(inbox, delivery(7))

            self.assertEqual(
                store.initialize_ingestion_source(
                    inbox, "reviewer", replay_existing=True
                ),
                0,
            )
            self.assertEqual(
                store.ingest_inbox(inbox, "reviewer")["inserted"], 1
            )

    def test_ingestion_is_idempotent_and_cursor_commits_with_attention(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inbox = root / "inbox.jsonl"
            store = AttentionStore(root / "state.sqlite3")
            append(inbox, delivery(10))

            first = store.ingest_inbox(inbox, "reviewer")
            second = store.ingest_inbox(inbox, "reviewer")

            self.assertEqual(first["inserted"], 1)
            self.assertEqual(second["inserted"], 0)
            self.assertEqual(len(store.snapshot("reviewer")["attentions"]), 1)
            with store.connect() as connection:
                source = connection.execute(
                    "SELECT offset FROM ingestion_sources"
                ).fetchone()
            self.assertEqual(source["offset"], inbox.stat().st_size)

    def test_same_room_delivery_is_independent_for_two_local_identities(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            first_inbox = root / "first.jsonl"
            second_inbox = root / "second.jsonl"
            store = AttentionStore(root / "state.sqlite3")
            first = delivery(10, identity="first-agent")
            second = delivery(10, identity="second-agent")
            append(first_inbox, first)
            append(second_inbox, second)

            self.assertEqual(
                store.ingest_inbox(first_inbox, "first-agent")["inserted"],
                1,
            )
            self.assertEqual(
                store.ingest_inbox(second_inbox, "second-agent")["inserted"],
                1,
            )
            self.assertEqual(
                len(store.snapshot("first-agent")["attentions"]), 1
            )
            self.assertEqual(
                len(store.snapshot("second-agent")["attentions"]), 1
            )

    def test_same_identity_delivery_is_scoped_by_server_origin(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prod_inbox = root / "prod.jsonl"
            local_inbox = root / "local.jsonl"
            store = AttentionStore(root / "state.sqlite3")
            prod = delivery(11)
            local = delivery(11)
            local["server"] = "http://127.0.0.1:8787"
            append(prod_inbox, prod)
            append(local_inbox, local)

            store.ingest_inbox(prod_inbox, "reviewer")
            store.ingest_inbox(local_inbox, "reviewer")

            attentions = store.snapshot("reviewer")["attentions"]
            self.assertEqual(len(attentions), 2)
            self.assertEqual(len({row["attention_id"] for row in attentions}), 2)

    def test_partial_tail_is_not_materialized_or_acknowledged(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inbox = root / "inbox.jsonl"
            store = AttentionStore(root / "state.sqlite3")
            raw = json.dumps(delivery(11)).encode()
            inbox.write_bytes(raw)
            result = store.ingest_inbox(inbox, "reviewer")
            self.assertEqual(result["inserted"], 0)
            self.assertEqual(result["offset"], 0)

            with inbox.open("ab") as handle:
                handle.write(b"\n")
            result = store.ingest_inbox(inbox, "reviewer")
            self.assertEqual(result["inserted"], 1)

    def test_lead_room_updates_context_without_creating_attention(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inbox = root / "inbox.jsonl"
            store = AttentionStore(root / "state.sqlite3")
            append(inbox, delivery(12, priority="lead_room"))
            result = store.ingest_inbox(inbox, "reviewer")
            self.assertEqual(result["context_only"], 1)
            self.assertEqual(store.snapshot("reviewer")["attentions"], [])
            with store.connect() as connection:
                watermark = connection.execute(
                    "SELECT last_id FROM context_watermarks"
                ).fetchone()
            self.assertEqual(watermark["last_id"], 12)

    def test_one_hundred_lead_messages_advance_context_without_model_work(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inbox = root / "inbox.jsonl"
            store = AttentionStore(root / "state.sqlite3")
            for message_id in range(100, 200):
                append(inbox, delivery(message_id, priority="lead_room"))

            result = store.ingest_inbox(inbox, "reviewer")

            self.assertEqual(result["context_only"], 100)
            self.assertEqual(result["inserted"], 0)
            self.assertEqual(store.snapshot("reviewer")["attentions"], [])
            with store.connect() as connection:
                watermark = connection.execute(
                    "SELECT last_id FROM context_watermarks"
                ).fetchone()
            self.assertEqual(watermark["last_id"], 199)

    def test_all_direct_messages_are_preserved_fifo(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inbox = root / "inbox.jsonl"
            store = AttentionStore(root / "state.sqlite3")
            for message_id in range(20, 30):
                append(inbox, delivery(message_id, text=f"message {message_id}"))
            store.ingest_inbox(inbox, "reviewer")
            with store.connect() as connection:
                rows = connection.execute(
                    """
                    SELECT delivery_id FROM attentions
                    WHERE identity = 'reviewer'
                    ORDER BY stored_at_ms, attention_id
                    """
                ).fetchall()
            self.assertEqual(
                [row["delivery_id"] for row in rows],
                [f"sb-dev:{message_id}" for message_id in range(20, 30)],
            )

    def test_milestones_are_independent_and_first_is_not_final(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inbox = root / "inbox.jsonl"
            store = AttentionStore(root / "state.sqlite3")
            append(inbox, delivery(30))
            store.ingest_inbox(inbox, "reviewer")
            attention = store.next_pending("reviewer", now_ms=31_000)
            owner = "adapter-a"
            epoch = store.claim_lease("reviewer", owner, 10_000, now_ms=31_000)
            store.mark_accepted(
                attention["attention_id"],
                "thread-1",
                "turn-1",
                owner,
                epoch,
                now_ms=31_100,
            )
            store.create_relay_item(
                op_id="op-commentary",
                identity="reviewer",
                room="sb-dev",
                target="operator",
                turn_id="turn-1",
                item_id="item-1",
                phase="commentary",
                text="aldım",
                covered_attention_ids=[attention["attention_id"]],
                relay_kind="commentary",
                owner=owner,
                epoch=epoch,
                now_ms=31_200,
            )
            store.mark_relay_accepted(
                "op-commentary", owner, epoch, now_ms=31_300
            )
            store.mark_turn_completed(
                "turn-1", "completed", "", owner, epoch, now_ms=31_400
            )
            row = store.snapshot("reviewer")["attentions"][0]
            self.assertEqual(row["first_response_at_ms"], 31_300)
            self.assertIsNone(row["final_response_at_ms"])
            self.assertEqual(row["turn_completed_at_ms"], 31_400)
            self.assertIsNone(row["terminal_status"])

    def test_fencing_rejects_stale_owner_after_takeover(self):
        with tempfile.TemporaryDirectory() as tmp:
            store = AttentionStore(Path(tmp) / "state.sqlite3")
            first_epoch = store.claim_lease(
                "reviewer", "adapter-a", 100, now_ms=1_000
            )
            with self.assertRaises(LeaseBusy):
                store.claim_lease("reviewer", "adapter-b", 100, now_ms=1_050)
            second_epoch = store.claim_lease(
                "reviewer", "adapter-b", 100, now_ms=1_101
            )
            self.assertEqual(second_epoch, first_epoch + 1)
            with self.assertRaises(StaleLease):
                store.set_thread(
                    "reviewer",
                    "https://loca.example",
                    "sb-dev",
                    "stale-thread",
                    "adapter-a",
                    first_epoch,
                    now_ms=1_102,
                )
            store.set_thread(
                "reviewer",
                "https://loca.example",
                "sb-dev",
                "fresh-thread",
                "adapter-b",
                second_epoch,
                now_ms=1_102,
            )
            self.assertEqual(
                store.get_thread(
                    "reviewer", "https://loca.example", "sb-dev"
                ),
                "fresh-thread",
            )

    def test_dead_pid_owner_is_fenced_without_waiting_for_ttl(self):
        with tempfile.TemporaryDirectory() as tmp:
            store = AttentionStore(Path(tmp) / "state.sqlite3")
            first_epoch = store.claim_lease(
                "reviewer", "99999999-dead", 60_000, now_ms=1_000
            )
            second_epoch = store.claim_lease(
                "reviewer", f"{os.getpid()}-replacement", 60_000, now_ms=1_001
            )
            self.assertEqual(second_epoch, first_epoch + 1)

    def test_invalid_record_rolls_back_offset_and_prior_insert_in_batch(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inbox = root / "inbox.jsonl"
            store = AttentionStore(root / "state.sqlite3")
            with inbox.open("w", encoding="utf-8") as handle:
                handle.write(json.dumps(delivery(40)) + "\n")
                handle.write("not-json\n")
            with self.assertRaises(ValueError):
                store.ingest_inbox(inbox, "reviewer")
            self.assertEqual(store.snapshot("reviewer")["attentions"], [])
            with store.connect() as connection:
                count = connection.execute(
                    "SELECT COUNT(*) AS count FROM ingestion_sources"
                ).fetchone()["count"]
            self.assertEqual(count, 0)

    def test_missing_thread_requeues_only_output_free_incomplete_work(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inbox = root / "inbox.jsonl"
            store = AttentionStore(root / "state.sqlite3")
            append(inbox, delivery(50))
            store.ingest_inbox(inbox, "reviewer")
            attention = store.next_pending("reviewer", now_ms=51_000)
            owner = "adapter-a"
            epoch = store.claim_lease(
                "reviewer", owner, 10_000, now_ms=51_000
            )
            store.set_thread(
                "reviewer",
                "https://loca.example",
                "sb-dev",
                "thread-missing",
                owner,
                epoch,
                now_ms=51_010,
            )
            store.mark_accepted(
                attention["attention_id"],
                "thread-missing",
                "turn-missing",
                owner,
                epoch,
                now_ms=51_020,
            )

            count = store.requeue_missing_thread(
                "reviewer",
                "thread-missing",
                "thread not found",
                owner,
                epoch,
                now_ms=51_030,
            )

            self.assertEqual(count, 1)
            pending = store.next_pending("reviewer", now_ms=51_031)
            self.assertEqual(pending["attention_id"], attention["attention_id"])
            self.assertIsNone(pending["accepted_at_ms"])
            self.assertEqual(
                store.get_thread(
                    "reviewer", "https://loca.example", "sb-dev"
                ),
                "",
            )
            with store.connect() as connection:
                turn = connection.execute(
                    "SELECT terminal_status FROM turns WHERE turn_id = ?",
                    ("turn-missing",),
                ).fetchone()
            self.assertEqual(turn["terminal_status"], "failed")

    def test_missing_turn_requeues_only_after_grace_and_never_replays_output(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inbox = root / "inbox.jsonl"
            store = AttentionStore(root / "state.sqlite3")
            append(inbox, delivery(55))
            store.ingest_inbox(inbox, "reviewer")
            attention = store.next_pending("reviewer", now_ms=55_000)
            owner = "adapter-a"
            epoch = store.claim_lease(
                "reviewer", owner, 20_000, now_ms=55_000
            )
            store.mark_accepted(
                attention["attention_id"],
                "thread-1",
                "turn-1",
                owner,
                epoch,
                now_ms=55_100,
            )

            self.assertEqual(
                store.incomplete_turn_ids(
                    "reviewer", "thread-1", older_than_ms=55_099
                ),
                [],
            )
            self.assertEqual(
                store.incomplete_turn_ids(
                    "reviewer", "thread-1", older_than_ms=55_100
                ),
                ["turn-1"],
            )

            store.create_relay_item(
                op_id="op-output",
                identity="reviewer",
                room="sb-dev",
                target="operator",
                turn_id="turn-1",
                item_id="item-1",
                phase="commentary",
                text="working",
                covered_attention_ids=[attention["attention_id"]],
                relay_kind="commentary",
                owner=owner,
                epoch=epoch,
                now_ms=55_200,
            )
            self.assertEqual(
                store.requeue_missing_turn(
                    "reviewer",
                    "thread-1",
                    "turn-1",
                    "missing",
                    owner,
                    epoch,
                    now_ms=55_300,
                ),
                0,
            )
            row = store.snapshot("reviewer")["attentions"][0]
            self.assertEqual(row["turn_id"], "turn-1")
            self.assertIsNotNone(row["accepted_at_ms"])

    def test_progress_summary_distinguishes_first_and_final_response(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inbox = root / "inbox.jsonl"
            store = AttentionStore(root / "state.sqlite3")
            append(inbox, delivery(60))
            store.ingest_inbox(inbox, "reviewer")
            before = store.progress_summary("reviewer")
            self.assertEqual(before["reply_required_pending"], 1)
            self.assertEqual(before["reply_awaiting_first"], 1)

            attention = store.next_pending("reviewer", now_ms=61_000)
            owner = "adapter-a"
            epoch = store.claim_lease(
                "reviewer", owner, 10_000, now_ms=61_000
            )
            store.mark_accepted(
                attention["attention_id"],
                "thread-1",
                "turn-1",
                owner,
                epoch,
                now_ms=61_010,
            )
            store.create_relay_item(
                op_id="op-first",
                identity="reviewer",
                room="sb-dev",
                target="operator",
                turn_id="turn-1",
                item_id="commentary",
                phase="commentary",
                text="aldım",
                covered_attention_ids=[attention["attention_id"]],
                relay_kind="commentary",
                owner=owner,
                epoch=epoch,
                now_ms=61_020,
            )
            store.mark_relay_accepted(
                "op-first", owner, epoch, now_ms=61_030
            )
            after_first = store.progress_summary("reviewer")
            self.assertEqual(after_first["reply_required_pending"], 1)
            self.assertEqual(after_first["reply_awaiting_first"], 0)

    # FIX 3: a rejected turn/start (or steer) used to re-drive the same
    # attention at ~1/sec forever. Deferral now backs off exponentially and
    # gives up at a ceiling instead of churning.
    def test_pre_acceptance_defer_backs_off_then_gives_up_at_ceiling(self):
        from attention_store import DEFER_MAX_ATTEMPTS, DEFER_MAX_DELAY_MS

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inbox = root / "inbox.jsonl"
            store = AttentionStore(root / "state.sqlite3")
            append(inbox, delivery(40))
            store.ingest_inbox(inbox, "reviewer")
            attention = store.next_pending("reviewer", now_ms=1_000)
            self.assertIsNotNone(attention)
            aid = attention["attention_id"]
            owner = "adapter-a"
            epoch = store.claim_lease(
                "reviewer", owner, 10_000_000, now_ms=1_000
            )

            delays = []
            for i in range(DEFER_MAX_ATTEMPTS - 1):
                now = 10_000 + i
                store.defer_attention(
                    aid, f"turn/start rejected #{i}", 1_000, owner, epoch,
                    now_ms=now,
                )
                row = store.snapshot("reviewer")["attentions"][0]
                self.assertIsNone(row["terminal_status"])
                self.assertEqual(row["attempts"], i + 1)
                delays.append(row["next_attempt_at_ms"] - now)

            # Exponential, non-decreasing, capped — not a flat ~1s retry.
            self.assertEqual(delays[0], 1_000)
            self.assertEqual(delays[1], 2_000)
            self.assertTrue(all(b >= a for a, b in zip(delays, delays[1:])))
            self.assertLessEqual(max(delays), DEFER_MAX_DELAY_MS)
            self.assertGreater(max(delays), delays[0])
            # Still retriable while below the ceiling.
            self.assertIsNotNone(store.next_pending("reviewer", now_ms=10_000_000))

            # The ceiling deferral gives up (degraded) instead of churning.
            store.defer_attention(
                aid, "turn/start rejected final", 1_000, owner, epoch,
                now_ms=99_000,
            )
            row = store.snapshot("reviewer")["attentions"][0]
            self.assertEqual(row["terminal_status"], "failed")
            self.assertIn("gave up", row["terminal_reason"])
            # No longer re-dispatched.
            self.assertIsNone(store.next_pending("reviewer", now_ms=10_000_000))

    def test_immediate_defer_does_not_consume_the_retry_budget(self):
        # delay_ms <= 0 is a deliberate immediate re-dispatch (a steer that
        # recovered from an inactive turn), not churn: it must not count toward
        # the ceiling or ever mark the attention degraded.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inbox = root / "inbox.jsonl"
            store = AttentionStore(root / "state.sqlite3")
            append(inbox, delivery(41))
            store.ingest_inbox(inbox, "reviewer")
            attention = store.next_pending("reviewer", now_ms=1_000)
            aid = attention["attention_id"]
            owner = "adapter-a"
            epoch = store.claim_lease("reviewer", owner, 10_000, now_ms=1_000)

            for i in range(25):
                store.defer_attention(
                    aid, "recovered inactive turn", 0, owner, epoch,
                    now_ms=2_000 + i,
                )
            row = store.snapshot("reviewer")["attentions"][0]
            self.assertEqual(row["attempts"], 0)
            self.assertIsNone(row["terminal_status"])
            self.assertIsNotNone(store.next_pending("reviewer", now_ms=10_000))


if __name__ == "__main__":
    unittest.main()
