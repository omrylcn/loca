//! Content-addressed blob store for room attachments (see docs/rfc-room-attachments.md).
//!
//! A blob lives at `<root>/<sha256[:2]>/<sha256>`; the path is derived ONLY from
//! the hash of the bytes, never from any client-supplied name, so it is
//! dedup-by-content and traversal-safe. The disk layer is hardened to match the
//! room-scoped HTTP auth above it:
//!
//! - **Integrity**: `read()` verifies the bytes hash back to the requested id, so
//!   a corrupted or swapped file is never served; `put()` self-heals a corrupt
//!   dedup target instead of trusting its mere existence.
//! - **No symlink follow**: every internal directory (root, `tmp/`, shard) is
//!   verified to be a real directory (not a symlink), and reads open with
//!   `O_NOFOLLOW`, so a planted symlink can never redirect a write or a read
//!   outside the tree.
//! - **Atomic + durable + private**: writes go temp (`0600`) → fsync → rename,
//!   then the parent dir is fsync'd; dirs are `0700`; a failed write cleans its
//!   temp file.
//!
//! Lifecycle/refcount/quota accounting lives in the SQLite store; this module is
//! only the bytes on disk.

use std::fs;
use std::io::{self, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

const DIR_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;
/// A stored blob can never exceed the upload cap; bound reads so a symlink swap
/// to a giant file can't be slurped. Any file that hashes to a valid id is well
/// within this, so a real blob is never rejected by the bound.
const MAX_BLOB_BYTES: u64 = 32 * 1024 * 1024;

/// The on-disk blob store, rooted at `<STORAGE_ROOT>/attachments`.
#[derive(Debug, Clone)]
pub struct BlobStore {
    root: PathBuf,
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn set_mode(_path: &Path, _mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    fs::set_permissions(_path, fs::Permissions::from_mode(_mode))?;
    Ok(())
}

/// Ensure `path` is a REAL directory we own (`0700`), creating it if missing.
/// A symlink or special file at that path is refused — a write is never made
/// through it, so `tmp/` or a shard dir cannot redirect bytes out of the tree.
fn ensure_dir(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "attachment directory is a symlink",
        )),
        // An existing real directory: (re)assert 0700 — a dir left 0755 by an
        // older build or a wider umask is tightened, not trusted as-is.
        Ok(m) if m.is_dir() => set_mode(path, DIR_MODE),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "attachment path is not a directory",
        )),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(path)?;
            set_mode(path, DIR_MODE)
        }
        Err(e) => Err(e),
    }
}

/// fsync a directory so a create/rename inside it is durable across a crash.
fn fsync_dir(_path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    fs::File::open(_path)?.sync_all()?;
    Ok(())
}

impl BlobStore {
    /// Open (creating if needed) a blob store under `root`, with a `tmp/` staging
    /// dir. Both are verified to be real directories with `0700` perms.
    pub fn new(root: impl AsRef<Path>) -> io::Result<Self> {
        let root = root.as_ref().to_path_buf();
        ensure_dir(&root)?;
        set_mode(&root, DIR_MODE)?;
        ensure_dir(&root.join("tmp"))?;
        Ok(Self { root })
    }

    /// The final path for a blob, or `None` if `sha` is not well-formed sha256
    /// hex — so a `..`/absolute/garbage id can never escape the root.
    pub fn path_for(&self, sha: &str) -> Option<PathBuf> {
        if !is_sha256_hex(sha) {
            return None;
        }
        Some(self.root.join(&sha[..2]).join(sha))
    }

    /// True only if `sha` names an existing regular blob file (not verified).
    // A physical-presence probe kept as part of the BlobStore surface and
    // exercised by the unit tests (a put lands a real file; a malformed id is
    // never a path). No production caller yet — the lifecycle decides presence
    // from the DB index, not the filesystem — so allow it until one appears.
    #[allow(dead_code)]
    pub fn exists(&self, sha: &str) -> bool {
        match self.path_for(sha) {
            Some(p) => fs::symlink_metadata(&p)
                .map(|m| m.is_file())
                .unwrap_or(false),
            None => false,
        }
    }

    /// Store `bytes`, returning their sha256 hex id. Idempotent (dedup); an intact
    /// existing blob is a no-op, a CORRUPT one is self-healed. Atomic + durable +
    /// `0600`. Refuses a symlink/special file at the blob path.
    pub fn put(&self, bytes: &[u8]) -> io::Result<String> {
        let sha = format!("{:x}", Sha256::digest(bytes));
        // Re-verify the directory chain on every write (cheap; catches a dir that
        // was swapped for a symlink since `new`).
        ensure_dir(&self.root)?;
        ensure_dir(&self.root.join("tmp"))?;
        let shard = self.root.join(&sha[..2]);
        ensure_dir(&shard)?;
        let final_path = shard.join(&sha);

        match fs::symlink_metadata(&final_path) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "blob path is a symlink",
                ))
            }
            Ok(m) if m.is_file() => {
                // Dedup ONLY if the existing bytes actually hash to this id;
                // otherwise it is corrupt/tampered — fall through and self-heal.
                if self.read_at(&final_path, Some(&sha)).is_ok() {
                    return Ok(sha);
                }
            }
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "blob path is not a regular file",
                ))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }

        let tmp = self.root.join("tmp").join(unique_name());
        // Removes the temp file on ANY early return below (open/write/sync/chmod/
        // publish error). After a successful rename the temp is already gone, so
        // the drop is a harmless no-op.
        let _guard = TempGuard(tmp.clone());
        {
            let mut opts = fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            opts.mode(FILE_MODE);
            let mut f = opts.open(&tmp)?;
            f.write_all(bytes)?;
            f.sync_all()?; // durable before the rename publishes it
        }
        set_mode(&tmp, FILE_MODE)?; // now covered by the guard too
        self.publish(&tmp, &final_path, &sha)?;
        fsync_dir(&shard)?; // the rename itself must survive a crash
        Ok(sha)
    }

    /// Publish `tmp` at `final_path` (an atomic rename on unix). Tolerates the
    /// two cross-platform edge cases loca-dev flagged: a concurrent writer that
    /// published this exact blob first, and Windows `rename` refusing to replace
    /// an existing file. Windows atomicity is authoritatively verified by the
    /// Windows CI runner; here we keep the store consistent on all platforms.
    fn publish(&self, tmp: &Path, final_path: &Path, sha: &str) -> io::Result<()> {
        match fs::rename(tmp, final_path) {
            Ok(()) => Ok(()),
            Err(e) => {
                // Someone (or an earlier self) already published the correct
                // bytes — the store is consistent; our temp is cleaned by the guard.
                if self.read_at(final_path, Some(sha)).is_ok() {
                    return Ok(());
                }
                // A stale/corrupt regular file blocks the rename (self-heal, and
                // Windows non-replacing rename): drop it, then rename.
                if fs::symlink_metadata(final_path)
                    .map(|m| m.is_file())
                    .unwrap_or(false)
                {
                    fs::remove_file(final_path)?;
                    fs::rename(tmp, final_path)
                } else {
                    Err(e)
                }
            }
        }
    }

    /// Read `path` without following a final symlink; if `expect` is given, verify
    /// the bytes hash to it (so a corrupt/swapped file yields an error, never
    /// wrong bytes). Bounded so a swapped-in giant file can't be slurped.
    fn read_at(&self, path: &Path, expect: Option<&str>) -> io::Result<Vec<u8>> {
        let buf = self.read_no_follow(path)?;
        if let Some(sha) = expect {
            if format!("{:x}", Sha256::digest(&buf)) != sha {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "blob content does not match its id (corrupt/tampered)",
                ));
            }
        }
        Ok(buf)
    }

    /// Read the raw bytes at `path` without following a final symlink. On unix
    /// `O_NOFOLLOW` makes the refusal atomic at open; on other platforms a
    /// symlink/reparse point is refused by metadata (the content-hash verify in
    /// `read_at` closes the residual TOCTOU window, and the Windows CI runner
    /// authoritatively checks the Windows path).
    #[cfg(unix)]
    fn read_no_follow(&self, path: &Path) -> io::Result<Vec<u8>> {
        let mut opts = fs::OpenOptions::new();
        opts.read(true).custom_flags(libc::O_NOFOLLOW);
        let mut f = opts.open(path)?;
        let meta = f.metadata()?;
        if !meta.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "blob is not a regular file",
            ));
        }
        if meta.len() > MAX_BLOB_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "blob exceeds the size bound",
            ));
        }
        let mut buf = Vec::with_capacity(meta.len() as usize);
        f.read_to_end(&mut buf)?;
        Ok(buf)
    }

    #[cfg(not(unix))]
    fn read_no_follow(&self, path: &Path) -> io::Result<Vec<u8>> {
        let meta = fs::symlink_metadata(path)?;
        if meta.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "blob path is a symlink",
            ));
        }
        if !meta.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "blob is not a regular file",
            ));
        }
        if meta.len() > MAX_BLOB_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "blob exceeds the size bound",
            ));
        }
        fs::read(path)
    }

    /// Read a blob's bytes by id, verifying integrity (id == sha256(bytes)).
    pub fn read(&self, sha: &str) -> io::Result<Vec<u8>> {
        let p = self
            .path_for(sha)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "malformed blob id"))?;
        self.read_at(&p, Some(sha))
    }

    /// Delete a blob's file. Used by the lifecycle sweep once a blob is
    /// unreferenced (no pending, no ref). A missing file is success — the goal
    /// state (gone) is reached — so the sweep is idempotent and never wedges on
    /// an already-collected blob. A malformed id is a no-op (never a path).
    pub fn delete(&self, sha: &str) -> io::Result<()> {
        let Some(p) = self.path_for(sha) else {
            return Ok(());
        };
        match fs::remove_file(&p) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// Removes a temp file on drop unless the rename already consumed it — so every
/// pre-publish error path (open/write/sync/chmod/publish) leaves no orphan.
struct TempGuard(PathBuf);
impl Drop for TempGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn unique_name() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!(
        "{}-{}-{}",
        std::process::id(),
        nanos,
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
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
        assert_eq!(sha, format!("{:x}", Sha256::digest(b"hello attachment")));
    }

    #[test]
    fn identical_bytes_dedup_to_one_id_and_re_put_is_noop() {
        let (_d, bs) = store();
        let a = bs.put(b"same").unwrap();
        assert_eq!(a, bs.put(b"same").unwrap());
        assert_ne!(a, bs.put(b"different").unwrap());
    }

    #[test]
    fn a_malformed_id_can_never_escape_the_root() {
        let (_d, bs) = store();
        for bad in [
            "../etc/passwd",
            "/abs",
            "..",
            "xyz",
            &"g".repeat(64),
            &"a".repeat(63),
        ] {
            assert!(bs.path_for(bad).is_none(), "{bad} must be rejected");
            assert!(!bs.exists(bad));
            assert!(bs.read(bad).is_err());
        }
    }

    #[test]
    fn read_rejects_a_corrupt_blob_and_put_self_heals_it() {
        let (_d, bs) = store();
        let sha = bs.put(b"authentic").unwrap();
        let path = bs.path_for(&sha).unwrap();
        // Tamper the stored bytes in place.
        fs::write(&path, b"tampered!").unwrap();
        // read() must refuse to serve wrong bytes (content != id).
        assert!(bs.read(&sha).is_err(), "a corrupt blob must not be served");
        // put() of the authentic bytes self-heals the corrupt file.
        assert_eq!(bs.put(b"authentic").unwrap(), sha);
        assert_eq!(bs.read(&sha).unwrap(), b"authentic");
    }

    #[test]
    #[cfg(unix)]
    fn put_refuses_a_symlinked_shard_dir_and_leaves_the_target_untouched() {
        let (_d, bs) = store();
        let sha = format!("{:x}", Sha256::digest(b"x"));
        // Plant the shard dir <sha[:2]> as a symlink to an external directory.
        let outside = _d.path().join("outside");
        fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, bs.path_for(&sha).unwrap().parent().unwrap()).unwrap();
        assert!(
            bs.put(b"x").is_err(),
            "a symlinked shard dir must be refused"
        );
        assert!(
            fs::read_dir(&outside).unwrap().next().is_none(),
            "external dir untouched"
        );
    }

    #[test]
    #[cfg(unix)]
    fn read_does_not_follow_a_symlink_swapped_over_a_blob() {
        let (_d, bs) = store();
        let sha = bs.put(b"real bytes").unwrap();
        let path = bs.path_for(&sha).unwrap();
        // The symlink target holds the SAME bytes, so the content-hash check would
        // PASS if the link were followed — only O_NOFOLLOW causes the refusal,
        // which isolates the no-follow guarantee from the integrity guarantee.
        let outside = _d.path().join("copy");
        fs::write(&outside, b"real bytes").unwrap();
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&outside, &path).unwrap();
        assert!(
            bs.read(&sha).is_err(),
            "a symlinked blob path must be refused, not followed"
        );
    }

    #[test]
    #[cfg(unix)]
    fn an_existing_wide_permission_dir_is_tightened_to_0700() {
        let (_d, bs) = store();
        let sha = format!("{:x}", Sha256::digest(b"tighten"));
        let shard = bs.path_for(&sha).unwrap().parent().unwrap().to_path_buf();
        // Pre-create the shard AND tmp dirs world-readable (an older build / wide umask).
        fs::create_dir_all(&shard).unwrap();
        set_mode(&shard, 0o755).unwrap();
        set_mode(&bs.root.join("tmp"), 0o755).unwrap();
        let mode = |p: &Path| fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&shard), 0o755, "precondition: shard is wide");
        // A put re-asserts 0700 on the existing dirs, not just fresh ones.
        bs.put(b"tighten").unwrap();
        assert_eq!(
            mode(&shard),
            DIR_MODE,
            "existing shard dir must be tightened to 0700"
        );
        assert_eq!(
            mode(&bs.root.join("tmp")),
            DIR_MODE,
            "existing tmp dir must be tightened"
        );
    }

    #[test]
    #[cfg(unix)]
    fn blob_is_0600_and_dirs_are_0700() {
        let (_d, bs) = store();
        let sha = bs.put(b"perms").unwrap();
        let path = bs.path_for(&sha).unwrap();
        let mode = |p: &Path| fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), FILE_MODE, "blob file must be 0600");
        assert_eq!(
            mode(path.parent().unwrap()),
            DIR_MODE,
            "shard dir must be 0700"
        );
        assert_eq!(mode(&bs.root.join("tmp")), DIR_MODE, "tmp dir must be 0700");
    }
}
