import sys
import unittest
from pathlib import Path


SKILL_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SKILL_DIR))

from listen import (  # noqa: E402
    HandshakeRefused,
    is_door_refusal,
    is_loca_full,
    refused_retry_delay,
    renew_backoff_plan,
    suspended_retry_delay,
)


class RenewBackoffTests(unittest.TestCase):
    # FIX 5: a handshake 401 that renews-but-is-still-rejected used to `continue`
    # with no delay, hammering /sessions. The renew path must always back off.
    def test_renew_always_sleeps_so_the_loop_cannot_spin(self):
        attempts = 0
        for _ in range(20):
            sleep_seconds, attempts = renew_backoff_plan(attempts)
            self.assertGreater(sleep_seconds, 0.0)

    def test_backoff_grows_then_resets_at_the_cap(self):
        schedule = []
        attempts = 0
        for _ in range(5):
            sleep_seconds, attempts = renew_backoff_plan(attempts)
            schedule.append(sleep_seconds)
        self.assertEqual(schedule, [1.0, 2.0, 4.0, 8.0, 16.0])
        # Reaching the attempt cap resets the counter so it stops growing.
        self.assertEqual(attempts, 0)

    def test_backoff_never_exceeds_the_cap(self):
        sleep_seconds, _ = renew_backoff_plan(
            9, base=1.0, cap=30.0, max_attempts=100
        )
        self.assertEqual(sleep_seconds, 30.0)

    def test_consecutive_renews_settle_into_a_bounded_backoff(self):
        attempts = 0
        peak = 0.0
        for _ in range(50):
            sleep_seconds, attempts = renew_backoff_plan(attempts)
            peak = max(peak, sleep_seconds)
        self.assertLessEqual(peak, 30.0)


class SuspendedRetryTests(unittest.TestCase):
    # A suspended membership is refused at the Lobby until the Master resumes
    # it. The wait must be slow from the first retry and bounded, so a long
    # suspension costs one handshake a minute, not a reconnect storm.
    def test_suspended_retry_is_slow_and_bounded(self):
        schedule = [suspended_retry_delay(attempt) for attempt in range(40)]
        self.assertEqual(schedule[:5], [5.0, 10.0, 20.0, 40.0, 60.0])
        self.assertEqual(max(schedule), 60.0)
        self.assertEqual(schedule[-1], 60.0)


class RefusedDoorTests(unittest.TestCase):
    # A loca's door that says no (banned, no such loca, loca full) was asked
    # again every two seconds for ever. A decision does not change by being
    # asked again quickly; the wait doubles to a bounded minute.
    def test_a_refused_door_is_asked_again_less_and_less_often(self):
        schedule = [refused_retry_delay(streak) for streak in range(1, 12)]
        self.assertEqual(schedule[:6], [2.0, 4.0, 8.0, 16.0, 32.0, 60.0])
        self.assertEqual(max(schedule), 60.0)
        self.assertEqual(schedule[-1], 60.0)
        # Nothing refused yet: the ordinary short retry.
        self.assertEqual(refused_retry_delay(0), 2.0)

    def test_a_door_refused_for_days_still_gets_an_answer(self):
        # A day of refusals at one a minute is 1,440 of them. The delay must
        # stay a number: an OverflowError here would end the loca's thread
        # and the agent would never take the seat when the door opens.
        for streak in (17, 64, 1024, 1025, 1440, 100_000, 10**9):
            self.assertEqual(refused_retry_delay(streak), 60.0, streak)

    def test_only_a_decision_counts_as_a_refusal(self):
        for status in (403, 404, 409, 429, 500):
            self.assertTrue(is_door_refusal(HandshakeRefused("refused", status)), status)
        # A 401 is a dead session, renewed by its own path.
        self.assertFalse(is_door_refusal(HandshakeRefused("refused", 401)))
        # A server that cannot be reached is found again quickly.
        self.assertFalse(is_door_refusal(ConnectionError("closed during handshake")))
        self.assertFalse(is_door_refusal(OSError("connection refused")))

    def test_the_full_loca_frame_is_recognised_and_other_controls_are_not(self):
        self.assertTrue(
            is_loca_full(
                {"t": "control", "cmd": "loca full (7 seats). ask the master for a seat, or watch with ?watch=1"}
            )
        )
        for cmd in ("stop", "room-closed", "", None):
            self.assertFalse(is_loca_full({"t": "control", "cmd": cmd}), cmd)
        self.assertFalse(is_loca_full(None))


if __name__ == "__main__":
    unittest.main()
