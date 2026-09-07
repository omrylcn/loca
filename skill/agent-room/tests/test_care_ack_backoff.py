"""Fences for the care-ACK auth refusal path.

A care ACK refused on auth grounds cannot succeed by retrying sooner. Before
these fences the listener treated it as a transient error and rode the generic
2s reconnect tail: one agent produced ~12 failed ACKs a minute for 14 hours
(6601 and climbing) while still reporting ONLINE, and nothing surfaced it.
"""
import http.server
import sys
import threading
import unittest
from pathlib import Path

SKILL_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SKILL_DIR))

from listen import (  # noqa: E402
    ACK_AUTH_BASE_BACKOFF,
    ACK_AUTH_MAX_BACKOFF,
    CARE_ACK_REFUSALS,
    acknowledge_care,
    care_ack_degrade,
    care_ack_recover,
)


def fresh_state():
    return {"count": 0, "first": None, "last": None, "reason": None}


class CareAckClassificationTests(unittest.TestCase):
    """The ACK result must distinguish 'never going to work' from 'transient'."""

    @classmethod
    def setUpClass(cls):
        cls.code = {"n": 401}

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                self.send_response(cls.code["n"])
                self.end_headers()

            def log_message(self, *args):
                pass

        cls.server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
        cls.port = cls.server.server_address[1]
        threading.Thread(target=cls.server.serve_forever, daemon=True).start()

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()

    def ack(self, status):
        type(self).code["n"] = status
        url = f"ws://127.0.0.1:{self.port}/ws?room=probe&name=alice"
        return acknowledge_care(url, "sig-1", "alice")

    def test_401_and_403_are_named_apart(self):
        # Both refuse, but a credential fault and a policy refusal are not the
        # same event and must not be reported under one name.
        self.assertEqual(self.ack(401), "unauthorized")
        self.assertEqual(self.ack(403), "forbidden")
        self.assertEqual(CARE_ACK_REFUSALS["unauthorized"], "care_ack_unauthorized")
        self.assertEqual(CARE_ACK_REFUSALS["forbidden"], "care_ack_forbidden")

    def test_transient_and_success_are_not_treated_as_refusals(self):
        # A 500 may well succeed on the next try, so it must NOT be pushed into
        # the backoff class; 204 is the success contract.
        self.assertEqual(self.ack(500), "error")
        self.assertNotIn("error", CARE_ACK_REFUSALS)
        self.assertEqual(self.ack(204), "ok")


class CareAckBackoffTests(unittest.TestCase):
    def test_refusals_share_the_backoff_but_keep_their_own_reason(self):
        for status, expected in CARE_ACK_REFUSALS.items():
            state = fresh_state()
            reason, delay = care_ack_degrade(status, state, now="T0")
            self.assertEqual(reason, expected)
            self.assertEqual(state["reason"], expected)
            self.assertEqual(delay, ACK_AUTH_BASE_BACKOFF)

    def test_delay_grows_and_stops_at_the_cap(self):
        state = fresh_state()
        delays = [care_ack_degrade("unauthorized", state, now="T")[1] for _ in range(12)]
        self.assertEqual(delays[:4], [2.0, 4.0, 8.0, 16.0])
        self.assertEqual(delays, sorted(delays), "backoff must never shrink")
        self.assertEqual(delays[-1], ACK_AUTH_MAX_BACKOFF)
        self.assertLessEqual(max(delays), ACK_AUTH_MAX_BACKOFF)

    def test_auth_refusal_outgrows_the_generic_reconnect_tail(self):
        # The bug was that this path fell back to the flat ~2s reconnect delay.
        # After a couple of refusals the wait must be far longer than that.
        state = fresh_state()
        for _ in range(5):
            _, delay = care_ack_degrade("unauthorized", state, now="T")
        self.assertGreater(delay, 10.0)

    def test_first_and_last_sighting_are_recorded(self):
        state = fresh_state()
        care_ack_degrade("unauthorized", state, now="FIRST")
        care_ack_degrade("unauthorized", state, now="LATER")
        self.assertEqual(state["count"], 2)
        self.assertEqual(state["first"], "FIRST", "first sighting must not move")
        self.assertEqual(state["last"], "LATER")

    def test_success_clears_the_degrade(self):
        # A recovered agent must not stay parked at the cap forever.
        state = fresh_state()
        for _ in range(9):
            care_ack_degrade("unauthorized", state, now="T")
        self.assertEqual(care_ack_recover(state), 9)
        self.assertEqual(state["count"], 0)
        self.assertIsNone(state["reason"])
        _, delay = care_ack_degrade("unauthorized", state, now="T")
        self.assertEqual(delay, ACK_AUTH_BASE_BACKOFF, "must restart from base")


if __name__ == "__main__":
    unittest.main()
