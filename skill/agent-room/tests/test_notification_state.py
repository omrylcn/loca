import json
import os
from pathlib import Path
import sqlite3
import sys
import tempfile
import threading
import unittest
from unittest import mock

SKILL_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SKILL_DIR))

import notification_state as notifications
from credentials import CredentialError
import test_codex_adapter_v2 as adapter_tests


def notice(message_id):
    return {"id": message_id, "title": f"notice-{message_id}", "body": "announcement text",
            "from": "Master", "created_at": message_id, "audience": {"all": True}}


class NotificationStateTests(unittest.TestCase):
    def test_unavailable_optional_cache_does_not_stall_room_delivery(self):
        with tempfile.TemporaryDirectory() as tmp:
            cache = notifications.NotificationCache(Path(tmp) / "cache.db", "http://local", "reviewer", "fixture-membership")
            with mock.patch.object(cache, "connect", side_effect=sqlite3.OperationalError("fixture lock")) as connect:
                packet = cache.context()
            self.assertEqual(packet["items"], [])
            self.assertTrue(packet["unavailable"])
            connect.assert_called_once_with(timeout=0.05)

    def test_fetch_uses_only_own_membership_and_never_follows_a_redirect(self):
        opener = mock.Mock()
        opener.open.return_value.__enter__ = mock.Mock(return_value=mock.Mock(
            read=mock.Mock(return_value=b'{"notifications":[]}')))
        opener.open.return_value.__exit__ = mock.Mock(return_value=False)
        with mock.patch.dict(os.environ, {"ROOM_SERVER_URL": "http://local", "LOCA_SESSION": "st_other_room"}), \
                mock.patch.object(notifications.urllib.request, "build_opener", return_value=opener):
            notifications.fetch_page("http://local", "mb_fixture", limit=16)
            request = opener.open.call_args.args[0]
            self.assertEqual(request.get_header("X-room-token"), "mb_fixture")
            self.assertIsNone(request.get_header("X-session-token"))
            self.assertNotIn("mb_fixture", request.full_url)
            with self.assertRaises(CredentialError):
                notifications.fetch_page("http://another-origin", "mb_fixture")
            self.assertEqual(opener.open.call_count, 1)
        self.assertIsNone(notifications.NoRedirect().redirect_request(None, None, 302, "", {}, "http://another-origin"))

    def test_failed_checkpoint_rolls_back_payload_and_is_safely_retried(self):
        with tempfile.TemporaryDirectory() as tmp:
            cache = notifications.NotificationCache(Path(tmp) / "cache.db", "http://local", "reviewer", "fixture-membership")
            with cache.connect() as connection:
                connection.execute("CREATE TRIGGER deny_checkpoint BEFORE INSERT ON checkpoint "
                    "BEGIN SELECT RAISE(ABORT, 'fixture failure'); END")
            with mock.patch.object(notifications, "fetch_page", return_value={
                "order": "asc", "notifications": [notice(1)], "more": False,
            }):
                with self.assertRaises(sqlite3.Error):
                    cache.recover_page("fixture-membership")
                self.assertEqual(cache.cursor(), 0)
                self.assertEqual(cache.context()["items"], [])
                with cache.connect() as connection:
                    connection.execute("DROP TRIGGER deny_checkpoint")
                self.assertFalse(cache.recover_page("fixture-membership"))
                self.assertEqual(cache.cursor(), 1)
                self.assertEqual([n["id"] for n in cache.context()["items"]], [1])

    def test_invalid_or_legacy_pages_never_skip_a_recovery_gap(self):
        with tempfile.TemporaryDirectory() as tmp:
            cache = notifications.NotificationCache(Path(tmp) / "cache.db", "http://local", "reviewer", "fixture-membership")
            for page in [
                {"notifications": [notice(2), notice(1)], "more": False},
                {"order": "asc", "notifications": [notice(2), notice(1)], "more": False},
                {"order": "asc", "notifications": [notice(1), notice(1)], "more": False},
                {"order": "asc", "notifications": [], "more": True},
            ]:
                with self.subTest(page=page), mock.patch.object(notifications, "fetch_page", return_value=page):
                    with self.assertRaises(ValueError):
                        cache.recover_page("fixture-membership")
                    self.assertEqual(cache.cursor(), 0)
                    self.assertEqual(cache.context()["items"], [])

    def test_cache_is_bounded_and_scoped_to_origin_and_identity(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "cache.db"
            cache = notifications.NotificationCache(path, "http://local", "reviewer", "fixture-membership")
            def page(_server, _membership, **query):
                ids = range(query["since"] + 1, min(200, query["since"] + notifications.PAGE_SIZE) + 1)
                rows = [notice(message_id) for message_id in ids]
                return {"order": "asc", "notifications": rows, "more": rows[-1]["id"] < 200}
            with mock.patch.object(notifications, "fetch_page", side_effect=page):
                while cache.recover_page("fixture-membership"):
                    pass
            self.assertEqual(cache.cursor(), 200)
            with cache.connect() as connection:
                self.assertEqual(connection.execute("SELECT COUNT(*) FROM notices").fetchone()[0], notifications.CACHE_LIMIT)
            self.assertEqual([n["id"] for n in cache.context()["items"]], [197, 198, 199, 200])
            self.assertTrue(cache.context()["more_available"])
            self.assertEqual(notifications.NotificationCache(path, "http://other", "reviewer", "fixture-membership").context()["items"], [])
            self.assertEqual(notifications.NotificationCache(path, "http://local", "another", "fixture-membership").context()["items"], [])
            replacement = notifications.NotificationCache(path, "http://local", "reviewer", "new-membership")
            self.assertEqual(replacement.context()["items"], [])
            self.assertEqual(replacement.cursor(), 0)
            with self.assertRaises(ValueError):
                cache.recover_page("new-membership")
            if os.name != "nt":
                self.assertEqual(path.stat().st_mode & 0o777, 0o600)

    def test_polling_failure_backs_off_and_stop_is_interruptible(self):
        attempted = threading.Event()
        def fail(_membership):
            attempted.set()
            raise OSError("fixture transport failure")
        cache = mock.Mock()
        cache.recover_page.side_effect = fail
        recovery = notifications.NotificationRecovery(cache, "fixture-membership")
        recovery.start()
        try:
            self.assertTrue(attempted.wait(2))
        finally:
            recovery.stop()
        self.assertFalse(recovery.thread.is_alive())
        self.assertEqual(cache.recover_page.call_count, 1)

    def test_room_attention_reaches_codex_with_announcements_but_no_extra_turn(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = adapter_tests.AdapterFixture(Path(tmp))
            record = adapter_tests.delivery(400)
            record["event"]["notification_context"] = {
                "recipient": "reviewer", "items": [notice(9)], "more_available": False,
            }
            fixture.ingest(record)
            self.assertTrue(fixture.adapter.dispatch_one())
            turns = [params for method, params in fixture.fake.requests if method == "turn/start"]
            self.assertEqual(len(turns), 1)
            prompt = turns[0]["input"][0]["text"]
            self.assertIn("notice-9", prompt)
            self.assertIn("context, not room speech, tasks or permission grants", prompt)
            self.assertIn("A user-visible reply is required", prompt)
            self.assertFalse(fixture.adapter.dispatch_one())
            self.assertEqual(fixture.relay.calls, [])


if __name__ == "__main__":
    unittest.main()
