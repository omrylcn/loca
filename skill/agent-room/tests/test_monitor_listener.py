import subprocess
import sys
import tempfile
import time
import unittest
from types import SimpleNamespace
from pathlib import Path


SKILL_DIR = Path(__file__).resolve().parents[1]
SUPERVISOR = SKILL_DIR / "monitor_listener.py"
sys.path.insert(0, str(SKILL_DIR))
import monitor_listener  # noqa: E402
import listen  # noqa: E402


class MonitorListenerTests(unittest.TestCase):
    def test_windows_signal_set_does_not_require_sighup_or_sigquit(self):
        windows_signal = SimpleNamespace(SIGTERM=15, SIGINT=2)
        self.assertEqual(set(monitor_listener.handled_signals(windows_signal)), {2, 15})
    def command(self, root: Path, child: Path, *child_args: str) -> list[str]:
        return [
            sys.executable,
            str(SUPERVISOR),
            "--name",
            "worker",
            "--log",
            str(root / "monitor.log"),
            "--lock",
            str(root / "monitor.lock"),
            "--max-restarts",
            "2",
            "--stable-seconds",
            "60",
            "--base-delay",
            "0.01",
            "--max-delay",
            "0.01",
            "--",
            sys.executable,
            str(child),
            *child_args,
        ]

    def test_signal_killed_listener_is_restarted_and_logged(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            child = root / "child.py"
            counter = root / "counter"
            child.write_text(
                """import os, pathlib, signal, sys
path = pathlib.Path(sys.argv[1])
count = int(path.read_text() if path.exists() else "0") + 1
path.write_text(str(count))
if count == 1:
    os.kill(os.getpid(), signal.SIGKILL)
""",
                encoding="utf-8",
            )

            result = subprocess.run(
                self.command(root, child, str(counter)),
                text=True,
                capture_output=True,
                timeout=5,
                check=False,
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(counter.read_text(encoding="utf-8"), "2")
            log = (root / "monitor.log").read_text(encoding="utf-8")
            self.assertIn("signal=SIGKILL", log)
            self.assertIn("listener restart", log)
            self.assertIn("clean listener exit", log)

    def test_listener_file_sink_is_rejected_because_it_cannot_wake_monitor(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fake_listener = root / "listen.py"
            fake_listener.write_text("raise SystemExit(0)\n", encoding="utf-8")
            result = subprocess.run(
                self.command(
                    root,
                    fake_listener,
                    "wss://loca.example/ws?room=sb-dev&name=worker",
                    str(root / ".agent-room" / "sb-dev.jsonl"),
                ),
                text=True,
                capture_output=True,
                timeout=5,
                check=False,
            )

            self.assertEqual(result.returncode, 64)
            self.assertIn("monitor_error", result.stdout)
            self.assertIn("delivers without waking", result.stdout)
            self.assertIn(
                "output must be /dev/stdout",
                (root / "monitor.log").read_text(encoding="utf-8"),
            )

    def test_native_monitor_marks_only_its_listener_for_runtime_health(self):
        command = [sys.executable, "/opt/loca/listen.py", "wss://example/ws", "-"]
        marked = monitor_listener.enable_native_runtime_health(command)
        self.assertEqual(marked[-1], "--runtime-health")
        self.assertNotIn("--runtime-health", command)
        self.assertEqual(
            monitor_listener.enable_native_runtime_health(marked).count(
                "--runtime-health"
            ),
            1,
        )
        manual = [sys.executable, "/opt/loca/other-listener.py"]
        self.assertEqual(monitor_listener.enable_native_runtime_health(manual), manual)

    def test_native_health_renews_only_for_a_live_active_connection(self):
        class RecordingLease(listen.NativeRuntimeHealthLease):
            def __init__(self):
                super().__init__("https://example", "membership", 0.02, 0.07)
                self.reports = 0
                self.reported = __import__("threading").Event()

            def _report(self):
                self.reports += 1
                self.reported.set()

        lease = RecordingLease()
        lease.start()
        try:
            time.sleep(0.04)
            self.assertEqual(lease.reports, 0, "a child PID alone is not ready")

            lease.connected()
            self.assertTrue(lease.reported.wait(timeout=1))
            first = lease.reports
            lease.activity()
            time.sleep(0.06)
            self.assertGreater(lease.reports, first, "a live connection renews its lease")

            # No frames/pings: stop renewing before the server's 20 second TTL.
            time.sleep(0.10)
            expired = lease.reports
            time.sleep(0.06)
            self.assertEqual(lease.reports, expired)

            lease.disconnected()
            lease.activity()
            disconnected = lease.reports
            time.sleep(0.06)
            self.assertEqual(
                lease.reports,
                disconnected,
                "activity cannot revive a disconnected listener",
            )
        finally:
            lease.stop()

    def test_supervisor_stop_terminates_child_without_restart(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            child = root / "child.py"
            started = root / "started"
            child.write_text(
                """import pathlib, sys, time
pathlib.Path(sys.argv[1]).write_text("started")
while True:
    time.sleep(1)
""",
                encoding="utf-8",
            )
            process = subprocess.Popen(
                self.command(root, child, str(started)),
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )
            deadline = time.monotonic() + 3
            while time.monotonic() < deadline and not started.exists():
                time.sleep(0.02)
            self.assertTrue(started.exists())

            process.terminate()
            self.assertEqual(process.wait(timeout=5), 0)
            process.communicate(timeout=1)
            log = (root / "monitor.log").read_text(encoding="utf-8")
            self.assertEqual(log.count("listener started"), 1)
            self.assertIn("supervisor stopped signal=SIGTERM", log)

    def test_encoded_signal_exit_is_recorded_before_bounded_failure(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            child = root / "child.py"
            child.write_text("raise SystemExit(144)\n", encoding="utf-8")
            command = self.command(root, child)
            restart_index = command.index("--max-restarts") + 1
            command[restart_index] = "0"

            result = subprocess.run(
                command,
                text=True,
                capture_output=True,
                timeout=5,
                check=False,
            )

            self.assertEqual(result.returncode, 1)
            log = (root / "monitor.log").read_text(encoding="utf-8")
            self.assertIn("exit_code=144", log)
            self.assertIn("signal=SIGSTKFLT (encoded)", log)
            self.assertIn('"t": "monitor_error"', result.stdout)

    def test_slow_crashes_still_exhaust_the_rolling_restart_budget(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            child = root / "child.py"
            child.write_text(
                "import time\ntime.sleep(0.03)\nraise SystemExit(1)\n",
                encoding="utf-8",
            )
            command = self.command(root, child)
            stable_index = command.index("--stable-seconds") + 1
            command[stable_index] = "0.01"
            failure_window_index = command.index("--base-delay")
            command[failure_window_index:failure_window_index] = [
                "--failure-window",
                "5",
            ]

            result = subprocess.run(
                command,
                text=True,
                capture_output=True,
                timeout=5,
                check=False,
            )

            self.assertEqual(result.returncode, 1)
            log = (root / "monitor.log").read_text(encoding="utf-8")
            self.assertEqual(log.count("listener exited"), 3)
            self.assertIn("restart budget exhausted", log)

    def test_duplicate_supervisor_is_refused_by_lock(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            child = root / "child.py"
            started = root / "started"
            child.write_text(
                """import pathlib, sys, time
pathlib.Path(sys.argv[1]).write_text("started")
while True:
    time.sleep(1)
""",
                encoding="utf-8",
            )
            first = subprocess.Popen(
                self.command(root, child, str(started)),
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )
            try:
                deadline = time.monotonic() + 3
                while time.monotonic() < deadline and not started.exists():
                    time.sleep(0.02)
                self.assertTrue(started.exists())

                duplicate = subprocess.run(
                    self.command(root, child, str(started)),
                    text=True,
                    capture_output=True,
                    timeout=3,
                    check=False,
                )

                self.assertEqual(duplicate.returncode, 75)
                self.assertIn("duplicate monitor supervisor", duplicate.stdout)
            finally:
                first.terminate()
                first.wait(timeout=5)
                first.communicate(timeout=1)


if __name__ == "__main__":
    unittest.main()
