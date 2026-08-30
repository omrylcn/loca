//! Test helper: acquire the OS advisory install lock on the file given as the
//! first argument and hold it until the process is killed, printing a "held"
//! marker so the parent test knows the lock is taken. Used by the cross-process
//! lock test to prove a live owner cannot be stolen and that the OS releases the
//! lock when the holder dies.

use std::io::Write;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: lock_hold <lock-file>");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .expect("open lock file");
    file.lock().expect("acquire lock"); // the same OS advisory lock the crate uses
    println!("held");
    std::io::stdout().flush().expect("flush");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
