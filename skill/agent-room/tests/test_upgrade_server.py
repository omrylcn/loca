"""scripts/upgrade-server.py end to end on loopback, in temporary directories.

A stand-in service manager (`svc.py start|stop`) runs whatever file sits at
the installed binary path, the way systemd runs its ExecStart. The default
tests use small Python stand-ins for room-server so they run without a Rust
build; set LOCA_UPGRADE_E2E_OLD, LOCA_UPGRADE_E2E_NEW and
LOCA_UPGRADE_E2E_FAILING to real room-server builds to run the same flow on
them (RealBinaryUpgradeTests).
"""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import socket
import signal
import sqlite3
import subprocess
import sys
import tarfile
import tempfile
import time
import unittest
import urllib.request

REPO = Path(__file__).resolve().parents[3]
SCRIPT = REPO / "scripts" / "upgrade-server.py"
SNAPSHOT_TOOL = REPO / "scripts" / "storage_snapshot.py"

spec = importlib.util.spec_from_file_location("upgrade_server", SCRIPT)
upgrade = importlib.util.module_from_spec(spec)
spec.loader.exec_module(upgrade)

FAKE_SERVER = """#!{python}
import http.server, json, os, signal, sqlite3, sys, time
VERSION = {version!r}
MODE = {mode!r}
db = os.environ.get("DB_PATH")
if db:
    connection = sqlite3.connect(db)
    connection.execute("PRAGMA journal_mode=WAL")  # as the real store
    connection.execute("CREATE TABLE IF NOT EXISTS opened_by (version TEXT)")
    connection.execute("INSERT INTO opened_by VALUES (?)", (VERSION,))
    connection.commit()
    connection.close()
    if MODE == "fail-after-open":
        sys.exit("fault: opened the store, refusing to serve")
class Health(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        # With a store, the "alive-*" modes model a process that is running
        # with the database open and does not answer /health as health.
        if db and MODE == "alive-hang":
            time.sleep(3600)
        if db and MODE == "alive-500":
            self.send_response(500)
            self.send_header("content-length", "0")
            self.end_headers()
            return
        if db and MODE.startswith("alive-redirect:"):
            # Points at a port where nothing listens.
            self.send_response(302)
            self.send_header("location", "http://127.0.0.1:%s/health" % MODE.split(":")[1])
            self.send_header("content-length", "0")
            self.end_headers()
            return
        body = json.dumps({{"ok": True, "version": VERSION}}).encode()
        if db and MODE == "alive-garbage":
            body = b"<html>not the health answer</html>"
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *args):
        pass
server = http.server.HTTPServer((os.environ.get("BIND_ADDR", "127.0.0.1"), int(os.environ.get("PORT", "8787"))), Health)
if os.environ.get("BOUND_PORT_FILE"):
    with open(os.environ["BOUND_PORT_FILE"], "w") as port_file:
        port_file.write(str(server.server_address[1]))
signal.signal(signal.SIGTERM, lambda *args: sys.exit(0))
server.serve_forever()
"""

SERVICE = """#!{python}
import json, os, signal, subprocess, sys, time
here = os.path.dirname(os.path.abspath(__file__))
config = json.load(open(os.path.join(here, "svc.json")))
with open(os.path.join(here, "calls.log"), "a") as calls:
    calls.write(sys.argv[1] + "\\n")
pidfile = os.path.join(here, "pid")

def alive(pid):
    try:
        with open("/proc/%d/stat" % pid) as stat:
            return stat.read().rsplit(")", 1)[1].split()[0] != "Z"
    except FileNotFoundError:
        return False

if sys.argv[1] == "start":
    env = {{"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "DB_PATH": config["db"],
           "PORT": str(config["port"]), "BIND_ADDR": "127.0.0.1", "RUST_LOG": "warn"}}
    log = open(os.path.join(here, "server.log"), "ab")
    process = subprocess.Popen([config["binary"]], env=env, stdout=log, stderr=subprocess.STDOUT,
                               start_new_session=True)
    with open(pidfile, "w") as handle:
        handle.write(str(process.pid))
else:
    # A service manager whose stop does nothing from its second call on:
    # it either says so ("fail") or reports success anyway ("lie").
    stops = open(os.path.join(here, "calls.log")).read().split().count("stop")
    if config.get("later_stops") and stops >= 2:
        sys.exit(1 if config["later_stops"] == "fail" else 0)
    try:
        pid = int(open(pidfile).read())
    except FileNotFoundError:
        sys.exit(0)
    try:
        os.kill(pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    deadline = time.time() + 10
    while time.time() < deadline and alive(pid):
        time.sleep(0.05)
    if alive(pid):
        os.kill(pid, signal.SIGKILL)
    os.unlink(pidfile)
"""


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def health(port):
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=2) as response:
            return json.loads(response.read())
    except OSError:
        return None


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


class Deployment:
    """A temporary binary deployment run by svc.py on a loopback port."""

    def __init__(self, root, installed_binary, with_attachment=True):
        self.root = root
        self.port = free_port()
        self.service = root / "service"
        self.service.mkdir()
        self.bin_dir = root / "bin"
        self.bin_dir.mkdir()
        self.binary = self.bin_dir / "room-server"
        self.binary.write_bytes(Path(installed_binary).read_bytes())
        self.binary.chmod(0o755)
        self.data = root / "data"
        self.data.mkdir()
        self.db = self.data / "loca.db"
        self.blob_content = b"an attachment that must survive an upgrade and a rollback"
        self.blob_sha = hashlib.sha256(self.blob_content).hexdigest()
        with sqlite3.connect(self.db) as conn:
            conn.executescript(
                "CREATE TABLE IF NOT EXISTS attachment_blobs (sha TEXT PRIMARY KEY, size INTEGER);"
                "CREATE TABLE IF NOT EXISTS upgrade_test (text TEXT);"
            )
            conn.execute("INSERT INTO upgrade_test VALUES ('written before the upgrade')")
            if with_attachment:
                conn.execute("INSERT INTO attachment_blobs VALUES (?, ?)", (self.blob_sha, len(self.blob_content)))
        if with_attachment:
            self.blob = self.data / "attachments" / self.blob_sha[:2] / self.blob_sha
            self.blob.parent.mkdir(parents=True)
            self.blob.write_bytes(self.blob_content)
        (self.service / "svc.json").write_text(json.dumps(
            {"binary": str(self.binary), "db": str(self.db), "port": self.port}))
        self.svc = self.service / "svc.py"
        self.svc.write_text(SERVICE.format(python=sys.executable))
        self.svc.chmod(0o755)

    def start(self):
        subprocess.run([str(self.svc), "start"], check=True)
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if health(self.port):
                (self.service / "calls.log").unlink()
                return
            time.sleep(0.05)
        raise AssertionError("the stand-in service did not become healthy:\n"
                             + (self.service / "server.log").read_text(errors="replace"))

    def stop(self):
        subprocess.run([str(self.svc), "stop"], check=False)
        # A stand-in whose stop does nothing leaves its server running.
        pid = self.pid()
        if pid:
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass

    def pid(self):
        try:
            return int((self.service / "pid").read_text())
        except (FileNotFoundError, ValueError):
            return None

    def server_alive(self):
        pid = self.pid()
        if not pid:
            return False
        try:
            stat = Path(f"/proc/{pid}/stat").read_text()
        except FileNotFoundError:
            return False
        return stat.rsplit(")", 1)[1].split()[0] != "Z"

    def later_stops(self, behaviour):
        """From its second call on, `svc.py stop` does nothing: "fail" or "lie"."""
        config = json.loads((self.service / "svc.json").read_text())
        config["later_stops"] = behaviour
        (self.service / "svc.json").write_text(json.dumps(config))

    def calls(self):
        path = self.service / "calls.log"
        return path.read_text().split() if path.exists() else []

    def rows(self, table):
        with sqlite3.connect(self.db) as conn:
            exists = conn.execute("SELECT 1 FROM sqlite_master WHERE name = ?", (table,)).fetchone()
            if not exists:
                return []
            return [row[0] for row in conn.execute(f"SELECT * FROM {table}")]


class UpgradeHarness(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.downloads = self.root / "downloads"
        self.downloads.mkdir()
        self.backups = self.root / "backups"
        self.backups.mkdir()

    def fake_binary(self, version, mode="ok"):
        path = self.root / f"fake-room-server-{version}-{mode}"
        path.write_text(FAKE_SERVER.format(python=sys.executable, version=version, mode=mode))
        path.chmod(0o755)
        return path

    def release(self, version, binary, *, corrupt_sum=False):
        """A release archive and SHA256SUMS in the layout release.yml publishes."""
        stage = self.root / f"stage-{version}-{len(list(self.root.iterdir()))}"
        folder = stage / f"loca-{version}-linux-amd64"
        folder.mkdir(parents=True)
        (folder / "room-server").write_bytes(Path(binary).read_bytes())
        (folder / "room-server").chmod(0o755)
        archive = self.downloads / f"loca-{version}-linux-amd64.tar.gz"
        with tarfile.open(archive, "w:gz") as tar:
            tar.add(folder, arcname=folder.name)
        digest = sha256(archive)
        if corrupt_sum:
            digest = ("0" if digest[0] != "0" else "1") + digest[1:]
        (self.downloads / "SHA256SUMS").write_text(f"{digest}  ./{archive.name}\n")
        return archive

    def deploy(self, installed_binary, **kwargs):
        deployment = Deployment(self.root, installed_binary, **kwargs)
        self.addCleanup(deployment.stop)
        deployment.start()
        return deployment

    def run_upgrade(self, deployment, archive, version, *extra, work_dir=None, snapshot_tool=SNAPSHOT_TOOL,
                    health_timeout="20"):
        self.work_dir = work_dir or self.backups / f"upgrade-{version}"
        argv = [
            sys.executable, str(SCRIPT),
            "--archive", str(archive), "--sums", str(self.downloads / "SHA256SUMS"),
            "--version", version, "--binary", str(deployment.binary), "--db-path", str(deployment.db),
            "--work-dir", str(self.work_dir), "--health-url", f"http://127.0.0.1:{deployment.port}/health",
            "--stop-command", f"{deployment.svc} stop", "--start-command", f"{deployment.svc} start",
            "--snapshot-tool", str(snapshot_tool), "--health-timeout", health_timeout, *extra,
        ]
        return subprocess.run(argv, capture_output=True, text=True, timeout=180, check=False)


class UpgradeServerTests(UpgradeHarness):
    def test_upgrade_installs_new_version_after_a_verified_snapshot(self):
        deployment = self.deploy(self.fake_binary("0.13.0"))
        archive = self.release("0.13.1", self.fake_binary("0.13.1"))
        old_binary = sha256(deployment.binary)

        result = self.run_upgrade(deployment, archive, "0.13.1")

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("UPGRADED 0.13.0 -> 0.13.1", result.stdout)
        # The one-way warning comes before the service is stopped.
        self.assertLess(result.stdout.index("ONE WAY: once 0.13.1 has started"), result.stdout.index("[3/7]"))
        self.assertEqual(health(deployment.port)["version"], "0.13.1")
        self.assertEqual(deployment.calls(), ["stop", "start"])
        self.assertEqual(sha256(self.work_dir / "previous" / "room-server"), old_binary)
        verify = subprocess.run([sys.executable, str(SNAPSHOT_TOOL), "verify", str(self.work_dir / "snapshot")],
                                capture_output=True, text=True, check=False)
        self.assertEqual(verify.returncode, 0, verify.stdout + verify.stderr)
        with sqlite3.connect(self.work_dir / "snapshot" / "loca.sqlite3") as conn:
            self.assertEqual([r[0] for r in conn.execute("SELECT version FROM opened_by")], ["0.13.0"])
        self.assertEqual(deployment.rows("opened_by"), ["0.13.0", "0.13.1"])
        self.assertEqual(deployment.blob.read_bytes(), deployment.blob_content)

    def test_new_version_failing_before_healthy_rolls_back_onto_the_snapshot(self):
        deployment = self.deploy(self.fake_binary("0.13.0"))
        # Answers the memory-only probe, then writes to the store and dies:
        # a migration that ran and a server that never became healthy.
        archive = self.release("0.13.1", self.fake_binary("0.13.1", mode="fail-after-open"))
        old_binary = sha256(deployment.binary)

        result = self.run_upgrade(deployment, archive, "0.13.1", health_timeout="3")

        self.assertEqual(result.returncode, 4, result.stdout + result.stderr)
        self.assertIn("ROLLING BACK to 0.13.0", result.stdout)
        self.assertIn("everything written after the upgrade started is discarded", result.stdout)
        self.assertEqual(health(deployment.port)["version"], "0.13.0")
        self.assertEqual(sha256(deployment.binary), old_binary)
        # The live store is the snapshot again: the failed version's write is
        # gone from it, and kept aside rather than deleted.
        self.assertEqual(deployment.rows("opened_by"), ["0.13.0", "0.13.0"])
        self.assertEqual(deployment.rows("upgrade_test"), ["written before the upgrade"])
        self.assertEqual(deployment.blob.read_bytes(), deployment.blob_content)
        aside = list(deployment.data.glob(".loca-failed-upgrade-*"))
        self.assertEqual(len(aside), 1)
        with sqlite3.connect(aside[0] / "loca.db") as conn:
            self.assertIn(("0.13.1",), conn.execute("SELECT version FROM opened_by").fetchall())
        self.assertEqual(list(deployment.data.glob(".loca-rollback-*")), [])
        self.assertEqual(deployment.calls(), ["stop", "start", "stop", "start"])

    def test_a_server_not_known_to_have_stopped_keeps_its_data_where_it_is(self):
        # The new version started, opened the store and is still running, but
        # does not answer /health as health (the last case redirects to a
        # port where nothing listens); and the service manager's stop
        # does nothing, saying so or not. Rolling back would restore a
        # snapshot over, and move away, files a live process has open.
        for stop in ("fail", "lie"):
            for mode in ("alive-500", "alive-garbage", "alive-hang", f"alive-redirect:{free_port()}"):
                with self.subTest(stop=stop, mode=mode):
                    case = self.root / f"case-{stop}-{mode}"
                    case.mkdir()
                    deployment = Deployment(case, self.fake_binary("0.13.0"))
                    self.addCleanup(deployment.stop)
                    deployment.start()
                    deployment.later_stops(stop)
                    archive = self.release("0.13.1", self.fake_binary("0.13.1", mode=mode))
                    database = deployment.db.stat().st_ino

                    result = self.run_upgrade(
                        deployment, archive, "0.13.1", "--command-timeout", "4",
                        health_timeout="3", work_dir=self.backups / f"upgrade-{stop}-{mode}",
                    )

                    # Its data is exactly where it was: nothing set aside,
                    # nothing restored next to it, the same file, its own
                    # write still in it.
                    moved = sorted(path.name for path in deployment.data.glob(".loca-failed-upgrade-*/*"))
                    self.assertEqual(moved, [], "data was moved from under a running server")
                    self.assertTrue(deployment.server_alive(), "the new version is still running")
                    self.assertEqual(list(deployment.data.glob(".loca-rollback-*")), [])
                    self.assertTrue(deployment.db.exists())
                    self.assertEqual(deployment.db.stat().st_ino, database)
                    self.assertIn("0.13.1", deployment.rows("opened_by"))
                    self.assertEqual(deployment.blob.read_bytes(), deployment.blob_content)
                    self.assertEqual(result.returncode, 5, result.stdout + result.stderr)
                    self.assertIn("not known to have stopped", result.stdout)
                    self.assertIn("Nothing was restored or moved", result.stdout)
                    self.assertEqual(deployment.calls(), ["stop", "start", "stop"])

    def test_no_usable_snapshot_refuses_before_anything_is_stopped(self):
        deployment = self.deploy(self.fake_binary("0.13.0"))
        archive = self.release("0.13.1", self.fake_binary("0.13.1"))
        old_binary = sha256(deployment.binary)
        taken = self.backups / "taken"
        taken.mkdir()
        cases = {
            "missing tool": dict(snapshot_tool=self.root / "missing" / "storage_snapshot.py"),
            "work dir exists": dict(work_dir=taken),
        }
        for name, kwargs in cases.items():
            with self.subTest(name):
                result = self.run_upgrade(deployment, archive, "0.13.1", **kwargs)
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn("REFUSED", result.stdout)
                self.assertIn("Nothing was stopped or changed", result.stdout)
                self.assertEqual(deployment.calls(), [])
                self.assertEqual(health(deployment.port)["version"], "0.13.0")
                self.assertEqual(sha256(deployment.binary), old_binary)
        self.assertEqual(list(taken.iterdir()), [])

    def test_snapshot_failing_after_the_stop_restarts_the_unchanged_old_version(self):
        deployment = self.deploy(self.fake_binary("0.13.0"))
        archive = self.release("0.13.1", self.fake_binary("0.13.1"))
        old_binary = sha256(deployment.binary)
        deployment.blob.unlink()  # indexed but missing: the snapshot tool refuses

        result = self.run_upgrade(deployment, archive, "0.13.1")

        self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
        self.assertIn("FAILED before anything was changed", result.stdout)
        self.assertEqual(health(deployment.port)["version"], "0.13.0")
        self.assertEqual(sha256(deployment.binary), old_binary)
        self.assertEqual(deployment.rows("opened_by"), ["0.13.0", "0.13.0"])
        self.assertFalse((self.work_dir / "snapshot").exists())
        self.assertEqual(deployment.calls(), ["stop", "start"])

    def test_archive_that_fails_its_checksum_or_version_is_refused(self):
        deployment = self.deploy(self.fake_binary("0.13.0"))
        cases = {
            "checksum": (self.fake_binary("0.13.1"), dict(corrupt_sum=True), "checksum mismatch"),
            "version": (self.fake_binary("0.13.9"), {}, "reports version 0.13.9, not 0.13.1"),
        }
        for name, (binary, kwargs, message) in cases.items():
            with self.subTest(name):
                archive = self.release("0.13.1", binary, **kwargs)
                result = self.run_upgrade(deployment, archive, "0.13.1")
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn(message, result.stdout)
                self.assertEqual(deployment.calls(), [])
                self.assertFalse(self.work_dir.exists())
                self.assertEqual(health(deployment.port)["version"], "0.13.0")

    def test_dry_run_prints_every_step_and_changes_nothing(self):
        deployment = self.deploy(self.fake_binary("0.13.0"))
        archive = self.release("0.13.1", self.fake_binary("0.13.1"))
        old_binary = sha256(deployment.binary)

        result = self.run_upgrade(deployment, archive, "0.13.1", "--dry-run")

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        for number in range(1, 9):
            self.assertIn(f"[dry-run] step {number}:", result.stdout)
        for text in (f"{deployment.svc} stop", f"{deployment.svc} start", "storage_snapshot.py backup",
                     "ONE WAY: once 0.13.1 has started on this database, no earlier build can open it",
                     "restoring the snapshot this command takes before the upgrade and starting 0.13.0"):
            self.assertIn(text, result.stdout)
        self.assertEqual(deployment.calls(), [])
        self.assertFalse(self.work_dir.exists())
        self.assertEqual(sha256(deployment.binary), old_binary)
        self.assertEqual(health(deployment.port)["version"], "0.13.0")

    def test_arguments_it_would_have_to_guess_are_refused(self):
        deployment = self.deploy(self.fake_binary("0.13.0"))
        cases = {
            "relative work dir": ("0.13.1", ["--work-dir", "relative/upgrade"], "must be an absolute path"),
            "remote health": ("0.13.1", ["--health-url", "http://loca.example.com/health"], "loopback"),
            "not newer": ("0.13.0", [], "the running server is 0.13.0; 0.13.0 is not newer"),
        }
        for name, (version, override, message) in cases.items():
            with self.subTest(name):
                archive = self.release(version, self.fake_binary(version))
                result = self.run_upgrade(deployment, archive, version, *override)
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn(message, result.stdout)
                self.assertEqual(deployment.calls(), [])
                self.assertEqual(health(deployment.port)["version"], "0.13.0")

    def test_waiting_for_the_old_version_does_not_accept_another_one(self):
        deployment = self.deploy(self.fake_binary("0.13.1"))
        url = f"http://127.0.0.1:{deployment.port}/health"
        self.assertIsNone(upgrade.wait_health(url, 1.0, "0.13.0"))
        self.assertEqual(upgrade.wait_health(url, 1.0, "0.13.1")["version"], "0.13.1")

    def test_a_work_dir_that_appears_after_the_checks_is_never_removed(self):
        deployment = self.deploy(self.fake_binary("0.13.0"))
        archive = self.release("0.13.1", self.fake_binary("0.13.1"))
        self.work_dir = self.backups / "appeared"
        self.work_dir.mkdir()
        (self.work_dir / "someone-elses-file").write_text("keep me")
        original = upgrade.Upgrade.check_paths
        # The directory is created by someone else between the check and the
        # extraction: simulated by skipping the check.
        upgrade.Upgrade.check_paths = lambda self: setattr(self, "member_size", 0)
        self.addCleanup(setattr, upgrade.Upgrade, "check_paths", original)
        code = upgrade.main([
            "--archive", str(archive), "--sums", str(self.downloads / "SHA256SUMS"), "--version", "0.13.1",
            "--binary", str(deployment.binary), "--db-path", str(deployment.db),
            "--work-dir", str(self.work_dir), "--health-url", f"http://127.0.0.1:{deployment.port}/health",
            "--stop-command", f"{deployment.svc} stop", "--start-command", f"{deployment.svc} start",
        ])
        self.assertEqual(code, 1)
        self.assertEqual((self.work_dir / "someone-elses-file").read_text(), "keep me")
        self.assertEqual(deployment.calls(), [])


@unittest.skipUnless(
    all(os.environ.get(name) for name in ("LOCA_UPGRADE_E2E_OLD", "LOCA_UPGRADE_E2E_NEW", "LOCA_UPGRADE_E2E_FAILING")),
    "set LOCA_UPGRADE_E2E_OLD/NEW/FAILING to real room-server builds",
)
class RealBinaryUpgradeTests(UpgradeHarness):
    def version_of(self, binary):
        scratch = Path(tempfile.mkdtemp(dir=self.root))
        return upgrade.probe_version(Path(binary), scratch)

    def real_deploy(self):
        deployment = Deployment(self.root, os.environ["LOCA_UPGRADE_E2E_OLD"], with_attachment=False)
        deployment.db.unlink()  # the real server creates its own schema
        self.addCleanup(deployment.stop)
        deployment.start()
        return deployment

    def test_real_old_build_upgrades_to_real_newer_build(self):
        new = os.environ["LOCA_UPGRADE_E2E_NEW"]
        old_version, new_version = self.version_of(os.environ["LOCA_UPGRADE_E2E_OLD"]), self.version_of(new)
        deployment = self.real_deploy()
        self.assertEqual(health(deployment.port)["version"], old_version)
        archive = self.release(new_version, new)

        result = self.run_upgrade(deployment, archive, new_version)

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(health(deployment.port)["version"], new_version)
        self.assertEqual(sha256(deployment.binary), sha256(new))

    def test_real_failing_build_is_rolled_back_onto_the_snapshot(self):
        failing = os.environ["LOCA_UPGRADE_E2E_FAILING"]
        old_version, new_version = self.version_of(os.environ["LOCA_UPGRADE_E2E_OLD"]), self.version_of(failing)
        deployment = self.real_deploy()
        old_binary = sha256(deployment.binary)
        before = deployment.db.stat().st_ino
        archive = self.release(new_version, failing)

        result = self.run_upgrade(deployment, archive, new_version, health_timeout="10")

        self.assertEqual(result.returncode, 4, result.stdout + result.stderr)
        self.assertEqual(health(deployment.port)["version"], old_version)
        self.assertEqual(sha256(deployment.binary), old_binary)
        self.assertNotEqual(deployment.db.stat().st_ino, before)
        self.assertEqual(len(list(deployment.data.glob(".loca-failed-upgrade-*"))), 1)


if __name__ == "__main__":
    unittest.main()
