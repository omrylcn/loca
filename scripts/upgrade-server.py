#!/usr/bin/env python3
"""Upgrade a running Loca server from a release archive, with one command.

For the binary deployment: the server is the `room-server` file from a
`loca-<version>-linux-amd64.tar.gz` release archive, run by a service manager
(systemd or similar) that you stop and start with your own commands. The
Compose deployment builds its image from a checkout and is not handled here;
see docs/backup-restore.md.

Nothing is guessed. Every path and command comes from an argument:

  scripts/upgrade-server.py \
    --archive /srv/dl/loca-0.14.0-linux-amd64.tar.gz \
    --sums /srv/dl/SHA256SUMS --version 0.14.0 \
    --binary /usr/local/bin/room-server --db-path /var/lib/loca/loca.db \
    --work-dir /var/backups/loca-upgrade-0.14.0 \
    --health-url http://127.0.0.1:8787/health \
    --stop-command 'systemctl stop loca' --start-command 'systemctl start loca'

Order: check the archive against SHA256SUMS, check every path, the running
version and the space; extract the new binary and ask it for its version
(memory-only, loopback, random port); stop the server; take and verify a
snapshot with scripts/storage_snapshot.py; install the new binary; start it;
wait for /health and its version. Any failure before that point puts the old
binary back on the snapshot taken before the upgrade. Rolling back DISCARDS
everything written after the upgrade started: it is moved aside, not deleted.

Exit codes: 0 upgraded; 1 refused, nothing was stopped or changed; 2 usage;
3 stopped, nothing changed, old version running again; 4 rolled back to the
pre-upgrade snapshot; 5 the server is not back: act by hand (see output).
"""
import argparse
import hashlib
import ipaddress
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import socket
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

REFUSED, ABORTED, ROLLED_BACK, MANUAL = 1, 3, 4, 5
DISCARD_WARNING = (
    "Rolling back restores the snapshot taken before the upgrade: everything "
    "written after the upgrade started is discarded from the live data "
    "(moved aside, not deleted)."
)
ONE_WAY_NOTICE = (
    "ONE WAY: once {new} has started on this database, no earlier build can open it "
    "(see docs/backup-restore.md). Going back to {old}, now or later, is only possible by restoring "
    "the snapshot this command takes before the upgrade and starting {old} on that copy; "
    "everything written after the upgrade started is then discarded."
)


class Refused(Exception):
    """A check failed before anything was stopped or changed."""


class StepFailed(Exception):
    """A step after the server was stopped failed."""


def say(message):
    print(message, flush=True)


def sha256(path):
    with open(path, "rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def version_tuple(text):
    match = re.match(r"^(\d+)\.(\d+)\.(\d+)", text or "")
    return tuple(int(part) for part in match.groups()) if match else None


def loopback_url(url):
    parsed = urllib.parse.urlsplit(url)
    if parsed.scheme != "http" or not parsed.hostname:
        return False
    if parsed.hostname == "localhost":
        return True
    try:
        return ipaddress.ip_address(parsed.hostname).is_loopback
    except ValueError:
        return False


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    """A redirect is an answer, not somewhere to go: the health answer of the
    server at this address is what is asked, never another server's."""

    def redirect_request(self, *args, **kwargs):
        return None


# Asks exactly the address it is given: no proxy from the environment, and
# no redirect followed.
_DIRECT = urllib.request.build_opener(urllib.request.ProxyHandler({}), _NoRedirect())


def get_health(url, timeout=2.0):
    """The /health JSON of the server at `url` itself, or None."""
    try:
        with _DIRECT.open(url, timeout=timeout) as response:
            payload = json.loads(response.read().decode("utf-8"))
    except (OSError, ValueError, urllib.error.URLError):
        return None
    return payload if isinstance(payload, dict) else None


def wait_health(url, timeout, version=None):
    """The first answer with ok:true (and, if given, this version), or None."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        answer = get_health(url)
        if answer and answer.get("ok") is True and (version is None or answer.get("version") == version):
            return answer
        time.sleep(0.25)
    return None


def nothing_listens(url, timeout=1.0):
    """True only when a TCP connection to the URL's own host and port is refused.

    No HTTP is spoken, so nothing the process answers can be mistaken for its
    absence: not an error status, not an answer that is not the health JSON,
    not a redirect to a port where nothing listens (an HTTP client would
    follow it and report that port's refusal), and no proxy stands between.
    A connection that is accepted, or an attempt that times out, means a
    process still holds the port, and its data must not be copied or moved
    from under it.
    """
    parts = urllib.parse.urlsplit(url)
    try:
        connection = socket.create_connection((parts.hostname, parts.port or 80), timeout=timeout)
    except ConnectionRefusedError:
        return True
    except OSError:
        return False
    connection.close()
    return False


def wait_down(url, timeout):
    """True once the port refuses connections twice in a row, within `timeout`."""
    deadline = time.monotonic() + timeout
    refused = 0
    while time.monotonic() < deadline:
        refused = refused + 1 if nothing_listens(url) else 0
        if refused >= 2:
            return True
        time.sleep(0.25)
    return False


def run_command(command, timeout):
    argv = shlex.split(command)
    try:
        result = subprocess.run(argv, timeout=timeout, check=False)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise StepFailed(f"`{command}` failed: {error}") from error
    if result.returncode != 0:
        raise StepFailed(f"`{command}` exited with {result.returncode}")


def probe_version(binary, scratch, timeout=15.0):
    """Start a binary memory-only on a random loopback port; return its version.

    The environment is built from nothing: no DB_PATH (so no data is opened),
    no ADMIN_CONSOLE_PORT, loopback only, the OS picks the port.
    """
    port_file = scratch / "probe-port"
    env = {
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        "PORT": "0",
        "BIND_ADDR": "127.0.0.1",
        "BOUND_PORT_FILE": str(port_file),
        "RUST_LOG": "warn",
    }
    log = (scratch / "probe.log").open("wb")
    try:
        process = subprocess.Popen([str(binary)], env=env, stdout=log, stderr=subprocess.STDOUT, cwd=scratch)
    except OSError as error:
        log.close()
        raise Refused(f"{binary} does not run here: {error}") from error
    try:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if process.poll() is not None:
                raise Refused(f"{binary} exited with {process.returncode} before answering /health")
            text = port_file.read_text().strip() if port_file.is_file() else ""
            if text.isdigit():
                health = wait_health(f"http://127.0.0.1:{text}/health", max(0.5, deadline - time.monotonic()))
                if health and health.get("ok") is True and isinstance(health.get("version"), str):
                    return health["version"]
            time.sleep(0.1)
        raise Refused(f"{binary} did not answer /health within {timeout:.0f}s")
    finally:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
        log.close()
        port_file.unlink(missing_ok=True)


def tree_size(path):
    total = 0
    if path.is_file():
        return path.stat().st_size
    for root, _dirs, files in os.walk(path):
        for name in files:
            entry = Path(root) / name
            if not entry.is_symlink():
                total += entry.stat().st_size
    return total


def install_file(source, target, like):
    """Atomically replace `target` with `source`, keeping `like`'s mode and owner."""
    info = like.stat()
    with tempfile.NamedTemporaryFile(dir=target.parent, prefix=".room-server.upgrade-", delete=False) as temp:
        staged = Path(temp.name)
    try:
        shutil.copyfile(source, staged)
        os.chmod(staged, info.st_mode & 0o7777)
        if os.geteuid() == 0:
            os.chown(staged, info.st_uid, info.st_gid)
        if sha256(staged) != sha256(source):
            raise StepFailed(f"copy of {source} does not match its source")
        os.replace(staged, target)
    finally:
        staged.unlink(missing_ok=True)


def chown_tree(path, uid, gid):
    if os.geteuid() != 0:
        return
    os.chown(path, uid, gid)
    if path.is_dir():
        for root, dirs, files in os.walk(path):
            for name in dirs + files:
                os.chown(Path(root) / name, uid, gid)


class Upgrade:
    def __init__(self, args):
        self.args = args
        self.work = args.work_dir
        self.snapshot = args.work_dir / "snapshot"
        self.new_binary = args.work_dir / "new" / "room-server"
        self.previous_binary = args.work_dir / "previous" / "room-server"
        self.member = f"loca-{args.version}-linux-amd64/room-server"
        self.old_version = None
        self.created_work = False
        self.stamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())

    # ---- checks that change nothing -------------------------------------
    def check_arguments(self):
        a = self.args
        for name in ("archive", "sums", "binary", "db_path", "work_dir", "snapshot_tool"):
            if not getattr(a, name).is_absolute():
                raise Refused(f"--{name.replace('_', '-')} must be an absolute path")
        if not loopback_url(a.health_url):
            raise Refused("--health-url must be an http:// URL on a loopback address")
        if version_tuple(a.version) is None:
            raise Refused(f"--version {a.version!r} is not a release version")
        for name in ("stop_command", "start_command"):
            if not shlex.split(getattr(a, name)):
                raise Refused(f"--{name.replace('_', '-')} is empty")

    def check_archive(self):
        a = self.args
        if not a.archive.is_file() or not a.sums.is_file():
            raise Refused("--archive and --sums must be existing files")
        wanted = a.archive.name
        listed = []
        for line in a.sums.read_text(encoding="utf-8").splitlines():
            parts = line.split(None, 1)
            if len(parts) == 2 and parts[1].lstrip("*").removeprefix("./") == wanted:
                listed.append(parts[0].lower())
        if len(listed) != 1:
            raise Refused(f"{a.sums} must list {wanted} exactly once (found {len(listed)})")
        actual = sha256(a.archive)
        if actual != listed[0]:
            raise Refused(f"checksum mismatch for {wanted}: SHA256SUMS says {listed[0]}, the file is {actual}")
        say(f"  checksum ok: {wanted} sha256 {actual}")
        with tarfile.open(a.archive, "r:gz") as archive:
            members = [m for m in archive.getmembers() if m.name.removeprefix("./") == self.member]
        if len(members) != 1 or not members[0].isfile():
            raise Refused(f"the archive does not contain exactly one regular file {self.member}")
        self.member_size = members[0].size

    def check_running(self):
        a = self.args
        health = get_health(a.health_url)
        if not health or health.get("ok") is not True or not isinstance(health.get("version"), str):
            raise Refused(f"no healthy server answers {a.health_url}; refusing to upgrade what cannot be checked")
        self.old_version = health["version"]
        old, new = version_tuple(self.old_version), version_tuple(a.version)
        if old is None:
            raise Refused(f"the running server reports version {self.old_version!r}, which cannot be compared")
        if new <= old:
            raise Refused(f"the running server is {self.old_version}; {a.version} is not newer")
        say(f"  running server reports {self.old_version}")

    def check_paths(self):
        a = self.args
        for label, path in (("--binary", a.binary), ("--db-path", a.db_path), ("--snapshot-tool", a.snapshot_tool)):
            if path.is_symlink() or not path.is_file():
                raise Refused(f"{label} {path} must be an existing regular file (not a symlink)")
        for label, directory in (("--binary", a.binary.parent), ("--db-path", a.db_path.parent)):
            if not os.access(directory, os.W_OK | os.X_OK):
                raise Refused(f"the directory of {label} ({directory}) is not writable")
        if sys.version_info < (3, 11):
            raise Refused("the snapshot tool needs Python 3.11 or newer")
        tool = subprocess.run([sys.executable, str(a.snapshot_tool), "--help"], capture_output=True, check=False)
        if tool.returncode != 0:
            raise Refused(f"the snapshot tool {a.snapshot_tool} does not run")
        if a.work_dir.exists() or a.work_dir.is_symlink():
            raise Refused(f"--work-dir {a.work_dir} already exists; choose a new one for each upgrade")
        parent = a.work_dir.parent
        if not parent.is_dir() or not os.access(parent, os.W_OK | os.X_OK):
            raise Refused(f"cannot create --work-dir: {parent} is not a writable directory")
        data = sum(tree_size(p) for p in self.data_paths() if p.exists())
        free_work = shutil.disk_usage(parent).free
        need_work = data + self.member_size + a.binary.stat().st_size
        if free_work < need_work:
            raise Refused(f"not enough space for the snapshot under {parent}: {free_work} bytes free, {need_work} needed")
        database = sum(tree_size(p) for p in self.data_paths()[:-1] if p.exists())
        free_data = shutil.disk_usage(a.db_path.parent).free
        if free_data < 2 * database:
            raise Refused(
                f"{a.db_path.parent} has {free_data} bytes free; the first start of a new release can need "
                f"twice the database ({2 * database} bytes), see docs/backup-restore.md"
            )
        say(f"  paths ok; data {data} bytes, {free_work} bytes free for the snapshot")

    def data_paths(self):
        db = self.args.db_path
        return [db, Path(f"{db}-wal"), Path(f"{db}-shm"), Path(f"{db}-journal"), db.parent / "attachments"]

    # ---- the steps ------------------------------------------------------
    def steps(self):
        a = self.args
        return [
            f"create {self.work} (mode 0700); extract {self.member} to {self.new_binary}; "
            f"copy {a.binary} to {self.previous_binary}",
            f"start {self.new_binary} memory-only on a random loopback port and require /health version {a.version}; "
            f"start {a.binary} the same way and require the running version",
            f"stop the server: {a.stop_command}; wait until {a.health_url} no longer answers",
            f"snapshot: {sys.executable} {a.snapshot_tool} backup {a.db_path} {self.snapshot} --server-stopped; "
            f"then verify {self.snapshot}",
            f"install {self.new_binary} at {a.binary} (atomic rename, same mode and owner)",
            f"start the server: {a.start_command}",
            f"wait up to {a.health_timeout}s for {a.health_url} to report ok and version {a.version}",
            "on any failure from the stop on: stop, restore the snapshot beside the database, move the "
            f"current data aside under {a.db_path.parent}/.loca-failed-upgrade-<time>, put the restored "
            f"data at {a.db_path}, put {self.previous_binary} back at {a.binary}, start, and require the old version",
        ]

    def execute(self):
        a = self.args
        say("Preflight (nothing is stopped or changed):")
        try:
            self.check_arguments()
            self.check_archive()
            self.check_running()
            self.check_paths()
        except (OSError, ValueError, tarfile.TarError) as error:
            raise Refused(str(error)) from error
        say(f"Upgrade {self.old_version} -> {a.version}.")
        say(ONE_WAY_NOTICE.format(new=a.version, old=self.old_version))
        if a.dry_run:
            for number, step in enumerate(self.steps(), 1):
                say(f"[dry-run] step {number}: {step}")
            say("[dry-run] nothing was stopped, written or changed")
            return 0
        steps = self.steps()

        say(f"[1/7] {steps[0]}")
        try:
            self.prepare()
            say(f"[2/7] {steps[1]}")
            probed = probe_version(self.new_binary, self.work)
            if probed != a.version:
                raise Refused(f"the new binary reports version {probed}, not {a.version}")
            current = probe_version(self.previous_binary, self.work)
            if current != self.old_version:
                raise Refused(
                    f"{a.binary} reports {current} but the running server reports {self.old_version}; "
                    "--binary is not the file the service runs"
                )
        except (Refused, OSError, tarfile.TarError, StopIteration) as error:
            if self.created_work:  # never remove a directory this run did not create
                shutil.rmtree(self.work, ignore_errors=True)
            raise Refused(str(error) or type(error).__name__) from error

        say(f"[3/7] {steps[2]}")
        try:
            run_command(a.stop_command, a.command_timeout)
            if not wait_down(a.health_url, a.command_timeout):
                raise StepFailed(
                    f"something still holds {a.health_url} after the stop command; "
                    "the server is not known to have stopped"
                )
            say(f"[4/7] {steps[3]}")
            snapshot = [sys.executable, str(a.snapshot_tool)]
            for argv in (snapshot + ["backup", str(a.db_path), str(self.snapshot), "--server-stopped"],
                         snapshot + ["verify", str(self.snapshot)]):
                if subprocess.run(argv, check=False).returncode != 0:
                    raise StepFailed("the snapshot failed")
        except StepFailed as error:
            return self.restart_unchanged(error)

        try:
            say(f"[5/7] {steps[4]}")
            install_file(self.new_binary, a.binary, a.binary)
            say(f"[6/7] {steps[5]}")
            run_command(a.start_command, a.command_timeout)
            say(f"[7/7] {steps[6]}")
            health = wait_health(a.health_url, a.health_timeout)
            if not health or health.get("ok") is not True:
                raise StepFailed(f"{a.health_url} did not report ok within {a.health_timeout}s")
            if health.get("version") != a.version:
                raise StepFailed(f"the server reports version {health.get('version')!r}, not {a.version}")
        except (StepFailed, OSError) as error:
            return self.roll_back(error)
        say(
            f"UPGRADED {self.old_version} -> {a.version}. Keep the verified snapshot {self.snapshot} and the "
            f"previous binary {self.previous_binary} until you would no longer go back. To go back later, "
            "follow docs/backup-restore.md: everything written after this upgrade is then lost."
        )
        return 0

    def prepare(self):
        a = self.args
        self.work.mkdir(mode=0o700)
        self.created_work = True
        for sub in ("new", "previous"):
            (self.work / sub).mkdir(mode=0o700)
        with tarfile.open(a.archive, "r:gz") as archive:
            member = next(m for m in archive.getmembers() if m.name.removeprefix("./") == self.member)
            source = archive.extractfile(member)
            if source is None:
                raise Refused(f"cannot read {self.member} from the archive")
            with source, self.new_binary.open("xb") as target:
                shutil.copyfileobj(source, target)
        os.chmod(self.new_binary, 0o755)
        shutil.copy2(a.binary, self.previous_binary)
        if sha256(self.previous_binary) != sha256(a.binary):
            raise Refused(f"the copy of {a.binary} does not match it")

    def restart_unchanged(self, error):
        a = self.args
        say(f"FAILED before anything was changed: {error}")
        say(f"Starting the unchanged old version again: {a.start_command}")
        try:
            run_command(a.start_command, a.command_timeout)
        except StepFailed as start_error:
            say(f"MANUAL ACTION NEEDED: {start_error}. Binary and data are unchanged; start the service by hand.")
            return MANUAL
        health = wait_health(a.health_url, a.health_timeout, self.old_version)
        if not health:
            say(f"MANUAL ACTION NEEDED: {a.health_url} does not report {self.old_version}. "
                "Binary and data are unchanged.")
            return MANUAL
        say(f"ABORTED: {self.old_version} is running again on unchanged data. Nothing was upgraded.")
        return ABORTED

    def roll_back(self, error):
        a = self.args
        parent = a.db_path.parent
        restored = parent / f".loca-rollback-{self.stamp}"
        aside = parent / f".loca-failed-upgrade-{self.stamp}"
        say(f"FAILED: {error}")
        say(f"ROLLING BACK to {self.old_version} on the snapshot {self.snapshot}. {DISCARD_WARNING}")
        # Nothing is restored, moved or installed unless the server is known to
        # have stopped: the stop command succeeded AND the port refuses
        # connections. A stop command that failed, or a port that still
        # accepts (a hung process, an HTTP error, an answer that is not the
        # health JSON), means a process may still have the database open.
        try:
            run_command(a.stop_command, a.command_timeout)
            if not wait_down(a.health_url, a.command_timeout):
                raise StepFailed(f"something still holds {a.health_url} after the stop command")
        except StepFailed as stop_error:
            say(f"MANUAL ACTION NEEDED: the server is not known to have stopped: {stop_error}")
            say("  Nothing was restored or moved: the database, its -wal/-shm and attachments are where")
            say(f"  they were, and {a.binary} is still the new version. Stop the service by hand, make sure")
            say("  no room-server process is left, then follow docs/backup-restore.md 'Rolling back is")
            say("  restoring a snapshot' with:")
            say(f"  pre-upgrade snapshot (verified): {self.snapshot}")
            say(f"  previous binary: {self.previous_binary}")
            return MANUAL
        try:
            snapshot = [sys.executable, str(a.snapshot_tool)]
            for argv in (snapshot + ["restore", str(self.snapshot), str(restored), "--server-stopped"],
                         snapshot + ["verify", str(restored)]):
                if subprocess.run(argv, check=False).returncode != 0:
                    raise StepFailed(f"restoring {self.snapshot} into {restored} failed")
            owner = a.db_path.stat()
            aside.mkdir(mode=0o700)
            for path in self.data_paths():
                if path.exists() or path.is_symlink():
                    path.rename(aside / path.name)
            say(f"  the data written after the upgrade started is kept aside in {aside}")
            # The tool's read-only checks leave SQLite's -wal/-shm beside the
            # copy; they move with it under the service's database name.
            for suffix in ("", "-wal", "-shm"):
                source = restored / f"loca.sqlite3{suffix}"
                if source.exists():
                    target = Path(f"{a.db_path}{suffix}")
                    os.rename(source, target)
                    chown_tree(target, owner.st_uid, owner.st_gid)
            if (restored / "attachments").exists():
                os.rename(restored / "attachments", parent / "attachments")
                chown_tree(parent / "attachments", owner.st_uid, owner.st_gid)
            (restored / "manifest.json").unlink()
            restored.rmdir()
            install_file(self.previous_binary, a.binary, a.binary)
            run_command(a.start_command, a.command_timeout)
            health = wait_health(a.health_url, a.health_timeout, self.old_version)
            if not health:
                raise StepFailed(f"{a.health_url} does not report {self.old_version} after the rollback")
        except (StepFailed, OSError, ValueError) as rollback_error:
            say(f"MANUAL ACTION NEEDED: the rollback stopped: {rollback_error}")
            say(f"  pre-upgrade snapshot (verified): {self.snapshot}")
            say(f"  previous binary: {self.previous_binary}")
            say(f"  data written after the upgrade started, if moved: {aside}")
            say("  follow docs/backup-restore.md 'Rolling back is restoring a snapshot'.")
            return MANUAL
        say(
            f"ROLLED BACK: {self.old_version} is running on the snapshot taken before the upgrade. "
            f"Everything written after the upgrade started is gone from the live data; it is kept in {aside}."
        )
        return ROLLED_BACK


def main(argv=None):
    here = Path(__file__).resolve().parent
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--archive", type=Path, required=True, help="loca-<version>-linux-amd64.tar.gz")
    parser.add_argument("--sums", type=Path, required=True, help="SHA256SUMS from the same release")
    parser.add_argument("--version", required=True, help="the version you are installing, e.g. 0.14.0")
    parser.add_argument("--binary", type=Path, required=True, help="the room-server file your service runs")
    parser.add_argument("--db-path", type=Path, required=True, help="the DB_PATH your service uses")
    parser.add_argument("--work-dir", type=Path, required=True, help="new directory for the snapshot and binaries")
    parser.add_argument("--health-url", required=True, help="the running server's loopback /health URL")
    parser.add_argument("--stop-command", required=True, help="stops the service, e.g. 'systemctl stop loca'")
    parser.add_argument("--start-command", required=True, help="starts the service, e.g. 'systemctl start loca'")
    parser.add_argument("--snapshot-tool", type=Path, default=here / "storage_snapshot.py",
                        help="storage_snapshot.py (default: the one beside this script)")
    parser.add_argument("--health-timeout", type=float, default=120.0, help="seconds to wait for health")
    parser.add_argument("--command-timeout", type=float, default=60.0,
                        help="seconds for each stop/start command and for the server to stop answering")
    parser.add_argument("--dry-run", action="store_true", help="run the checks and print every step; change nothing")
    args = parser.parse_args(argv)
    try:
        return Upgrade(args).execute()
    except Refused as error:
        say(f"REFUSED: {error}. Nothing was stopped or changed.")
        return REFUSED


if __name__ == "__main__":
    sys.exit(main())
