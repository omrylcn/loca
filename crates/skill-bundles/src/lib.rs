//! The embedded loca / loca-care skill distribution bundles (see build.rs),
//! shared by the server download endpoint (Web) and the Desktop local Skill
//! Library. Both surfaces embed the SAME deterministic archives, so the bytes
//! the endpoint serves and the files the Desktop extracts are identical — the
//! per-file and archive SHA-256 in each manifest verify from one build artifact.

include!(concat!(env!("OUT_DIR"), "/skill_bundles_data.rs"));

use std::io::Read;
use std::path::{Path, PathBuf};

/// The skill-library version (the workspace release). Used to name the versioned
/// extraction directory so an update lands beside the old version, not over it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Look up an embedded bundle by skill name (`loca`, `loca-care`).
pub fn bundle(name: &str) -> Option<&'static Bundle> {
    BUNDLES.iter().find(|b| b.name == name)
}

/// Every embedded bundle.
pub fn all() -> &'static [Bundle] {
    BUNDLES
}

fn io_err(msg: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, msg)
}
fn to_io<E: std::fmt::Display>(e: E) -> std::io::Error {
    io_err(&e.to_string())
}

/// Extract EVERY bundle under `dest` (each as `dest/<name>/...`) and then make
/// the whole tree read-only. Traversal-safe: an archive entry whose path is
/// absolute, contains `..`, or otherwise resolves outside `dest` aborts the
/// entire extraction — nothing is ever written outside `dest`.
pub fn extract_all(dest: &Path) -> std::io::Result<()> {
    for b in BUNDLES {
        extract_one(b, dest)?;
    }
    make_readonly_recursive(dest)?;
    Ok(())
}

fn extract_one(b: &Bundle, dest: &Path) -> std::io::Result<()> {
    extract_zip(b.zip, dest)
}

fn extract_zip(zip_bytes: &[u8], dest: &Path) -> std::io::Result<()> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).map_err(to_io)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(to_io)?;
        // `enclosed_name` returns None for an unsafe path (absolute or `..`).
        let rel = entry
            .enclosed_name()
            .ok_or_else(|| io_err("unsafe path in skill bundle"))?;
        let out = dest.join(&rel);
        // Defense in depth: the resolved path must stay under dest.
        if !out.starts_with(dest) {
            return Err(io_err("skill bundle entry escapes the destination"));
        }
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut buf = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut buf)?;
        std::fs::write(&out, &buf)?;
    }
    Ok(())
}

/// Install the Skill Library under `root` at the current [`VERSION`] and return
/// that version's directory. This is what the Desktop calls on startup/update:
///
/// - Concurrency-safe: an install lock serialises callers and staging/temp names
///   are unique, so two Desktop instances can never corrupt each other.
/// - Integrity-checked: an existing install is ACCEPTED only if every file still
///   matches the embedded manifest SHA-256. A missing, partial, or tampered tree
///   is re-extracted automatically (self-healing) — a stale marker alone is never
///   trusted.
/// - A fresh install is staged under a unique sibling directory, made read-only,
///   then atomically `rename`d into `root/<VERSION>`.
/// - The `current` pointer is updated atomically. Older versions are LEFT IN
///   PLACE for rollback.
/// - Works with no server and no network: the bytes are embedded in the binary.
pub fn install_versioned(root: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(root)?;
    let _lock = InstallLock::acquire(root)?;
    let version_dir = root.join(VERSION);

    // Accept an existing install ONLY if it verifies against the manifests.
    let intact = version_dir.is_dir() && verify(&version_dir).unwrap_or(false);
    if !intact {
        remove_tree(&version_dir)?; // clear a missing / partial / tampered tree
        let staging = root.join(format!(".staging-{}", unique_suffix()));
        remove_tree(&staging)?;
        std::fs::create_dir_all(&staging)?;
        extract_all(&staging)?; // extracts, then makes the whole tree read-only
        remove_tree(&version_dir)?; // defensive: nothing should have appeared
        std::fs::rename(&staging, &version_dir)?;
    }
    set_current(root, VERSION)?;
    Ok(version_dir)
}

/// Verify that `dir` is EXACTLY the embedded library: the file set matches the
/// manifests one-to-one (no extra, no missing), every file's SHA-256 matches,
/// there are NO symlinks or special files, and every file AND directory is
/// read-only. Any deviation returns false so the caller re-extracts (self-heal).
pub fn verify(dir: &Path) -> std::io::Result<bool> {
    // The version ROOT itself must exist, be a real directory (not a symlink),
    // and be read-only — otherwise a writable root passes even with intact files.
    match std::fs::symlink_metadata(dir) {
        Ok(m) if m.is_dir() && !m.file_type().is_symlink() && m.permissions().readonly() => {}
        _ => return Ok(false),
    }
    let (mut files, mut dirs) = expected_entries()?;
    if !check_tree(dir, dir, &mut files, &mut dirs)? {
        return Ok(false);
    }
    // Exact tree: nothing the manifests list is missing, and no extra directory.
    Ok(files.is_empty() && dirs.is_empty())
}

type FileMap = std::collections::HashMap<PathBuf, String>;
type DirSet = std::collections::HashSet<PathBuf>;

fn expected_entries() -> std::io::Result<(FileMap, DirSet)> {
    let mut files = FileMap::new();
    let mut dirs = DirSet::new();
    for b in BUNDLES {
        let manifest: serde_json::Value = serde_json::from_str(b.manifest).map_err(to_io)?;
        for file in manifest["files"]
            .as_array()
            .ok_or_else(|| io_err("bad manifest: files"))?
        {
            let path = PathBuf::from(
                file["path"]
                    .as_str()
                    .ok_or_else(|| io_err("bad manifest: path"))?,
            );
            let sha = file["sha256"]
                .as_str()
                .ok_or_else(|| io_err("bad manifest: sha256"))?;
            // Every directory ancestor of the file is an expected directory.
            let mut ancestor = path.parent();
            while let Some(d) = ancestor {
                if d.as_os_str().is_empty() {
                    break;
                }
                dirs.insert(d.to_path_buf());
                ancestor = d.parent();
            }
            files.insert(path, sha.to_string());
        }
    }
    Ok((files, dirs))
}

fn check_tree(
    root: &Path,
    dir: &Path,
    files: &mut FileMap,
    dirs: &mut DirSet,
) -> std::io::Result<bool> {
    use sha2::{Digest, Sha256};
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let meta = std::fs::symlink_metadata(&path)?; // does NOT follow symlinks
                                                      // No symlinks and no special files — only regular files and directories.
        if meta.file_type().is_symlink() {
            return Ok(false);
        }
        // Every entry (file OR directory) must be read-only.
        if !meta.permissions().readonly() {
            return Ok(false);
        }
        let rel = path.strip_prefix(root).map_err(to_io)?.to_path_buf();
        if meta.is_dir() {
            if !dirs.remove(&rel) {
                return Ok(false); // an unexpected (e.g. empty) directory
            }
            if !check_tree(root, &path, files, dirs)? {
                return Ok(false);
            }
        } else if meta.is_file() {
            match files.remove(&rel) {
                Some(want) => {
                    let bytes = std::fs::read(&path)?;
                    if format!("{:x}", Sha256::digest(&bytes)) != want {
                        return Ok(false);
                    }
                }
                None => return Ok(false), // an unexpected extra file
            }
        } else {
            return Ok(false); // special file (fifo, device, socket, …)
        }
    }
    Ok(true)
}

fn set_current(root: &Path, version: &str) -> std::io::Result<()> {
    // Unique temp name so two installers never collide on one temp file.
    let tmp = root.join(format!(".current.{}.tmp", unique_suffix()));
    std::fs::write(&tmp, version)?;
    std::fs::rename(&tmp, root.join("current"))
}

/// The version directory `current` points at, if it exists.
pub fn current_dir(root: &Path) -> Option<PathBuf> {
    let version = std::fs::read_to_string(root.join("current")).ok()?;
    let dir = root.join(version.trim());
    dir.is_dir().then_some(dir)
}

fn unique_suffix() -> String {
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

/// A cross-process install lock backed by an OS advisory file lock. The lock is
/// tied to an open file handle, so the OS releases it automatically when the
/// holder exits or crashes — a live (even slow) owner can NEVER be stolen, and
/// there is no lock directory another installer could delete out from under the
/// real owner. Released on drop (and by the OS on process death).
struct InstallLock(std::fs::File);
impl InstallLock {
    fn acquire(root: &Path) -> std::io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(root.join(".install.lock"))?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(InstallLock(file)),
                Err(std::fs::TryLockError::WouldBlock) => {
                    if std::time::Instant::now() >= deadline {
                        return Err(io_err(
                            "timed out waiting for the skill-library install lock",
                        ));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(std::fs::TryLockError::Error(e)) => return Err(e),
            }
        }
    }
}
impl Drop for InstallLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

fn remove_tree(path: &Path) -> std::io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    // The version tree is read-only; restore write before removing.
    let _ = make_writable_recursive(path);
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn make_readonly_recursive(path: &Path) -> std::io::Result<()> {
    // Post-order: children first, then the directory itself. The whole version
    // tree — DIRECTORIES included — becomes read-only, so a file cannot be
    // deleted or replaced (on Unix that needs write on the parent directory).
    // Only the library root, staging, and the `current` pointer stay writable,
    // and version replacement/rollback goes through `remove_tree` (which
    // restores write first) — the installed contents are genuinely tamper-proof.
    if path.is_dir() {
        for entry in std::fs::read_dir(path)? {
            make_readonly_recursive(&entry?.path())?;
        }
    }
    set_readonly(path, true)
}

fn set_readonly(path: &Path, readonly: bool) -> std::io::Result<()> {
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_readonly(readonly);
    std::fs::set_permissions(path, perms)
}

fn make_writable_recursive(path: &Path) -> std::io::Result<()> {
    // Use symlink_metadata so we NEVER follow a symlink: chmod-ing a symlink's
    // target could touch files OUTSIDE the library tree. A symlink is left as-is
    // for removal — `remove_dir_all` unlinks it without entering its target.
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    let _ = set_readonly(path, false);
    if meta.is_dir() {
        for entry in std::fs::read_dir(path)? {
            make_writable_recursive(&entry?.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn sha(bytes: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(bytes);
        format!("{:x}", h.finalize())
    }

    #[test]
    fn extract_matches_the_manifest_and_is_read_only() {
        let dir = tempfile::tempdir().unwrap();
        extract_all(dir.path()).unwrap();
        for b in all() {
            let manifest: serde_json::Value = serde_json::from_str(b.manifest).unwrap();
            // The embedded archive is exactly what the manifest (and the Web
            // endpoint) claims — one build artifact on both surfaces.
            assert_eq!(manifest["bundle_sha256"].as_str().unwrap(), sha(b.zip));
            for file in manifest["files"].as_array().unwrap() {
                let path = file["path"].as_str().unwrap();
                let want = file["sha256"].as_str().unwrap();
                let full = dir.path().join(path);
                let bytes = std::fs::read(&full).unwrap_or_else(|_| panic!("missing {path}"));
                assert_eq!(
                    sha(&bytes),
                    want,
                    "extracted {path} must match the manifest"
                );
                assert!(
                    std::fs::metadata(&full).unwrap().permissions().readonly(),
                    "{path} must be read-only"
                );
            }
        }
    }

    #[test]
    fn a_path_traversal_entry_writes_nothing_outside_dest() {
        // Craft a malicious archive whose entry tries to escape the destination.
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            w.start_file("../escape.txt", zip::write::SimpleFileOptions::default())
                .unwrap();
            use std::io::Write;
            w.write_all(b"pwned").unwrap();
            w.finish().unwrap();
        }
        let parent = tempfile::tempdir().unwrap();
        let dest = parent.path().join("dest");
        std::fs::create_dir(&dest).unwrap();
        // Whether the archive is rejected or its name is sanitised, NOTHING is
        // ever written outside the destination.
        let _ = extract_zip(&buf, &dest);
        assert!(!parent.path().join("escape.txt").exists());
    }

    // The read-only version tree cannot be removed by TempDir's cleanup; restore
    // write so the temp directory is not leaked after the test.
    fn cleanup(root: &Path) {
        let _ = make_writable_recursive(root);
    }

    #[test]
    fn install_versioned_is_idempotent_and_points_current() {
        let root = tempfile::tempdir().unwrap();
        let dir1 = install_versioned(root.path()).unwrap();
        assert!(dir1.ends_with(VERSION));
        assert!(dir1.join("loca/connect.sh").exists());
        assert!(dir1.join("loca-care/SKILL.md").exists());
        assert_eq!(current_dir(root.path()).unwrap(), dir1);
        // A second call verifies the intact install and is a cheap no-op.
        let dir2 = install_versioned(root.path()).unwrap();
        assert_eq!(dir1, dir2);
        assert!(verify(&dir1).unwrap());
        cleanup(root.path());
    }

    #[test]
    fn installed_files_and_dirs_are_truly_read_only() {
        let root = tempfile::tempdir().unwrap();
        let dir = install_versioned(root.path()).unwrap();
        let file = dir.join("loca/connect.sh");
        assert!(std::fs::metadata(&file).unwrap().permissions().readonly());
        assert!(
            std::fs::metadata(dir.join("loca"))
                .unwrap()
                .permissions()
                .readonly(),
            "directories must be read-only too"
        );
        // A read-only parent directory means the file cannot be deleted OR
        // replaced — the installed skill is genuinely tamper-proof.
        assert!(
            std::fs::remove_file(&file).is_err(),
            "must not be deletable"
        );
        assert!(
            std::fs::write(&file, b"tampered").is_err(),
            "must not be writable"
        );
        cleanup(root.path());
    }

    #[test]
    fn a_corrupt_install_is_detected_and_self_heals() {
        let root = tempfile::tempdir().unwrap();
        let dir = install_versioned(root.path()).unwrap();
        let file = dir.join("loca/SKILL.md");
        let original = std::fs::read(&file).unwrap();
        // Tamper with the tree.
        make_writable_recursive(&dir).unwrap();
        std::fs::write(&file, b"corrupted").unwrap();
        assert!(!verify(&dir).unwrap(), "verify must catch the corruption");
        // A re-install re-extracts and restores the manifest content.
        install_versioned(root.path()).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), original);
        assert!(verify(&dir).unwrap());
        cleanup(root.path());
    }

    #[test]
    fn concurrent_installs_agree_and_do_not_corrupt() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().to_path_buf();
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let p = path.clone();
                std::thread::spawn(move || install_versioned(&p).unwrap())
            })
            .collect();
        let dirs: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        for d in &dirs {
            assert_eq!(d, &dirs[0], "all installers must agree on one version dir");
        }
        assert!(verify(&dirs[0]).unwrap(), "the install must be intact");
        cleanup(root.path());
    }

    #[test]
    fn verify_rejects_extra_symlink_and_writable_and_self_heals() {
        let root = tempfile::tempdir().unwrap();
        let dir = install_versioned(root.path()).unwrap();
        assert!(verify(&dir).unwrap());

        // (a) An EXTRA file the manifest does not list.
        make_writable_recursive(&dir).unwrap();
        std::fs::write(dir.join("loca/EXTRA.txt"), b"x").unwrap();
        assert!(!verify(&dir).unwrap(), "an extra file must fail verify");
        install_versioned(root.path()).unwrap();
        assert!(
            !dir.join("loca/EXTRA.txt").exists(),
            "self-heal removes the extra file"
        );
        assert!(verify(&dir).unwrap());

        // (b) A SYMLINK inside the tree.
        #[cfg(unix)]
        {
            make_writable_recursive(&dir).unwrap();
            std::os::unix::fs::symlink("/etc/passwd", dir.join("loca/link")).unwrap();
            assert!(!verify(&dir).unwrap(), "a symlink must fail verify");
            install_versioned(root.path()).unwrap();
            assert!(
                !dir.join("loca/link").exists(),
                "self-heal removes the symlink"
            );
            assert!(verify(&dir).unwrap());
        }

        // (c) A PERMISSION-only tamper: a file turned writable (content intact).
        set_readonly(&dir.join("loca/SKILL.md"), false).unwrap();
        assert!(!verify(&dir).unwrap(), "a writable file must fail verify");
        install_versioned(root.path()).unwrap();
        assert!(std::fs::metadata(dir.join("loca/SKILL.md"))
            .unwrap()
            .permissions()
            .readonly());
        assert!(verify(&dir).unwrap());

        cleanup(root.path());
    }

    #[test]
    fn verify_rejects_a_writable_root_and_self_heals() {
        // A writable version ROOT is a tamper vector even when every file inside
        // is intact and read-only: a writable parent lets an attacker delete or
        // swap the whole directory. verify() must reject it, and re-install must
        // restore the read-only root.
        let root = tempfile::tempdir().unwrap();
        let dir = install_versioned(root.path()).unwrap();
        assert!(verify(&dir).unwrap());
        set_readonly(&dir, false).unwrap();
        assert!(
            !verify(&dir).unwrap(),
            "a writable version root must fail verify"
        );
        install_versioned(root.path()).unwrap();
        assert!(
            std::fs::metadata(&dir).unwrap().permissions().readonly(),
            "self-heal restores the read-only root"
        );
        assert!(verify(&dir).unwrap());
        cleanup(root.path());
    }

    #[test]
    fn verify_rejects_an_extra_empty_directory_and_self_heals() {
        // An EXTRA directory the manifest does not list — even an empty one, which
        // a hash-only check would miss — must fail verify and be removed on heal.
        // Insert it while keeping EVERYTHING (root, `loca`, the new dir) read-only,
        // so verify() fails ONLY because of the extra directory, not a writable
        // parent — otherwise the test would pass for the wrong reason.
        let root = tempfile::tempdir().unwrap();
        let dir = install_versioned(root.path()).unwrap();
        let parent = dir.join("loca");
        set_readonly(&parent, false).unwrap(); // open just this dir to mkdir
        let extra = parent.join("EXTRA_DIR");
        std::fs::create_dir(&extra).unwrap();
        set_readonly(&extra, true).unwrap();
        set_readonly(&parent, true).unwrap(); // re-lock: only EXTRA_DIR is anomalous
        assert!(verify(&dir).is_ok());
        assert!(
            !verify(&dir).unwrap(),
            "an extra empty directory must fail verify"
        );
        install_versioned(root.path()).unwrap();
        assert!(
            !dir.join("loca/EXTRA_DIR").exists(),
            "self-heal removes the extra directory"
        );
        assert!(verify(&dir).unwrap());
        cleanup(root.path());
    }

    #[test]
    #[cfg(unix)]
    fn self_heal_cleanup_never_touches_a_symlink_target_outside_the_tree() {
        // The self-heal path makes the tree writable before removing it. It must
        // NOT follow a symlink and chmod files OUTSIDE the library — a tampering
        // symlink pointing at an external directory must leave that directory's
        // permissions and contents completely untouched.
        let root = tempfile::tempdir().unwrap();
        let dir = install_versioned(root.path()).unwrap();

        // An external directory with a read-only file, outside the library tree.
        let external = tempfile::tempdir().unwrap();
        let ext_file = external.path().join("secret");
        std::fs::write(&ext_file, b"external").unwrap();
        set_readonly(&ext_file, true).unwrap();
        let ext_dir_perm = std::fs::metadata(external.path()).unwrap().permissions();
        let ext_file_perm = std::fs::metadata(&ext_file).unwrap().permissions();

        // Tamper: a symlink inside the library pointing at the external directory.
        make_writable_recursive(&dir).unwrap();
        std::os::unix::fs::symlink(external.path(), dir.join("loca/evil")).unwrap();

        // Self-heal (verify false -> remove_tree makes-writable + removes).
        install_versioned(root.path()).unwrap();

        // The external directory and its file are byte- and permission-identical,
        // and the symlink is gone from the healed library.
        assert_eq!(
            std::fs::metadata(external.path()).unwrap().permissions(),
            ext_dir_perm
        );
        assert_eq!(
            std::fs::metadata(&ext_file).unwrap().permissions(),
            ext_file_perm
        );
        assert_eq!(std::fs::read(&ext_file).unwrap(), b"external");
        assert!(
            !dir.join("loca/evil").exists(),
            "the tampering symlink is removed"
        );
        assert!(verify(&dir).unwrap());
        cleanup(root.path());
    }

    #[test]
    fn a_held_lock_is_never_stolen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".install.lock");
        let a = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .unwrap();
        a.try_lock().unwrap(); // A holds the lock (a live owner)
                               // A second, independent handle CANNOT acquire it while A holds it —
                               // regardless of age. A live owner is never stolen.
        let b = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .unwrap();
        assert!(matches!(
            b.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        // Once A releases (or would die), B can take it.
        a.unlock().unwrap();
        assert!(b.try_lock().is_ok());
        b.unlock().unwrap();
    }
}
