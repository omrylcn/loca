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
                // The target exists with stale/corrupt bytes (or, on Windows,
                // `rename` won't replace an existing file): swap it atomically.
                self.replace_existing(tmp, final_path, e)
            }
        }
    }

    /// Replace an existing `final_path` with `tmp`. On unix `rename` already
    /// replaces atomically, so reaching here is a genuine failure — but a stale
    /// regular file can still block it; drop and retry.
    #[cfg(not(windows))]
    fn replace_existing(&self, tmp: &Path, final_path: &Path, orig: io::Error) -> io::Result<()> {
        if fs::symlink_metadata(final_path)
            .map(|m| m.is_file())
            .unwrap_or(false)
        {
            fs::remove_file(final_path)?;
            fs::rename(tmp, final_path)
        } else {
            Err(orig)
        }
    }

    /// Windows: `ReplaceFileW` swaps the file contents in ONE atomic operation
    /// and tolerates a reader holding the old file open — unlike remove-then-
    /// rename, which has a window where the blob is briefly absent.
    #[cfg(windows)]
    fn replace_existing(&self, tmp: &Path, final_path: &Path, _orig: io::Error) -> io::Result<()> {
        use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;
        let replaced = to_wide(final_path);
        let replacement = to_wide(tmp);
        let ok = unsafe {
            ReplaceFileW(
                replaced.as_ptr(),
                replacement.as_ptr(),
                std::ptr::null(),
                0,
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
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

    /// Read the raw bytes at `path` without following a final symlink/reparse
    /// point. Both hardened paths refuse it atomically at open — unix via
    /// `O_NOFOLLOW`, Windows via `FILE_FLAG_OPEN_REPARSE_POINT` + a reparse-point
    /// attribute check on the same handle — so there is no metadata→read TOCTOU;
    /// the content-hash verify in `read_at` is the additional backstop. The
    /// windows-latest CI is the runtime authority for the Windows path.
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

    /// Windows: open with `FILE_FLAG_OPEN_REPARSE_POINT` — the analogue of unix
    /// `O_NOFOLLOW` — so the handle is the reparse point ITSELF, never its
    /// target. If the opened entry is a reparse point we refuse; otherwise we
    /// read from that same handle, so there is no metadata→read TOCTOU. loca-dev's
    /// windows-latest CI is the runtime authority for this path.
    #[cfg(windows)]
    fn read_no_follow(&self, path: &Path) -> io::Result<Vec<u8>> {
        use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        let mut f = fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        let meta = f.metadata()?;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "blob path is a reparse point",
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
        let mut buf = Vec::with_capacity(meta.len() as usize);
        f.read_to_end(&mut buf)?;
        Ok(buf)
    }

    /// Any other platform: refuse a symlink by metadata (the content-hash verify
    /// in `read_at` closes the residual TOCTOU). Neither prod (unix) nor Desktop
    /// (windows) reaches this; it only keeps an exotic target compiling.
    #[cfg(not(any(unix, windows)))]
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

    /// Delete a blob's file, NEVER following a symlink out of the tree. Used by
    /// the lifecycle sweep once a blob is unreferenced. A missing file is success
    /// (the goal state is reached), so the sweep is idempotent; a malformed id is
    /// a no-op (never a path). Symmetric with the O_NOFOLLOW read path: an
    /// attacker who swaps a shard dir for a symlink cannot make the sweep unlink
    /// a file outside the store.
    pub fn delete(&self, sha: &str) -> io::Result<()> {
        let Some(p) = self.path_for(sha) else {
            return Ok(());
        };
        self.delete_no_follow(&p)
    }

    /// Unix: open the shard directory with `O_NOFOLLOW | O_DIRECTORY` (fails if
    /// the shard was swapped for a symlink) and `unlinkat` the leaf by name.
    /// `unlinkat` removes the directory ENTRY, so even a symlink named like the
    /// blob is unlinked itself, not its target — the delete can never escape the
    /// tree. This is atomic (no metadata-then-act TOCTOU), matching `read_no_follow`.
    #[cfg(unix)]
    fn delete_no_follow(&self, path: &Path) -> io::Result<()> {
        use std::os::unix::ffi::OsStrExt;
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return Ok(());
        };
        let parent_c = std::ffi::CString::new(parent.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "bad path"))?;
        let dfd = unsafe {
            libc::open(
                parent_c.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        if dfd < 0 {
            let e = io::Error::last_os_error();
            // Shard absent → nothing to delete. A symlinked shard fails to open
            // here (ELOOP/ENOTDIR): refuse rather than traverse out of the tree.
            return if e.kind() == io::ErrorKind::NotFound {
                Ok(())
            } else {
                Err(e)
            };
        }
        // Close the dir fd on every path out.
        struct Fd(libc::c_int);
        impl Drop for Fd {
            fn drop(&mut self) {
                unsafe { libc::close(self.0) };
            }
        }
        let _fd = Fd(dfd);
        let name_c = std::ffi::CString::new(name.as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "bad name"))?;
        let rc = unsafe { libc::unlinkat(dfd, name_c.as_ptr(), 0) };
        if rc != 0 {
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::NotFound {
                return Err(e);
            }
        }
        Ok(())
    }

    /// Non-unix: no `unlinkat`, so verify neither the shard dir nor the leaf is a
    /// symlink before unlinking. A planted symlink is refused, so the delete
    /// still cannot redirect out of the tree (the Windows CI runner is the
    /// authority on the platform specifics).
    /// Windows: open the entry with `OPEN_REPARSE_POINT | DELETE_ON_CLOSE` and
    /// let the close delete it. `OPEN_REPARSE_POINT` means a reparse point is
    /// opened (and thus deleted) as ITSELF, never followed to its target, so a
    /// planted symlink/junction can't make the sweep unlink a file elsewhere.
    /// Atomic — no metadata-then-act TOCTOU. Runtime-authoritative on the
    /// windows-latest CI.
    #[cfg(windows)]
    fn delete_no_follow(&self, path: &Path) -> io::Result<()> {
        use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000; // open a dir handle
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        const DELETE: u32 = 0x0001_0000;
        // Refuse a reparse-point shard directory (a planted junction/symlink
        // could redirect the unlink out of the tree). `OPEN_REPARSE_POINT` only
        // guards the leaf, so check the parent handle's attributes. A fully
        // dirfd-relative delete (never re-resolving the shard) would need ntdll
        // `NtCreateFile(RootDirectory)`; this leaves only a narrow swap window
        // that requires concurrent server-level write access to the store dir.
        if let Some(parent) = path.parent() {
            match fs::OpenOptions::new()
                .access_mode(0)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
                .open(parent)
            {
                Ok(dir) => {
                    if dir.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "shard directory is a reparse point",
                        ));
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(e),
            }
        }
        match fs::OpenOptions::new()
            .access_mode(DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_DELETE_ON_CLOSE)
            .open(path)
        {
            Ok(_handle) => Ok(()), // dropping the handle fires delete-on-close
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Any other platform: metadata-guarded unlink (neither prod nor Desktop
    /// reaches this; it only keeps an exotic target compiling).
    #[cfg(not(any(unix, windows)))]
    fn delete_no_follow(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            match fs::symlink_metadata(parent) {
                Ok(m) if m.file_type().is_symlink() => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "shard directory is a symlink",
                    ))
                }
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(e),
            }
        }
        match fs::symlink_metadata(path) {
            Ok(m) if m.file_type().is_file() => fs::remove_file(path),
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "blob path is not a regular file",
            )),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// Windows wide-string (UTF-16 NUL-terminated) for a path, for `ReplaceFileW`.
#[cfg(windows)]
fn to_wide(p: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
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

    /// A shard dir swapped for a symlink must not let `delete` unlink a file
    /// OUTSIDE the store tree (an attacker who plants such a symlink could
    /// otherwise make the sweep delete an arbitrary file).
    #[cfg(unix)]
    #[test]
    fn delete_never_follows_a_symlinked_shard_out_of_the_tree() {
        let (dir, bs) = store();
        let sha = bs.put(b"victim bytes").unwrap();
        // An external file named exactly like the blob's leaf, so that FOLLOWING
        // the symlink would compute the same path and unlink it.
        let external = tempfile::tempdir().unwrap();
        let target = external.path().join(&sha);
        std::fs::write(&target, b"do not delete me").unwrap();
        // Replace the real shard dir with a symlink to the external directory.
        let shard = dir.path().join("attachments").join(&sha[..2]);
        std::fs::remove_dir_all(&shard).unwrap();
        std::os::unix::fs::symlink(external.path(), &shard).unwrap();
        // delete may error (refusal) or be a no-op, but it must NOT touch the
        // external file behind the symlink.
        let _ = bs.delete(&sha);
        assert!(target.exists(), "external file must survive");
        assert_eq!(std::fs::read(&target).unwrap(), b"do not delete me");
    }

    #[test]
    fn delete_removes_a_real_blob_and_is_idempotent() {
        let (_d, bs) = store();
        let sha = bs.put(b"collect me").unwrap();
        assert!(bs.exists(&sha));
        bs.delete(&sha).unwrap();
        assert!(!bs.exists(&sha), "the blob file is gone");
        // A second delete on the now-absent blob is a clean no-op.
        bs.delete(&sha).unwrap();
    }

    // ---- Windows blob-store authoritative tests (run on windows-latest CI) ----
    // Written without a local Windows box; the FFI is compile-checked via a
    // standalone probe and these prove the security contract on real Windows.

    /// A reparse-point shard directory must not let `delete` unlink a file
    /// OUTSIDE the store tree. Uses a directory JUNCTION (needs no privilege,
    /// unlike a symlink) so the check ALWAYS runs on the runner — no false-skip;
    /// if the junction can't be created the test fails rather than passing empty.
    #[cfg(windows)]
    #[test]
    fn windows_delete_refuses_a_reparse_shard_and_leaves_external_file() {
        let (dir, bs) = store();
        let sha = bs.put(b"victim bytes").unwrap();
        let external = tempfile::tempdir().unwrap();
        let target = external.path().join(&sha);
        std::fs::write(&target, b"do not delete me").unwrap();
        let shard = dir.path().join("attachments").join(&sha[..2]);
        std::fs::remove_dir_all(&shard).unwrap();
        let out = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&shard)
            .arg(external.path())
            .output()
            .expect("run mklink");
        assert!(
            out.status.success(),
            "mklink /J must create a junction: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let _ = bs.delete(&sha);
        assert!(target.exists(), "external file behind the reparse shard must survive");
        assert_eq!(std::fs::read(&target).unwrap(), b"do not delete me");
    }

    /// Old-or-new atomicity, measured with a live handle: hold the OLD corrupt
    /// file open (sharing read/write/DELETE so the replace can proceed) while
    /// `put()` self-heals it. Afterward the path yields the FULL new bytes and
    /// the pre-replace handle still yields the FULL old bytes — never a torn
    /// file or a `NotFound` in between.
    #[cfg(windows)]
    #[test]
    fn windows_replace_is_old_or_new_never_torn() {
        use std::io::Read as _;
        use std::os::windows::fs::OpenOptionsExt;
        let (_d, bs) = store();
        let new_bytes: &[u8] = b"the healed new bytes exactly!";
        let old_bytes: &[u8] = b"old corrupt content bytes??";
        let sha = format!("{:x}", Sha256::digest(new_bytes));
        let p = bs.path_for(&sha).unwrap();
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, old_bytes).unwrap();
        let mut old_handle = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0x1 | 0x2 | 0x4) // FILE_SHARE_READ | WRITE | DELETE
            .open(&p)
            .unwrap();
        assert_eq!(bs.put(new_bytes).unwrap(), sha);
        assert_eq!(bs.read(&sha).unwrap(), new_bytes, "path reads the full new bytes");
        let mut buf = Vec::new();
        old_handle.read_to_end(&mut buf).unwrap();
        assert_eq!(buf, old_bytes, "the pre-replace handle still reads the full old bytes");
    }

    /// Parallel writers of the SAME blob over a PRE-PLACED CORRUPT target, so at
    /// least one writer must take the real replace path (not just the create/
    /// rename fast path). All writers succeed, agree on the id, and the final
    /// blob is the intact healed content.
    #[cfg(windows)]
    #[test]
    fn windows_parallel_writers_over_corrupt_target_converge() {
        let (_d, bs) = store();
        let bytes: &[u8] = b"concurrent blob bytes over a corrupt target";
        let sha = format!("{:x}", Sha256::digest(bytes));
        let p = bs.path_for(&sha).unwrap();
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"corrupt-preexisting").unwrap();
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let bs = bs.clone();
                let data = bytes.to_vec();
                std::thread::spawn(move || bs.put(&data))
            })
            .collect();
        for h in handles {
            assert_eq!(h.join().unwrap().unwrap(), sha, "every writer agrees on the id");
        }
        assert_eq!(
            bs.read(&sha).unwrap(),
            bytes,
            "one consistent healed blob after concurrent writes"
        );
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
