import hashlib
import importlib.util
import json
from pathlib import Path
import sqlite3
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("storage_snapshot", Path(__file__).resolve().parents[3] / "scripts/storage_snapshot.py")
snapshot = importlib.util.module_from_spec(spec)
spec.loader.exec_module(snapshot)


class SnapshotTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        data = self.root / "data"
        data.mkdir()
        self.db = data / "original.sqlite3"
        self.content = b"proof of an attachment preserved through restore"
        self.sha = hashlib.sha256(self.content).hexdigest()
        self.blob = snapshot.blob_path(data, self.sha)
        self.blob.parent.mkdir(parents=True)
        self.blob.write_bytes(self.content)
        with sqlite3.connect(self.db) as conn:
            conn.executescript("CREATE TABLE attachment_blobs (sha TEXT PRIMARY KEY, size INTEGER); CREATE TABLE messages (text TEXT);")
            conn.execute("INSERT INTO attachment_blobs VALUES (?,?)", (self.sha, len(self.content)))
            conn.execute("INSERT INTO messages VALUES ('restored history')")

    def test_backup_restore_into_empty_directory_preserves_history_and_blob(self):
        target, restored = self.root / "snapshot", self.root / "restore"
        snapshot.backup(self.db, target)
        snapshot.restore(target, restored)
        self.assertEqual(snapshot.blob_path(restored, self.sha).read_bytes(), self.content)
        with sqlite3.connect(restored / "loca.sqlite3") as conn:
            self.assertEqual(conn.execute("SELECT text FROM messages").fetchone()[0], "restored history")
        self.assertEqual(snapshot.validate(target), {self.sha: len(self.content)})

    def test_missing_or_corrupt_blob_never_publishes_success(self):
        for content in (None, b"wrong"):
            if self.blob.exists():
                self.blob.unlink()
            if content is not None:
                self.blob.write_bytes(content)
            with self.assertRaises((ValueError, OSError)):
                snapshot.backup(self.db, self.root / "bad")
            self.assertFalse((self.root / "bad").exists())

    def test_restore_refuses_corruption_and_overwrite(self):
        target = self.root / "snapshot"
        snapshot.backup(self.db, target)
        with self.assertRaises(ValueError):
            snapshot.restore(target, target)
        snapshot.blob_path(target, self.sha).write_bytes(b"tampered")
        with self.assertRaises(ValueError):
            snapshot.restore(target, self.root / "restore")
        self.assertFalse((self.root / "restore").exists())

    def test_manifest_paths_cannot_escape_destination(self):
        target = self.root / "snapshot"
        snapshot.backup(self.db, target)
        manifest = json.loads((target / "manifest.json").read_text())
        manifest["attachments"] = {"../../outside": 1}
        (target / "manifest.json").write_text(json.dumps(manifest))
        with self.assertRaises(ValueError):
            snapshot.restore(target, self.root / "restore")

    def test_symlink_blob_is_rejected(self):
        self.blob.unlink()
        external = self.root / "external"
        external.write_bytes(self.content)
        self.blob.symlink_to(external)
        with self.assertRaises(ValueError):
            snapshot.backup(self.db, self.root / "snapshot")

    def test_non_object_manifest_is_rejected_without_publishing(self):
        target = self.root / "snapshot"
        snapshot.backup(self.db, target)
        (target / "manifest.json").write_text("[]")
        with self.assertRaisesRegex(ValueError, "must be an object"):
            snapshot.restore(target, self.root / "restore")
        self.assertFalse((self.root / "restore").exists())
