"""Building notifications reach one model context once and are never dropped.

The listener bakes the recent recipient-only window into every room attention
and keeps up to 128 notices in its cache. Codex v2 owns the model context: it
gives each thread every notice that thread is owed and has not accepted,
oldest first, a budget's worth per turn, records them in the same transaction
that records Codex's acceptance of the turn input, and releases them when that
context is compacted or Codex loses the turn. A failure anywhere before that
record shows the notice again.
"""
import json
import os
import sqlite3
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

SKILL_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SKILL_DIR))
sys.path.insert(0, str(Path(__file__).resolve().parent))

import test_codex_adapter_v2 as adapter_tests  # noqa: E402
from attention_store import AttentionStore  # noqa: E402
from codex_adapter_v2 import PersistentCodexAdapter  # noqa: E402
import notification_state  # noqa: E402

FIXTURES = Path(__file__).resolve().parent / "fixtures"
# Realistic server clock (milliseconds since the epoch) for notice timestamps.
CREATED_AT = 1_763_100_000_000


def notice(message_id):
    return {"id": message_id, "title": f"notice-{message_id}", "body": f"body-{message_id}",
            "from": "operator", "created_at": CREATED_AT + message_id, "scope": "building"}


def attention(message_id, *notice_ids, more=False):
    record = adapter_tests.delivery(message_id)
    if notice_ids:
        record["event"]["notification_context"] = {
            "recipient": "reviewer", "items": [notice(n) for n in notice_ids], "more_available": more,
        }
    return record


def restart(fixture, root, cache=None):
    fixture.adapter.close()
    fixture.adapter = PersistentCodexAdapter(
        store=AttentionStore(root / "state.sqlite3"), inbox=fixture.inbox, identity="reviewer",
        workdir=root, codex_bin="codex", relay_mode="live", relay=fixture.relay,
        context_provider=lambda _attention: [], app_server_factory=lambda _bin, _timeout: fixture.fake,
        **({"notification_cache": cache} if cache is not None else {}))
    fixture.adapter.initialize()
    return fixture.adapter


class ListenerCache:
    """The listener's real cache file, filled through its recovery path."""

    def __init__(self, root):
        self.path = root / "cursor.json.notifications.sqlite3"
        self.cache = notification_state.NotificationCache(
            self.path, "https://loca.example", "reviewer", "fixture-membership")

    def receive(self, *ids):
        rows = [{**notice(n), "audience": {"all": True}} for n in ids]
        page = {"order": "asc", "notifications": rows, "more": False}
        with mock.patch.object(notification_state, "fetch_page", return_value=page):
            self.cache.recover_page("fixture-membership")


class SimulatedCrash(BaseException):
    """The process dies; nothing after this point runs."""


class NotificationContextOnceTests(unittest.TestCase):
    def prompts(self, fixture):
        return [params["input"][0]["text"] for method, params in fixture.fake.requests
                if method in ("turn/start", "turn/steer")]

    def packet(self, prompt):
        if "Building announcements" not in prompt:
            return None
        raw = prompt.split("Building announcements", 1)[1].split("\n", 1)[1]
        raw = raw.split("\n\nRespond as normal", 1)[0]
        try:
            return json.loads(raw)
        except ValueError:
            self.fail("the announcement packet was cut in the middle of a notice")

    def shown_ids(self, prompt):
        packet = self.packet(prompt)
        return [item["id"] for item in packet["items"]] if packet else []

    def turn(self, fixture, message_id, *notice_ids, more=False):
        """One idle turn: ingest, dispatch, complete. Returns the ids shown."""
        record = attention(message_id, *notice_ids, more=more)
        fixture.ingest(record)
        self.assertTrue(fixture.adapter.dispatch_one())
        turn_id = fixture.adapter.active_turns["thread-1"]
        fixture.adapter.handle_turn_completed("thread-1", {"id": turn_id, "status": "completed"})
        return self.shown_ids(self.prompts(fixture)[-1])

    def complete(self, fixture, turn="turn-1"):
        fixture.adapter.handle_turn_completed("thread-1", {"id": turn, "status": "completed"})

    def test_consecutive_turns_and_steers_do_not_repeat_a_shown_notice(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = adapter_tests.AdapterFixture(Path(tmp))
            fixture.ingest(attention(400, 9))
            self.assertTrue(fixture.adapter.dispatch_one())
            fixture.ingest(attention(401, 9))          # steered into the active turn
            self.assertTrue(fixture.adapter.dispatch_one())
            self.complete(fixture)
            fixture.ingest(attention(402, 9))          # next idle turn
            self.assertTrue(fixture.adapter.dispatch_one())
            prompts = self.prompts(fixture)
            self.assertEqual(len(prompts), 3)
            self.assertIn("notice-9", prompts[0])
            self.assertNotIn("notice-9", prompts[1], "steer repeated a notice the turn already holds")
            self.assertNotIn("notice-9", prompts[2], "next turn repeated a notice the thread already holds")
            self.assertNotIn("Building announcements", prompts[2])
            fixture.adapter.close()

    def test_two_notices_in_one_turn_are_both_shown_then_only_a_new_one(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = adapter_tests.AdapterFixture(Path(tmp))
            fixture.ingest(attention(400, 9, 10))
            self.assertTrue(fixture.adapter.dispatch_one())
            self.complete(fixture)
            fixture.ingest(attention(401, 9, 10, 11))
            self.assertTrue(fixture.adapter.dispatch_one())
            first, second = self.prompts(fixture)
            self.assertIn("notice-9", first)
            self.assertIn("notice-10", first)
            self.assertIn("notice-11", second)
            self.assertNotIn("notice-9", second)
            self.assertNotIn("notice-10", second)
            fixture.adapter.close()

    def test_rejected_turn_does_not_mark_the_notice_shown(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = adapter_tests.AdapterFixture(Path(tmp))
            original = fixture.fake.request
            fixture.fake.request = lambda method, params: (
                {"error": {"message": "unavailable"}} if method == "turn/start" else original(method, params))
            fixture.ingest(attention(400, 9))
            self.assertFalse(fixture.adapter.dispatch_one())
            fixture.fake.request = original
            with fixture.store.connect() as connection:
                connection.execute("UPDATE attentions SET next_attempt_at_ms = 0")
            self.assertTrue(fixture.adapter.dispatch_one())
            accepted = [params["input"][0]["text"] for method, params in fixture.fake.requests
                        if method == "turn/start"]
            self.assertEqual(len(accepted), 1)
            self.assertIn("notice-9", accepted[0])
            fixture.adapter.close()

    def test_crash_between_codex_acceptance_and_the_record_shows_the_notice_again(self):
        # Simulated at the function boundary: Codex accepted turn/start, then
        # the process dies inside the durable record of that acceptance.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fixture = adapter_tests.AdapterFixture(root)
            fixture.ingest(attention(400, 9))
            with mock.patch.object(AttentionStore, "finalize_dispatch", side_effect=SimulatedCrash):
                with self.assertRaises(SimulatedCrash):
                    fixture.adapter.dispatch_one()
            self.assertEqual(sum(m == "turn/start" for m, _ in fixture.fake.requests), 1)
            adapter = restart(fixture, root)
            # Recovery binds the attention to the turn Codex accepted rather
            # than running it twice; it cannot prove the notice was recorded.
            row = adapter.store.snapshot("reviewer")["attentions"][0]
            self.assertEqual(row["turn_id"], "turn-1")
            self.assertFalse(adapter.dispatch_one())
            self.complete(fixture)
            fixture.ingest(attention(401, 9))
            self.assertTrue(adapter.dispatch_one())
            self.assertIn("notice-9", self.prompts(fixture)[-1])
            adapter.close()

    def test_restart_with_the_same_thread_does_not_repeat_a_recorded_notice(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fixture = adapter_tests.AdapterFixture(root)
            fixture.ingest(attention(400, 9))
            self.assertTrue(fixture.adapter.dispatch_one())
            self.complete(fixture)
            adapter = restart(fixture, root)
            fixture.ingest(attention(401, 9, 10))
            self.assertTrue(adapter.dispatch_one())
            prompt = self.prompts(fixture)[-1]
            self.assertNotIn("notice-9", prompt)
            self.assertIn("notice-10", prompt)
            adapter.close()

    def test_compaction_returns_the_notice_to_the_next_turn(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = adapter_tests.AdapterFixture(Path(tmp))
            fixture.ingest(attention(400, 9))
            self.assertTrue(fixture.adapter.dispatch_one())
            fixture.adapter.handle_message({"method": "item/completed", "params": {
                "threadId": "thread-1", "turnId": "turn-1",
                "item": {"type": "contextCompaction", "id": "compact-1"}}})
            self.complete(fixture)
            fixture.ingest(attention(401, 9))
            self.assertTrue(fixture.adapter.dispatch_one())
            self.assertIn("notice-9", self.prompts(fixture)[-1])
            fixture.adapter.close()

    def test_compaction_seen_only_in_recovered_history_returns_the_notice(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fixture = adapter_tests.AdapterFixture(root)
            fixture.ingest(attention(400, 9))
            self.assertTrue(fixture.adapter.dispatch_one())
            # The adapter died before it read the compaction and the turn's end.
            turn = fixture.fake.threads["thread-1"][0]
            turn["items"].append({"type": "contextCompaction", "id": "compact-1"})
            turn["status"] = "completed"
            adapter = restart(fixture, root)
            fixture.ingest(attention(401, 9))
            self.assertTrue(adapter.dispatch_one())
            self.assertIn("notice-9", self.prompts(fixture)[-1])
            adapter.close()

    def test_a_new_thread_is_a_new_context_and_gets_the_notice(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fixture = adapter_tests.AdapterFixture(root)
            fixture.ingest(attention(400, 9))
            self.assertTrue(fixture.adapter.dispatch_one())
            self.complete(fixture)
            del fixture.fake.threads["thread-1"]      # Codex lost the session
            adapter = restart(fixture, root)
            fixture.ingest(attention(401, 9))
            self.assertTrue(adapter.dispatch_one())
            starts = [params for method, params in fixture.fake.requests if method == "turn/start"]
            self.assertEqual(starts[-1]["threadId"], "thread-2")
            self.assertIn("notice-9", starts[-1]["input"][0]["text"])
            adapter.close()

    def test_another_room_thread_is_another_context(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = adapter_tests.AdapterFixture(Path(tmp))
            fixture.ingest(attention(400, 9))
            self.assertTrue(fixture.adapter.dispatch_one())
            other = attention(401, 9)
            other["room"] = other["event"]["room"] = "other-room"
            other["delivery_id"] = "other-room:401"
            fixture.ingest(other)
            self.assertTrue(fixture.adapter.dispatch_one())
            self.assertIn("notice-9", self.prompts(fixture)[-1])
            fixture.adapter.close()

    def test_oversized_window_keeps_what_did_not_fit_owed_even_when_newer_arrive(self):
        def long(record):
            for item in record["event"]["notification_context"]["items"]:
                item["body"] = "\n" * 1990     # valid (<= 2000), but escapes to ~4 KB each
            return record
        with tempfile.TemporaryDirectory() as tmp:
            fixture = adapter_tests.AdapterFixture(Path(tmp))
            shown = []
            fixture.ingest(long(attention(400, 1, 2, 3, 4)))
            self.assertTrue(fixture.adapter.dispatch_one())
            shown.append(self.shown_ids(self.prompts(fixture)[-1]))
            self.complete(fixture)
            self.assertEqual(shown[0], [1, 2], "oldest first, a budget's worth")
            self.assertEqual(self.packet(self.prompts(fixture)[-1])["pending_after"], 2)
            self.assertIn("2 more follow on later turns", self.prompts(fixture)[-1])
            # Notice 5 arrives before the next turn; the window moves to 2..5.
            for message_id in (401, 402, 403):
                record = long(attention(message_id, 2, 3, 4, 5))
                fixture.ingest(record)
                self.assertTrue(fixture.adapter.dispatch_one())
                shown.append(self.shown_ids(self.prompts(fixture)[-1]))
                fixture.adapter.handle_turn_completed(
                    "thread-1", {"id": fixture.adapter.active_turns["thread-1"], "status": "completed"})
            flat = [n for ids in shown for n in ids]
            self.assertEqual(flat, [1, 2, 3, 4, 5], "every notice once, oldest first, none skipped")
            self.assertEqual(shown[-1], [])
            fixture.adapter.close()

    def test_burst_of_six_while_idle_is_offered_in_full_from_the_listener_cache(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            cache = ListenerCache(root)
            fixture = adapter_tests.AdapterFixture(root)
            restart(fixture, root, cache.path)
            cache.receive(10)
            self.assertEqual(self.turn(fixture, 400, 10), [10])
            cache.receive(11, 12, 13, 14, 15, 16)          # idle: no room turn
            # The attention carries only the newest four; the cache has all.
            self.assertEqual(self.turn(fixture, 401, 13, 14, 15, 16, more=True), [11, 12, 13, 14])
            self.assertIn("2 more follow on later turns", self.prompts(fixture)[-1])
            self.assertEqual(self.turn(fixture, 402, 13, 14, 15, 16, more=True), [15, 16])
            self.assertEqual(self.turn(fixture, 403, 13, 14, 15, 16, more=True), [])
            fixture.adapter.close()

    def test_burst_without_a_readable_cache_says_older_ones_exist(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = adapter_tests.AdapterFixture(Path(tmp))
            self.assertEqual(self.turn(fixture, 400, 10), [10])
            # Only the newest-four window is known: 11 and 12 cannot be shown.
            self.assertEqual(self.turn(fixture, 401, 13, 14, 15, 16, more=True), [13, 14, 15, 16])
            prompt = self.prompts(fixture)[-1]
            self.assertTrue(self.packet(prompt)["older_unavailable"])
            self.assertIn("Older announcements for you exist that were not kept locally; "
                          "read them with connect.sh notifications", prompt)
            # Said once: the next turn does not repeat the notice or the gap.
            self.assertEqual(self.turn(fixture, 402, 13, 14, 15, 16, more=True), [])
            self.assertNotIn("Building announcements", self.prompts(fixture)[-1])
            fixture.adapter.close()

    def test_rotated_cache_says_older_ones_exist_instead_of_skipping(self):
        with tempfile.TemporaryDirectory() as tmp, \
                mock.patch.object(notification_state, "CACHE_LIMIT", 6):
            root = Path(tmp)
            cache = ListenerCache(root)
            fixture = adapter_tests.AdapterFixture(root)
            restart(fixture, root, cache.path)
            cache.receive(10)
            self.assertEqual(self.turn(fixture, 400, 10), [10])
            cache.receive(*range(11, 21))                   # cache keeps 15..20
            self.assertEqual(self.turn(fixture, 401, 17, 18, 19, 20, more=True), [15, 16, 17, 18])
            prompt = self.prompts(fixture)[-1]
            self.assertTrue(self.packet(prompt)["older_unavailable"])
            self.assertIn("not kept locally", prompt)
            self.assertEqual(self.turn(fixture, 402, 17, 18, 19, 20, more=True), [19, 20])
            self.assertNotIn("not kept locally", self.prompts(fixture)[-1])
            fixture.adapter.close()

    def test_cache_rotating_past_given_notices_is_not_a_gap(self):
        # Every notice was given in order; the cache rotating past them must
        # not tell the model that older ones were not kept.
        with tempfile.TemporaryDirectory() as tmp, \
                mock.patch.object(notification_state, "CACHE_LIMIT", 6):
            root = Path(tmp)
            cache = ListenerCache(root)
            fixture = adapter_tests.AdapterFixture(root)
            restart(fixture, root, cache.path)
            for n in range(1, 11):
                cache.receive(n)
                window = range(max(1, n - 3), n + 1)
                self.assertEqual(self.turn(fixture, 400 + n, *window, more=n > 4), [n])
                self.assertNotIn("not kept locally", self.prompts(fixture)[-1], f"notice {n}")
            fixture.adapter.close()

    def given_one_by_one(self, fixture, cache, last):
        for n in range(1, last + 1):
            cache.receive(n)
            self.assertEqual(self.turn(fixture, 400 + n, *range(max(1, n - 3), n + 1), more=n > 4), [n])

    def test_turn_lost_after_the_floor_moved_past_its_notice_offers_it_again(self):
        with tempfile.TemporaryDirectory() as tmp, \
                mock.patch.object(notification_state, "CACHE_LIMIT", 6):
            root = Path(tmp)
            cache = ListenerCache(root)
            fixture = adapter_tests.AdapterFixture(root)
            restart(fixture, root, cache.path)
            self.given_one_by_one(fixture, cache, 6)
            cache.receive(7)
            fixture.ingest(attention(407, 4, 5, 6, 7, more=True))
            self.assertTrue(fixture.adapter.dispatch_one())        # accepted and recorded
            self.assertEqual(self.shown_ids(self.prompts(fixture)[-1]), [7])
            history = fixture.fake.threads["thread-1"]
            lost = history.pop()                                  # Codex never persisted it
            for turn in history:
                turn["status"] = "completed"
            with fixture.store.connect() as connection:
                connection.execute("UPDATE turns SET started_at_ms = started_at_ms - 61000 "
                                   "WHERE turn_id = ?", (lost["id"],))
            adapter = restart(fixture, root, cache.path)
            self.assertTrue(adapter.dispatch_one())
            prompt = self.prompts(fixture)[-1]
            self.assertEqual(self.shown_ids(prompt), [7], "the lost turn's notice, again")
            self.assertNotIn("not kept locally", prompt)
            adapter.close()

    def test_compaction_after_the_floor_moved_offers_the_recent_notices_again(self):
        with tempfile.TemporaryDirectory() as tmp, \
                mock.patch.object(notification_state, "CACHE_LIMIT", 6):
            root = Path(tmp)
            cache = ListenerCache(root)
            fixture = adapter_tests.AdapterFixture(root)
            restart(fixture, root, cache.path)
            self.given_one_by_one(fixture, cache, 7)
            turn_id = f"turn-{fixture.fake.next_turn - 1}"
            fixture.adapter.handle_message({"method": "item/completed", "params": {
                "threadId": "thread-1", "turnId": turn_id,
                "item": {"type": "contextCompaction", "id": "compact-1"}}})
            cache.receive(8)
            self.assertEqual(self.turn(fixture, 408, 5, 6, 7, 8, more=True), [4, 5, 6, 7])
            prompt = self.prompts(fixture)[-1]
            self.assertIn("1 more follow on later turns", prompt)
            self.assertNotIn("not kept locally", prompt)
            self.assertEqual(self.turn(fixture, 409, 5, 6, 7, 8, more=True), [8])
            self.assertEqual(self.turn(fixture, 410, 5, 6, 7, 8, more=True), [])
            fixture.adapter.close()

    def test_turn_reaped_after_recovery_lowers_the_floor_to_its_notice(self):
        # Store level: a stale-epoch turn absent after recovery held notice 7;
        # the floor had already moved past it.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            store = AttentionStore(root / "state.sqlite3")
            inbox = root / "inbox.jsonl"
            adapter_tests.append(inbox, attention(407, 7))
            store.ingest_inbox(inbox, "reviewer")
            now = CREATED_AT + 60_000
            first = store.claim_lease("reviewer", "adapter-a", 100_000, now_ms=now)
            pending = store.next_pending("reviewer", now_ms=now + 1)
            store.mark_accepted(pending["attention_id"], "thread-1", "turn-stale",
                                "adapter-a", first, now_ms=now + 2)
            with store.connect() as connection:
                connection.executemany(
                    "INSERT INTO notification_context VALUES ('reviewer', 'thread-1', ?, ?, ?)",
                    [(6, "turn-earlier", now), (7, "turn-stale", now + 2)])
                connection.execute(
                    "INSERT INTO notification_threads VALUES ('reviewer', 'thread-1', 8)")
            second = store.claim_lease("reviewer", "adapter-b", 100_000, now_ms=now + 200_000)
            self.assertEqual(store.reap_stale_turns_after_recovery(
                "reviewer", "adapter-b", second, now_ms=now + 200_001), 1)
            self.assertEqual(store.notification_state("reviewer", "thread-1"), (7, {6}))

    def test_compaction_read_with_a_steer_response_is_not_undone_by_its_record(self):
        # The steer was planned (floor past 1..4), then Codex reports a
        # compaction before the steer response; the adapter handles it,
        # releasing 1..4, and only then records the steer.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            cache = ListenerCache(root)
            fixture = adapter_tests.AdapterFixture(root)
            restart(fixture, root, cache.path)
            cache.receive(1, 2, 3, 4)
            fixture.ingest(attention(401, 1, 2, 3, 4))
            self.assertTrue(fixture.adapter.dispatch_one())       # turn-1, still running
            self.assertEqual(self.shown_ids(self.prompts(fixture)[-1]), [1, 2, 3, 4])
            cache.receive(5)
            fixture.fake.before_steer_response = [{"method": "item/completed", "params": {
                "threadId": "thread-1", "turnId": "turn-1",
                "item": {"type": "contextCompaction", "id": "compact-1"}}}]
            fixture.ingest(attention(402, 2, 3, 4, 5, more=True))
            self.assertTrue(fixture.adapter.dispatch_one())       # steered into turn-1
            self.assertEqual(self.shown_ids(self.prompts(fixture)[-1]), [5])
            self.complete(fixture)
            self.assertEqual(self.turn(fixture, 403, 2, 3, 4, 5, more=True), [1, 2, 3, 4],
                             "the compaction released 1..4; the steer's record must not skip them")
            fixture.adapter.close()

    def release_between_plan_and_record(self, release):
        """Store level: plan a dispatch, release an earlier turn, record it."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            store = AttentionStore(root / "state.sqlite3")
            inbox = root / "inbox.jsonl"
            adapter_tests.append(inbox, attention(401, 1, 2, 3, 4))
            adapter_tests.append(inbox, attention(402, 2, 3, 4, 5, more=True))
            store.ingest_inbox(inbox, "reviewer")
            now = CREATED_AT + 60_000
            epoch = store.claim_lease("reviewer", "adapter-a", 100_000, now_ms=now)
            first = store.next_pending("reviewer", now_ms=now + 1)
            store.prepare_dispatch(first["attention_id"], "thread-1", "start", "c-1",
                                   "adapter-a", epoch, now_ms=now + 2)
            store.finalize_dispatch(first["attention_id"], "thread-1", "turn-1", "adapter-a", epoch,
                                    notification_ids=[1, 2, 3, 4], notification_floor=5,
                                    now_ms=now + 3)
            owner, epoch = release(store, "plan", now, epoch)
            second = store.next_pending("reviewer", now_ms=now + 300_002)
            self.assertEqual(second["delivery_id"], "sb-dev:402")
            rows = [notice(n) for n in range(1, 6)]
            floor, accepted = store.notification_state("reviewer", "thread-1")
            packet, planned = notification_state.plan_context(rows, floor, accepted, True)
            self.assertEqual((notification_state.context_ids(packet), planned), ([5], 6))
            store.prepare_dispatch(second["attention_id"], "thread-1", "start", "c-2",
                                   owner, epoch, now_ms=now + 300_003)
            release(store, "between", now, epoch)
            store.finalize_dispatch(second["attention_id"], "thread-1", "turn-2", owner, epoch,
                                    notification_ids=notification_state.context_ids(packet),
                                    notification_floor=planned,
                                    notification_accepted=accepted, now_ms=now + 300_005)
            floor, accepted = store.notification_state("reviewer", "thread-1")
            packet, _ = notification_state.plan_context(rows, floor, accepted, True)
            self.assertEqual(notification_state.context_ids(packet), [1, 2, 3, 4],
                             "turn-1 was released; recording turn-2 must not skip its notices")

    def test_lost_turn_released_between_plan_and_record_is_offered_again(self):
        def release(store, stage, now, epoch):
            if stage == "plan":                    # the same adapter, its lease renewed
                return "adapter-a", store.claim_lease("reviewer", "adapter-a", 100_000,
                                                      now_ms=now + 300_000)
            store.requeue_missing_turn("reviewer", "thread-1", "turn-1", "lost",
                                       "adapter-a", epoch, now_ms=now + 300_004)
            return "adapter-a", epoch
        self.release_between_plan_and_record(release)

    def test_turn_reaped_between_plan_and_record_is_offered_again(self):
        state = {}

        def release(store, stage, now, epoch):
            if stage == "plan":
                # A new adapter epoch; turn-1 of the old one is now stale.
                state["epoch"] = store.claim_lease("reviewer", "adapter-b", 100_000,
                                                   now_ms=now + 300_000)
                return "adapter-b", state["epoch"]
            store.reap_stale_turns_after_recovery("reviewer", "adapter-b", state["epoch"],
                                                  now_ms=now + 300_004)
            return "adapter-b", state["epoch"]
        self.release_between_plan_and_record(release)

    def test_closed_room_forgets_its_thread_notification_records(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            cache = ListenerCache(root)
            fixture = adapter_tests.AdapterFixture(root)
            restart(fixture, root, cache.path)
            self.given_one_by_one(fixture, cache, 2)
            fixture.ingest(adapter_tests.room_closed_delivery(410))
            fixture.adapter.dispatch_one()
            with fixture.store.connect() as connection:
                self.assertEqual(connection.execute("SELECT COUNT(*) FROM threads").fetchone()[0], 0)
                left = [connection.execute(f"SELECT COUNT(*) FROM {table}").fetchone()[0]
                        for table in ("notification_context", "notification_threads")]
            self.assertEqual(left, [0, 0], "a closed room's thread keeps no notification records")
            fixture.adapter.close()

    def test_accepted_notice_records_stay_bounded_per_thread(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            cache = ListenerCache(root)
            fixture = adapter_tests.AdapterFixture(root)
            restart(fixture, root, cache.path)
            self.given_one_by_one(fixture, cache, 70)
            with fixture.store.connect() as connection:
                kept = connection.execute(
                    "SELECT COUNT(*), MIN(notification_id) FROM notification_context").fetchone()
            self.assertEqual(tuple(kept), (16, 55), "the 16 most recent records")
            fixture.adapter.close()

    def test_turn_codex_lost_after_the_record_returns_its_notice_on_retry(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fixture = adapter_tests.AdapterFixture(root)
            fixture.ingest(attention(400, 9))
            self.assertTrue(fixture.adapter.dispatch_one())       # accepted and recorded
            fixture.fake.threads["thread-1"] = []                 # Codex never persisted it
            with fixture.store.connect() as connection:
                connection.execute("UPDATE turns SET started_at_ms = started_at_ms - 61000")
            adapter = restart(fixture, root)
            row = adapter.store.snapshot("reviewer")["attentions"][0]
            self.assertIn("absent from durable thread history", row["terminal_reason"])
            self.assertTrue(adapter.dispatch_one())
            starts = [params for method, params in fixture.fake.requests if method == "turn/start"]
            self.assertEqual(starts[-1]["threadId"], "thread-1", "same thread, retried input")
            self.assertIn("notice-9", starts[-1]["input"][0]["text"])
            adapter.close()

    def test_turn_absent_from_history_with_output_releases_its_notice(self):
        # Codex ran the turn (it produced output) but its history lost it:
        # the resumed context does not hold the notice either.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fixture = adapter_tests.AdapterFixture(root)
            fixture.ingest(attention(400, 9))
            self.assertTrue(fixture.adapter.dispatch_one())
            fixture.adapter.handle_agent_item("thread-1", "turn-1", {
                "id": "final-1", "type": "agentMessage", "phase": "final_answer", "text": "done"})
            fixture.fake.threads["thread-1"] = []
            with fixture.store.connect() as connection:
                connection.execute("UPDATE turns SET started_at_ms = started_at_ms - 61000")
            adapter = restart(fixture, root)
            fixture.ingest(attention(401, 9))
            self.assertTrue(adapter.dispatch_one())
            self.assertIn("notice-9", self.prompts(fixture)[-1])
            adapter.close()

    def test_state_file_written_by_the_previous_kit_keeps_working(self):
        # fixtures/attention_store_0_13.sql is a dump of a state file written
        # by the 0.13 kit: thread-1 holds an accepted, completed turn, and a
        # pending attention whose event carries the old listener's packet.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            with sqlite3.connect(root / "state.sqlite3") as connection:
                connection.executescript((FIXTURES / "attention_store_0_13.sql").read_text())
            fixture = adapter_tests.AdapterFixture.__new__(adapter_tests.AdapterFixture)
            fixture.root, fixture.inbox = root, root / "inbox.jsonl"
            fixture.inbox.write_text("")
            fixture.store = AttentionStore(root / "state.sqlite3")
            fixture.fake = adapter_tests.FakeAppServer("codex", 10)
            fixture.fake.threads["thread-1"] = [{"id": "turn-1", "status": "completed", "items": []}]
            fixture.fake.next_turn = 2
            fixture.relay = adapter_tests.RelayRecorder()
            fixture.adapter = PersistentCodexAdapter(
                store=fixture.store, inbox=fixture.inbox, identity="reviewer", workdir=root,
                codex_bin="codex", relay_mode="live", relay=fixture.relay,
                context_provider=lambda _attention: [], app_server_factory=lambda _b, _t: fixture.fake)
            fixture.adapter.initialize()
            self.assertTrue(fixture.adapter.dispatch_one())
            self.assertIn("notice-9", self.prompts(fixture)[-1],
                          "the old kit kept no shown marker; the first new turn shows the window once")
            fixture.adapter.handle_turn_completed("thread-1", {"id": "turn-2", "status": "completed"})
            fixture.ingest(attention(402, 9))
            self.assertTrue(fixture.adapter.dispatch_one())
            self.assertNotIn("notice-9", self.prompts(fixture)[-1])
            fixture.adapter.close()

    def test_state_file_with_the_floor_at_the_first_notice_keeps_working(self):
        # fixtures/attention_store_9104af8.sql was written by the kit of
        # 9104af8, which kept a thread's floor at the first notice it gave:
        # thread-1 was given notices 1..128 four a turn (turn-1..turn-32),
        # floor 1, all 128 recorded.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            with sqlite3.connect(root / "state.sqlite3") as connection:
                connection.executescript((FIXTURES / "attention_store_9104af8.sql").read_text())
            cache = ListenerCache(root)
            for first in range(1, 129, 16):
                cache.receive(*range(first, first + 16))
            fixture = adapter_tests.AdapterFixture.__new__(adapter_tests.AdapterFixture)
            fixture.root, fixture.inbox = root, root / "inbox.jsonl"
            fixture.inbox.write_text("")
            fixture.store = AttentionStore(root / "state.sqlite3")
            fixture.fake = adapter_tests.FakeAppServer("codex", 10)
            fixture.fake.threads["thread-1"] = [
                {"id": f"turn-{n}", "status": "completed", "items": []} for n in range(1, 33)]
            fixture.fake.next_turn = 33
            fixture.relay = adapter_tests.RelayRecorder()
            fixture.adapter = PersistentCodexAdapter(
                store=fixture.store, inbox=fixture.inbox, identity="reviewer", workdir=root,
                codex_bin="codex", relay_mode="live", relay=fixture.relay,
                context_provider=lambda _attention: [], app_server_factory=lambda _b, _t: fixture.fake,
                notification_cache=cache.path)
            fixture.adapter.initialize()
            cache.receive(129)                     # the cache rotates past notice 1
            self.assertEqual(self.turn(fixture, 500, 126, 127, 128, 129, more=True), [129])
            self.assertNotIn("not kept locally", self.prompts(fixture)[-1])
            with fixture.store.connect() as connection:
                self.assertEqual(connection.execute(
                    "SELECT COUNT(*) FROM notification_context").fetchone()[0], 16)
            # Compaction still offers the most recent given notices again.
            fixture.adapter.handle_message({"method": "item/completed", "params": {
                "threadId": "thread-1", "turnId": "turn-33",
                "item": {"type": "contextCompaction", "id": "compact-1"}}})
            self.assertEqual(self.turn(fixture, 501, 126, 127, 128, 129, more=True), [126, 127, 128, 129])
            self.assertEqual(self.turn(fixture, 502, 126, 127, 128, 129, more=True), [])
            fixture.adapter.close()


if __name__ == "__main__":
    unittest.main()
