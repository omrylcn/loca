import sys
import unittest
from pathlib import Path


SKILL_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SKILL_DIR))

from listen import renew_backoff_plan  # noqa: E402


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


if __name__ == "__main__":
    unittest.main()
