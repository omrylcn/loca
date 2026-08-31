//! Content-addressed blob store for room attachments (see docs/rfc-room-attachments.md).
//!
//! A blob lives at `<root>/<sha256[:2]>/<sha256>`; the path is derived ONLY from
//! the hash of the bytes, never from any client-supplied name, so it is
//! dedup-by-content and traversal-safe. Writes are atomic (temp file + fsync +
//! rename) and refuse to follow a pre-existing symlink/special file at the target.
//! Lifecycle/refcount/quota accounting lives in the SQLite store; this module is
//! only the bytes on disk.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// The on-disk blob store, rooted at `<STORAGE_ROOT>/attachments`.
#[derive(Debug, Clone)]
pub struct BlobStore {
    root: PathBuf,
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl BlobStore {
    /// Open (creating if needed) a blob store under `root`. Also creates the
    /// `tmp/` staging dir used for atomic writes.
    pub fn new(root: impl AsRef<Path>) -> io::Result<Self> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(root.join("tmp"))?;
        Ok(Self { root })
    }

    /// The final path for a blob. `None` if `sha` is not a well-formed sha256
    /// hex — so a `..`/absolute/garbage id can never escape the root.
    pub fn path_for(&self, sha: &str) -> Option<PathBuf> {
        if !is_sha256_hex(sha) {
            return None;
        }
        Some(self.root.join(&sha[..2]).join(sha))
    }

    /// True only if `sha` names an existing regular blob file.
    pub fn exists(&self, sha: &str) -> bool {
        match self.path_for(sha) {
            Some(p) => fs::symlink_metadata(&p).map(|m| m.is_file()).unwrap_or(false),
            None => false,
        }
    }

    /// Store `bytes`, returning their sha256 hex id. Idempotent: identical bytes
    /// yield the same id and re-store is a cheap no-op (dedup). Atomic: readers
    /// never see a partial blob. Refuses to overwrite a non-regular file.
    pub fn put(&self, bytes: &[u8]) -> io::Result<String> {
        let sha = format!("{:x}", Sha256::digest(bytes));
        let final_path = self
            .path_for(&sha)
            .expect("a sha256 digest is always well-formed hex");

        // Dedup / tamper check: an existing blob must be a regular file. If a
        // symlink or special file squats the path, refuse — never follow it.
        match fs::symlink_metadata(&final_path) {
            Ok(m) if m.is_file() => return Ok(sha), // already stored
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "blob path is not a regular file",
                ))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }

        if let Some(parent) = final_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = self.root.join("tmp").join(unique_name());
        {
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)?;
            f.write_all(bytes)?;
            f.sync_all()?; // durable before the rename publishes it
        }
        // Atomic publish. If another writer won the race, the content is
        // identical (same hash), so either file is correct.
        match fs::rename(&tmp, &final_path) {
            Ok(()) => Ok(sha),
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                Err(e)
            }
        }
    }

    /// Read a blob's bytes by id.
    pub fn read(&self, sha: &str) -> io::Result<Vec<u8>> {
        let p = self
            .path_for(sha)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "malformed blob id"))?;
        // Do not follow a symlink that may have replaced a blob.
        if fs::symlink_metadata(&p)?.is_file() {
            fs::read(&p)
        } else {
            Err(io::Error::new(io::ErrorKind::NotFound, "blob is not a regular file"))
        }
    }
}

fn unique_name() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{}-{}-{}", std::process::id(), nanos, COUNTER.fetch_add(1, Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, BlobStore) {
        let dir = tempfile::tempdir().unwrap();
        let bs = BlobStore::new(dir.path().join("attachments")).unwrap();
        (dir, bs)
    }

    #[test]
    fn put_then_read_roundtrips_and_is_content_addressed() {
        let (_d, bs) = store();
        let sha = bs.put(b"hello attachment").unwrap();
        assert!(is_sha256_hex(&sha));
        assert_eq!(bs.read(&sha).unwrap(), b"hello attachment");
        assert!(bs.exists(&sha));
        // Independently computed digest matches the id.
        assert_eq!(sha, format!("{:x}", Sha256::digest(b"hello attachment")));
    }

    #[test]
    fn identical_bytes_dedup_to_one_id_and_re_put_is_noop() {
        let (_d, bs) = store();
        let a = bs.put(b"same").unwrap();
        let b = bs.put(b"same").unwrap();
        assert_eq!(a, b, "identical bytes must yield the same id");
        // Different bytes -> different id.
        assert_ne!(a, bs.put(b"different").unwrap());
    }

    #[test]
    fn a_malformed_id_can_never_escape_the_root() {
        let (_d, bs) = store();
        for bad in ["../etc/passwd", "/abs", "..", "xyz", &"g".repeat(64), &"a".repeat(63)] {
            assert!(bs.path_for(bad).is_none(), "{bad} must be rejected");
            assert!(!bs.exists(bad));
            assert!(bs.read(bad).is_err());
        }
    }

    #[test]
    #[cfg(unix)]
    fn put_refuses_to_overwrite_a_symlink_at_the_blob_path() {
        let (_d, bs) = store();
        let sha = format!("{:x}", Sha256::digest(b"payload"));
        let target = bs.path_for(&sha).unwrap();
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        // An attacker pre-plants a symlink where the blob would land.
        let outside = _d.path().join("outside");
        fs::write(&outside, b"secret").unwrap();
        std::os::unix::fs::symlink(&outside, &target).unwrap();
        // put must refuse rather than follow the symlink and clobber `outside`.
        assert!(bs.put(b"payload").is_err());
        assert_eq!(fs::read(&outside).unwrap(), b"secret", "the symlink target is untouched");
    }
}
