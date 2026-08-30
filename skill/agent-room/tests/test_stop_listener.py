"""Committed tests for the identity-specific safe stop.

Two layers:
  * pure matchers — an exact ws-URL name (never a prefix), listener detection,
    and self/ancestor exclusion;
  * REAL processes — stopping one identity actually terminates only that
    identity's listener while the others keep running; an exited PID is skipped;
    and a PID whose start-time changed after selection is NEVER signalled (the
    reuse guard). No broad pattern kill anywhere.

Every spawned process is reaped (terminate -> wait, pipes closed) so the suite
itself leaves no ghost PID or open fd behind.
"""

import importlib.util
import os
import subprocess
import sys
import time
import unittest
from pathlib import Path
from unittest import mock

SKILL_DIR = Path(__file__).resolve().parents[1]


def _load():
    spec = importlib.util.spec_from_file_location("stop_listener", SKILL_DIR / "stop_listener.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def reap(proc):
    """Terminate (if alive), wait (reap the PID), and close every pipe."""
    try:
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=5)
        else:
            proc.wait(timeout=5)  # idempotent reap of an already-exited child
    except Exception:  # noqa: BLE001
        pass
    finally:
        for pipe in (proc.stdout, proc.stderr, proc.stdin):
            if pipe is not None:
                try:
                    pipe.close()
                except Exception:  # noqa: BLE001
                    pass


class MatchTest(unittest.TestCase):
    def setUp(self):
        self.mod = _load()

    def test_ws_name_and_host_exact(self):
        args = [b"python3", b"listen.py",
                b"wss://loca.example/ws?room=&name=mihenk&type=agent&filter=mentions"]
        self.assertEqual(self.mod.ws_name_and_host(args), ("mihenk", "loca.example"))
        self.assertTrue(self.mod.is_listener(args))

    def test_name_is_exact_not_a_prefix(self):
        args = [b"python3", b"listen.py",
                b"ws://127.0.0.1:8787/ws?room=&name=mihenk-2&type=agent"]
        name, _ = self.mod.ws_name_and_host(args)
        self.assertEqual(name, "mihenk-2")
        self.assertNotEqual(name, "mihenk")

    def test_non_listener_is_ignored(self):
        self.assertFalse(self.mod.is_listener([b"python3", b"-c", b"print(1)"]))

    def test_ancestors_include_self(self):
        self.assertIn(os.getpid(), self.mod.ancestors())


class RealProcessTest(unittest.TestCase):
    def _fake_listener(self, name):
        # A real process whose /proc cmdline looks like a listener for `name`:
        # it mentions listen.py and carries a ws URL naming it. It sleeps so it
        # stays alive until stopped, with no pipes to leak.
        proc = subprocess.Popen(
            [sys.executable, "-c", "import time; time.sleep(30)", "listen.py",
             "ws://127.0.0.1:9/ws?room=&name=%s&type=agent&filter=mentions" % name],
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        self.addCleanup(reap, proc)
        return proc

    def _stop(self, name):
        return subprocess.run(
            [sys.executable, str(SKILL_DIR / "stop_listener.py"), "127.0.0.1", name],
            capture_output=True, text=True, timeout=15,
        )

    def test_stop_signals_only_the_named_identity(self):
        alpha = self._fake_listener("alpha")
        beta = self._fake_listener("beta")
        alpha2 = self._fake_listener("alpha-2")
        time.sleep(0.5)
        result = self._stop("alpha")
        time.sleep(0.6)
        self.assertIsNotNone(alpha.poll(), "alpha must be stopped\n%s" % result.stdout)
        self.assertIsNone(beta.poll(), "beta must keep running (isolation)")
        self.assertIsNone(alpha2.poll(), "alpha-2 must keep running (exact-name boundary)")
        self.assertIn("stopped pid=%d" % alpha.pid, result.stdout)

    def test_stop_with_nothing_to_signal_is_a_clean_noop(self):
        result = self._stop("no-such-identity-here")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("nothing to stop", result.stdout)

    def test_exited_pid_is_skipped_not_signalled(self):
        victim = self._fake_listener("ghost")
        time.sleep(0.4)
        mod = _load()
        cands = mod.candidates("127.0.0.1", "ghost")
        self.assertTrue(any(pid == victim.pid for pid, _ in cands))
        victim.terminate()
        victim.wait(timeout=5)
        result = self._stop("ghost")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("nothing to stop", result.stdout)

    def test_start_time_change_after_selection_prevents_any_signal(self):
        # The accepted PID-reuse boundary: if the recorded start-time from
        # candidate selection no longer matches the start-time seen just before
        # signalling, os.kill must NOT be called and the process must survive.
        mod = _load()
        victim = self._fake_listener("phantom")
        time.sleep(0.4)
        real = mod.starttime(victim.pid)
        self.assertIsNotNone(real)
        # Simulate reuse: candidates() reports a DIFFERENT start-time than the
        # live one, so main()'s pre-signal re-read will mismatch.
        stale = str(int(real) + 999)
        with mock.patch.object(mod, "candidates", return_value=[(victim.pid, stale)]), \
                mock.patch.object(mod.os, "kill") as killed:
            rc = mod.main(["stop_listener.py", "127.0.0.1", "phantom"])
        killed.assert_not_called()
        self.assertEqual(rc, 0)
        self.assertIsNone(victim.poll(), "an unmatched start-time must never be signalled")


if __name__ == "__main__":
    unittest.main()
