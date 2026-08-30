//! Real cross-process install-lock semantics: a separate process holds the lock,
//! this process cannot steal it while that process is alive, and once the holder
//! is killed the OS releases the lock (the crash-release gate).

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

#[test]
fn a_lock_held_by_another_process_cannot_be_stolen_and_frees_on_its_death() {
    let dir = tempfile::tempdir().unwrap();
    let lock = dir.path().join(".install.lock");

    // Child process takes the lock and reports "held". stdin/stderr are nulled so
    // the child never keeps a copy of cargo's captured pipes — if an assertion
    // below fails before we kill the child, the leaked child can't wedge the suite.
    let mut child = Command::new(env!("CARGO_BIN_EXE_lock_hold"))
        .arg(&lock)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    assert_eq!(line.trim(), "held", "child must report holding the lock");

    // While the child (a live, independent process) holds it, THIS process must
    // NOT be able to acquire it — a live owner is never stolen.
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock)
        .unwrap();
    assert!(
        matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock)),
        "a lock held by another live process must not be acquirable"
    );

    // Kill the holder — the OS must release the lock (crash-release).
    child.kill().unwrap();
    child.wait().unwrap();

    // Now the lock is free; acquire it (retry briefly while the OS reclaims it).
    let mut acquired = false;
    for _ in 0..100 {
        if file.try_lock().is_ok() {
            acquired = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        acquired,
        "the lock must be free once the holding process dies"
    );
    file.unlock().unwrap();
}
