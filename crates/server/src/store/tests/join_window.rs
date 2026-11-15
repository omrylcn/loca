//! The delivery window of a join request's key copy, at the store: the
//! deadline counted from approval across restarts, what an open clears, and
//! what an ACK or an expiry leaves in the database file.

use rusqlite::Connection;

use super::super::{BootstrapClaim, Store, JOIN_BOOTSTRAP_WINDOW_MS};
use super::{contains_bytes, files_containing, sql_values_containing};

/// An approval time shaped like a real one (milliseconds since the epoch),
/// so nothing here passes because the clock is near zero.
const APPROVED_AT: u64 = 1_800_000_000_000;

/// A key shaped like the server's (`mb_` and random letters) around a marker
/// nothing else in the file contains, so a byte search is meaningful.
fn marker_key(tag: &str) -> String {
    format!("mb_JOINKEYRESIDUE{tag}Q7ZK3W9X2M")
}

/// File one request named `name` and approve it at `at`, issuing `key`.
fn approve(store: &Store, id: &str, secret: &str, name: &str, key: &str, at: u64) {
    store
        .mint_admission_rights(&[format!("adm_{id}")], "pr_master", at, at + 1)
        .expect("mint");
    store
        .create_join_request(id, secret, name, "agent", at - 10)
        .expect("create");
    assert!(matches!(
        store.approve_join_request_atomic(id, key, "master", at),
        super::super::ApproveTxn::Committed(_)
    ));
}

fn expired_at(store: &Store, id: &str) -> Option<u64> {
    store
        .conn()
        .unwrap()
        .query_row(
            "SELECT bootstrap_expired_at FROM join_requests WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn the_deadline_counts_from_approval_and_a_restart_neither_resets_nor_skips_it() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("window.db");
    let p = path.to_str();
    {
        let store = Store::open_at(p, APPROVED_AT).unwrap();
        approve(&store, "jr_w", "sw", "windowed", "mb_windowed", APPROVED_AT);
    }
    // A restart one second before the deadline keeps the copy: the open
    // counts from the approval, it does not clear early.
    {
        let store = Store::open_at(p, APPROVED_AT + JOIN_BOOTSTRAP_WINDOW_MS - 1_000).unwrap();
        assert_eq!(
            store.join_request_mb_token_raw("jr_w"),
            Some(Some("mb_windowed".into()))
        );
    }
    // That open did not grant a fresh 24 hours either: the next open at the
    // deadline, a second later, clears the copy before anything is asked.
    let store = Store::open_at(p, APPROVED_AT + JOIN_BOOTSTRAP_WINDOW_MS).unwrap();
    assert_eq!(store.join_request_mb_token_raw("jr_w"), Some(None));
    assert_eq!(
        expired_at(&store, "jr_w"),
        Some(APPROVED_AT + JOIN_BOOTSTRAP_WINDOW_MS)
    );
    // Only the delivery copy ended: the membership is there and its key works.
    assert!(store.principal_for_credential("mb_windowed").is_some());
    let view = store
        .join_request_view("jr_w", "sw", APPROVED_AT + JOIN_BOOTSTRAP_WINDOW_MS)
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            view.status.as_str(),
            view.bootstrap_ready,
            view.bootstrap_expired
        ),
        ("approved", false, true)
    );
}

#[test]
fn an_approval_written_before_this_version_expires_from_its_own_decided_at() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("before.db");
    {
        let store = Store::open_at(path.to_str(), APPROVED_AT).unwrap();
        approve(&store, "jr_old", "so", "older", "mb_older", APPROVED_AT);
    }
    // The database as the previous version left it: no expiry column.
    Connection::open(&path)
        .unwrap()
        .execute_batch("ALTER TABLE join_requests DROP COLUMN bootstrap_expired_at")
        .unwrap();
    // The first open by this version adds the column and, the approval being
    // more than a window old, clears the copy at once instead of starting a
    // new window on upgrade.
    let store = Store::open_at(path.to_str(), APPROVED_AT + JOIN_BOOTSTRAP_WINDOW_MS + 1).unwrap();
    assert_eq!(store.join_request_mb_token_raw("jr_old"), Some(None));
    assert!(store.principal_for_credential("mb_older").is_some());
}

#[test]
fn an_approval_without_a_time_counts_as_past_its_deadline() {
    // Approval always writes `decided_at`; a row without one (damaged or
    // edited by hand) cannot place a deadline. It is taken as past: "never"
    // would leave the copy collectable for ever.
    for (tag, decided_at) in [("null", None), ("zero", Some(0_i64))] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(format!("{tag}.db"));
        {
            let store = Store::open_at(path.to_str(), APPROVED_AT).unwrap();
            approve(&store, "jr_t", "st", "timeless", "mb_timeless", APPROVED_AT);
            store
                .conn()
                .unwrap()
                .execute(
                    "UPDATE join_requests SET decided_at = ?1 WHERE id = 'jr_t'",
                    [decided_at],
                )
                .unwrap();
            // Asked at once, it is already gone, and says so.
            assert_eq!(
                store
                    .claim_join_request_bootstrap("jr_t", "st", APPROVED_AT)
                    .unwrap(),
                BootstrapClaim::Expired,
                "{tag}"
            );
            store
                .conn()
                .unwrap()
                .execute(
                    "UPDATE join_requests SET mb_token = 'mb_timeless', \
                     bootstrap_expired_at = NULL WHERE id = 'jr_t'",
                    [],
                )
                .unwrap();
        }
        // And an open clears it the same way.
        let store = Store::open_at(path.to_str(), APPROVED_AT).unwrap();
        assert_eq!(store.join_request_mb_token_raw("jr_t"), Some(None), "{tag}");
    }
}

/// Every page of the database file at `path` that holds `needle`, with the
/// b-tree that owns it (`dbstat`), or `None` for a page no b-tree owns: a
/// free page. Read through `conn`, the store's own connection, after the WAL
/// has been checkpointed into the file, so the file is what `dbstat` reads.
fn pages_holding(
    conn: &Connection,
    path: &std::path::Path,
    needle: &str,
) -> Vec<(u64, Option<String>)> {
    let page_size: u64 = conn
        .query_row("PRAGMA page_size", [], |r| r.get(0))
        .unwrap();
    let bytes = std::fs::read(path).unwrap();
    let mut pages: Vec<u64> = bytes
        .windows(needle.len())
        .enumerate()
        .filter(|(_, window)| *window == needle.as_bytes())
        .map(|(offset, _)| offset as u64 / page_size + 1)
        .collect();
    pages.dedup();
    pages
        .into_iter()
        .map(|page| {
            let owner: Option<String> = conn
                .query_row("SELECT name FROM dbstat WHERE pageno = ?1", [page], |r| {
                    r.get(0)
                })
                .ok();
            (page, owner)
        })
        .collect()
}

/// What an ACK and an expiry leave of the key in the database.
///
/// Member keys are stored as hashes: the join request's delivery copy is the
/// only place the approved member's key was ever written. Once the ACK or
/// the expiry clears it, the key is in no column, no page owned by any
/// b-tree, no free page and, with nothing else reading the file, nothing in
/// `-wal` or `-shm`.
#[test]
fn an_ack_or_an_expiry_leaves_no_copy_of_the_key_anywhere() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("residue.db");
    let store = Store::open_at(path.to_str(), APPROVED_AT).unwrap();
    // Neighbours, so the request rows share pages with others.
    for i in 0..20 {
        store
            .create_join_request(
                &format!("jr_n{i}"),
                "sn",
                &format!("near{i}"),
                "agent",
                APPROVED_AT,
            )
            .unwrap();
    }
    let acked = marker_key("ACKED");
    let lapsed = marker_key("LAPSED");
    approve(&store, "jr_a", "sa", "acker", &acked, APPROVED_AT);
    approve(&store, "jr_l", "sl", "lapser", &lapsed, APPROVED_AT);
    for (id, secret) in [("jr_a", "sa"), ("jr_l", "sl")] {
        for at in [APPROVED_AT + 1_000, APPROVED_AT + 2_000] {
            assert!(matches!(
                store.claim_join_request_bootstrap(id, secret, at).unwrap(),
                BootstrapClaim::Delivered(_)
            ));
        }
    }
    assert_eq!(
        store
            .ack_join_request_bootstrap("jr_a", "sa", APPROVED_AT + 3_000)
            .unwrap(),
        Some(true)
    );
    assert_eq!(
        store
            .expire_join_request_bootstraps(APPROVED_AT + JOIN_BOOTSTRAP_WINDOW_MS)
            .unwrap(),
        1
    );
    assert!(!store.join_key_checkpoint_owed().unwrap());

    for key in [&acked, &lapsed] {
        // The WAL was emptied by the owed checkpoint; `-shm` holds only the
        // WAL index. Neither has the key.
        let files = files_containing(&path, key);
        assert_eq!(files, Vec::<String>::new(), "{key}");
        let conn = store.conn().unwrap();
        // SQL: no column of any table holds it.
        assert_eq!(sql_values_containing(&conn, key), Vec::<String>::new());
        // File: no page holds it, live or free.
        assert_eq!(pages_holding(&conn, &path, key), Vec::new(), "{key}");
    }
    assert!(!contains_bytes(
        &std::fs::read(directory.path().join("residue.db-wal")).unwrap_or_default(),
        b"mb_JOINKEYRESIDUE"
    ));
}

/// A reader holding the file (a backup, `sqlite3` on the live database)
/// keeps the WAL from being emptied. The ACK, the sweep and the open must go
/// on, and the clean-up stay owed until it can be done.
#[test]
fn a_busy_database_fails_nothing_and_keeps_the_clean_up_owed() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("busy.db");
    let acked = marker_key("BUSYACK");
    let lapsed = marker_key("BUSYLAPSE");
    let store = Store::open_at(path.to_str(), APPROVED_AT).unwrap();
    approve(&store, "jr_a", "sa", "acker", &acked, APPROVED_AT);
    approve(&store, "jr_l", "sl", "lapser", &lapsed, APPROVED_AT);
    assert!(matches!(
        store
            .claim_join_request_bootstrap("jr_a", "sa", APPROVED_AT + 1)
            .unwrap(),
        BootstrapClaim::Delivered(_)
    ));

    let reader = Connection::open(&path).unwrap();
    reader
        .execute_batch("BEGIN; SELECT count(*) FROM join_requests;")
        .unwrap();

    // The ACK succeeds and the copy is gone from the row: that part is
    // guaranteed. The WAL still holds it, and the debt is recorded.
    assert_eq!(
        store
            .ack_join_request_bootstrap("jr_a", "sa", APPROVED_AT + 2)
            .unwrap(),
        Some(true)
    );
    assert_eq!(store.join_request_mb_token_raw("jr_a"), Some(None));
    assert!(store.join_key_checkpoint_owed().unwrap());
    assert!(files_containing(&path, &acked).contains(&"busy.db-wal".to_string()));

    // A late collection, refused and cleared, while busy.
    let late = APPROVED_AT + JOIN_BOOTSTRAP_WINDOW_MS;
    assert_eq!(
        store
            .claim_join_request_bootstrap("jr_l", "sl", late)
            .unwrap(),
        BootstrapClaim::Expired
    );
    assert_eq!(store.join_request_mb_token_raw("jr_l"), Some(None));
    assert!(store.join_key_checkpoint_owed().unwrap());

    // A restart while busy starts, and still owes it.
    drop(store);
    let store = Store::open_at(path.to_str(), late).expect("a busy WAL does not stop the start");
    assert!(store.join_key_checkpoint_owed().unwrap());

    // Once the reader is gone the sweep pays it: the record goes, and so do
    // the WAL's old copies. The member tables never held the key, so after
    // that it is in no file at all.
    reader.execute_batch("COMMIT").unwrap();
    drop(reader);
    assert_eq!(store.expire_join_request_bootstraps(late).unwrap(), 0);
    assert!(!store.join_key_checkpoint_owed().unwrap());
    for key in [&acked, &lapsed] {
        assert_eq!(files_containing(&path, key), Vec::<String>::new());
    }
}
