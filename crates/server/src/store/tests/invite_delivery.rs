//! An invite's delivery copy at the store (`invites.delivery_token`): kept
//! from issue until the invite's first use or 24 hours after issue, cleared
//! at use, by the sweep and at open, with `secure_delete` and the owed WAL
//! clean-up. Also: the invite and membership loads report a storage error
//! instead of answering "none".

use rusqlite::Connection;

use super::super::{secret_hash, InviteRecord, Store, INVITE_DELIVERY_WINDOW_MS};
use super::{contains_bytes, files_containing, sql_values_containing};

/// An issue time shaped like a real one (milliseconds since the epoch).
const ISSUED_AT: u64 = 1_800_000_000_000;

/// An invite shaped like the server's around a marker nothing else in the file
/// contains, so a byte search is meaningful.
fn marker_invite(tag: &str) -> String {
    format!("dv_INVITEDELIVERY{tag}Q7ZK3W9X2M")
}

/// The membership the fixture's invites seat, by its id.
const MEMBER_ID: &str = "mbid_deliverymember";

fn invite(token: &str, room: &str, at: u64) -> InviteRecord {
    InviteRecord::issued(token, room, MEMBER_ID, "delivered", "agent", at, "master")
}

/// Mint `token` for delivery, as the hub does.
fn issue(store: &Store, token: &str, room: &str, at: u64) {
    store.issue_invite(&invite(token, room, at), token).unwrap();
}

/// The copy, and when the invite was first used.
fn copy(store: &Store, token: &str) -> (Option<String>, Option<u64>) {
    store
        .invite_delivery_raw_by_hash(&secret_hash(token))
        .expect("row")
}

fn end_delivery(store: &Store, token: &str, at: u64) -> bool {
    store
        .end_invite_delivery_by_hash(&secret_hash(token), at)
        .unwrap()
}

#[test]
fn a_minted_invite_keeps_a_copy() {
    let store = Store::open_at(None, ISSUED_AT).unwrap();
    // A memory-only store has nothing to keep.
    issue(&store, "dv_memory", "parlor", ISSUED_AT);
    assert!(store
        .invite_delivery_raw_by_hash(&secret_hash("dv_memory"))
        .is_none());

    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_at(directory.path().join("mint.db").to_str(), ISSUED_AT).unwrap();
    issue(&store, "dv_issued", "parlor", ISSUED_AT);
    assert_eq!(copy(&store, "dv_issued"), (Some("dv_issued".into()), None));
    assert_eq!(store.load_invite_deliveries().unwrap(), vec!["dv_issued"]);
}

/// The start binds an old invite to its membership. That write is not a
/// first use and not a deadline, so it must leave the delivery state as it
/// found it.
#[test]
fn binding_an_invite_to_its_membership_leaves_its_delivery_state_alone() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_at(directory.path().join("rebind.db").to_str(), ISSUED_AT).unwrap();

    // Owed: the copy is still held after the binding.
    let owed = InviteRecord::issued(
        "dv_owed",
        "parlor",
        "",
        "delivered",
        "agent",
        ISSUED_AT,
        "master",
    );
    store.issue_invite(&owed, "dv_owed").unwrap();
    store
        .bind_invite_member_by_hash(&owed.secret_hash, MEMBER_ID)
        .unwrap();
    assert_eq!(
        copy(&store, "dv_owed"),
        (Some("dv_owed".into()), None),
        "the binding ended a delivery nobody used"
    );
    assert_eq!(store.load_invite_deliveries().unwrap(), vec!["dv_owed"]);
    assert_eq!(
        store.load_invites().unwrap()[0].member_id,
        MEMBER_ID,
        "the binding itself is written"
    );

    // Used: the first use stays recorded and no copy comes back.
    let used = InviteRecord::issued(
        "dv_used",
        "parlor",
        "",
        "delivered",
        "agent",
        ISSUED_AT,
        "master",
    );
    store.issue_invite(&used, "dv_used").unwrap();
    assert!(end_delivery(&store, "dv_used", ISSUED_AT + 5_000));
    store
        .bind_invite_member_by_hash(&used.secret_hash, MEMBER_ID)
        .unwrap();
    assert_eq!(copy(&store, "dv_used"), (None, Some(ISSUED_AT + 5_000)));
}

#[test]
fn the_deadline_counts_from_issue_and_a_restart_neither_resets_nor_skips_it() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("window.db");
    let p = path.to_str();
    {
        let store = Store::open_at(p, ISSUED_AT).unwrap();
        issue(&store, "dv_windowed", "parlor", ISSUED_AT);
    }
    // A restart one millisecond before the deadline keeps the copy.
    {
        let store = Store::open_at(p, ISSUED_AT + INVITE_DELIVERY_WINDOW_MS - 1).unwrap();
        assert_eq!(
            copy(&store, "dv_windowed").0.as_deref(),
            Some("dv_windowed")
        );
        // The sweep at the same moment keeps it too.
        assert_eq!(
            store
                .expire_invite_deliveries(ISSUED_AT + INVITE_DELIVERY_WINDOW_MS - 1, None)
                .unwrap(),
            0
        );
    }
    // That open granted no fresh 24 hours: the next open, at the deadline
    // counted from issue, clears the copy before anything is asked.
    let store = Store::open_at(p, ISSUED_AT + INVITE_DELIVERY_WINDOW_MS).unwrap();
    assert_eq!(copy(&store, "dv_windowed"), (None, None));
    assert!(store.load_invite_deliveries().unwrap().is_empty());
    // Only the delivery ended: the invite is live.
    assert_eq!(store.load_invites().unwrap().len(), 1);
}

#[test]
fn the_sweep_and_a_late_attempt_clear_at_the_deadline_and_not_before() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_at(directory.path().join("sweep.db").to_str(), ISSUED_AT).unwrap();
    issue(&store, "dv_swept", "a", ISSUED_AT);
    issue(&store, "dv_late", "b", ISSUED_AT);
    issue(&store, "dv_young", "c", ISSUED_AT + 1);
    let deadline = ISSUED_AT + INVITE_DELIVERY_WINDOW_MS;
    // A late delivery attempt clears only its own copy.
    assert_eq!(
        store
            .expire_invite_deliveries(deadline - 1, Some(&secret_hash("dv_late")))
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .expire_invite_deliveries(deadline, Some(&secret_hash("dv_late")))
            .unwrap(),
        1
    );
    assert_eq!(copy(&store, "dv_swept").0.as_deref(), Some("dv_swept"));
    // The sweep at the deadline clears the other one issued then, and not
    // the one issued a millisecond later.
    assert_eq!(store.expire_invite_deliveries(deadline, None).unwrap(), 1);
    assert_eq!(copy(&store, "dv_swept").0, None);
    assert_eq!(copy(&store, "dv_young").0.as_deref(), Some("dv_young"));
    // An ended invite needs no delivery: the next sweep clears its copy.
    store
        .revoke_invite_by_hash(&secret_hash("dv_young"), deadline)
        .unwrap();
    assert_eq!(store.expire_invite_deliveries(deadline, None).unwrap(), 1);
    assert_eq!(copy(&store, "dv_young").0, None);
}

#[test]
fn first_use_clears_the_copy_and_records_when() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_at(directory.path().join("use.db").to_str(), ISSUED_AT).unwrap();
    issue(&store, "dv_used", "parlor", ISSUED_AT);
    assert!(end_delivery(&store, "dv_used", ISSUED_AT + 5));
    assert_eq!(copy(&store, "dv_used"), (None, Some(ISSUED_AT + 5)));
    // A second use changes nothing, not even the time.
    assert!(!end_delivery(&store, "dv_used", ISSUED_AT + 9));
    assert_eq!(copy(&store, "dv_used"), (None, Some(ISSUED_AT + 5)));
    assert_eq!(store.load_invites().unwrap().len(), 1, "the invite is live");
}

#[test]
fn the_upgrade_gives_a_copy_to_live_invites_inside_the_window_only() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("upgrade.db");
    let now = ISSUED_AT + INVITE_DELIVERY_WINDOW_MS;
    drop(Store::open_at(path.to_str(), ISSUED_AT).unwrap());
    // The tables as v0.13.0 left them, which had no delivery columns.
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(super::super::LEGACY_MEMBER_TABLES)
        .unwrap();
    for (token, room, at, revoked) in [
        ("dv_old", "a", ISSUED_AT, None),
        ("dv_recent", "b", now - 1, None),
        ("dv_ended", "c", now - 1, Some(now - 1)),
    ] {
        conn.execute(
            "INSERT INTO invites (token, room, name, kind, issued_at, issued_by, revoked_at, member)
             VALUES (?1, ?2, 'delivered', 'agent', ?3, 'master', ?4, '')",
            rusqlite::params![token, room, at, revoked],
        )
        .unwrap();
    }
    drop(conn);
    let store = Store::open_at(path.to_str(), now).unwrap();
    assert_eq!(copy(&store, "dv_recent"), (Some("dv_recent".into()), None));
    assert_eq!(copy(&store, "dv_old"), (None, None));
    assert_eq!(copy(&store, "dv_ended"), (None, None));
    drop(store);
    // Only that first open: a copy cleared later is not handed out again.
    let store = Store::open_at(path.to_str(), now).unwrap();
    assert!(end_delivery(&store, "dv_recent", now));
    drop(store);
    let store = Store::open_at(path.to_str(), now).unwrap();
    assert_eq!(copy(&store, "dv_recent").0, None);
}

/// How often `needle` occurs in each b-tree's pages of the file at `path`
/// (`None` for a page no b-tree owns: a free page), read through the
/// store's own connection after the WAL was checkpointed into the file.
pub(super) fn occurrences_by_owner(
    conn: &Connection,
    path: &std::path::Path,
    needle: &str,
) -> std::collections::BTreeMap<Option<String>, usize> {
    let page_size: u64 = conn
        .query_row("PRAGMA page_size", [], |r| r.get(0))
        .unwrap();
    let bytes = std::fs::read(path).unwrap();
    let mut out = std::collections::BTreeMap::new();
    for (offset, window) in bytes.windows(needle.len()).enumerate() {
        if window != needle.as_bytes() {
            continue;
        }
        let page = offset as u64 / page_size + 1;
        let owner: Option<String> = conn
            .query_row("SELECT name FROM dbstat WHERE pageno = ?1", [page], |r| {
                r.get(0)
            })
            .ok();
        *out.entry(owner).or_insert(0) += 1;
    }
    out
}

/// What a first use and an expiry leave of the invite in the database.
///
/// Invites are stored as hashes: the delivery copy is the only place the
/// invite itself was ever written. Once it is cleared the invite is in no
/// column, no page (a live b-tree's, or a free one) and, with the WAL
/// emptied, in no file.
#[test]
fn a_first_use_or_an_expiry_leaves_the_invite_nowhere() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("residue.db");
    let store = Store::open_at(path.to_str(), ISSUED_AT).unwrap();
    // Neighbours without a copy, so the rows share pages with others.
    for i in 0..12 {
        let token = format!("dv_neighbour{i}");
        issue(&store, &token, "near", ISSUED_AT);
        assert!(end_delivery(&store, &token, ISSUED_AT));
    }
    let used = marker_invite("USED");
    let lapsed = marker_invite("LAPSED");
    issue(&store, &used, "used", ISSUED_AT);
    issue(&store, &lapsed, "lapsed", ISSUED_AT);
    assert!(end_delivery(&store, &used, ISSUED_AT + 1_000));
    assert_eq!(
        store
            .expire_invite_deliveries(ISSUED_AT + INVITE_DELIVERY_WINDOW_MS, None)
            .unwrap(),
        1
    );
    assert!(!store.join_key_checkpoint_owed().unwrap());

    for key in [&used, &lapsed] {
        assert_eq!(files_containing(&path, key), Vec::<String>::new(), "{key}");
        let conn = store.conn().unwrap();
        assert_eq!(sql_values_containing(&conn, key), Vec::<String>::new());
        assert_eq!(
            occurrences_by_owner(&conn, &path, key),
            Default::default(),
            "no page of the file, live or free, holds {key}"
        );
    }
    assert!(!contains_bytes(
        &std::fs::read(directory.path().join("residue.db-wal")).unwrap_or_default(),
        b"dv_INVITEDELIVERY"
    ));
}

/// A reader holding the file keeps the WAL from being emptied. The first
/// use, the sweep and the open go on; the clean-up stays owed until it can
/// be done.
#[test]
fn a_busy_database_fails_nothing_and_keeps_the_clean_up_owed() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("busy.db");
    let used = marker_invite("BUSYUSE");
    let lapsed = marker_invite("BUSYLAPSE");
    let store = Store::open_at(path.to_str(), ISSUED_AT).unwrap();
    issue(&store, &used, "used", ISSUED_AT);
    issue(&store, &lapsed, "lapsed", ISSUED_AT);

    let reader = Connection::open(&path).unwrap();
    reader
        .execute_batch("BEGIN; SELECT count(*) FROM invites;")
        .unwrap();

    assert!(end_delivery(&store, &used, ISSUED_AT + 2));
    assert_eq!(copy(&store, &used).0, None);
    assert!(store.join_key_checkpoint_owed().unwrap());

    let late = ISSUED_AT + INVITE_DELIVERY_WINDOW_MS;
    assert_eq!(store.expire_invite_deliveries(late, None).unwrap(), 1);
    assert_eq!(copy(&store, &lapsed).0, None);
    assert!(store.join_key_checkpoint_owed().unwrap());

    drop(store);
    let store = Store::open_at(path.to_str(), late).expect("a busy WAL does not stop the start");
    assert!(store.join_key_checkpoint_owed().unwrap());

    reader.execute_batch("COMMIT").unwrap();
    drop(reader);
    assert_eq!(store.expire_invite_deliveries(late, None).unwrap(), 0);
    assert!(!store.join_key_checkpoint_owed().unwrap());
    assert!(!contains_bytes(
        &std::fs::read(directory.path().join("busy.db-wal")).unwrap_or_default(),
        b"dv_INVITEDELIVERY"
    ));
}

#[test]
fn an_unreadable_invite_or_member_table_is_an_error_not_an_empty_list() {
    for table in ["invites", "members"] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(format!("{table}.db"));
        let store = Store::open_at(path.to_str(), ISSUED_AT).unwrap();
        issue(&store, "dv_kept", "parlor", ISSUED_AT);
        Connection::open(&path)
            .unwrap()
            .execute_batch(&format!("DROP TABLE {table}"))
            .unwrap();
        match table {
            "invites" => assert!(store.load_invites().is_err()),
            _ => assert!(store.load_members().is_err()),
        }
    }
    // A row that cannot be read is an error too, not an invite left out.
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("row.db");
    let store = Store::open_at(path.to_str(), ISSUED_AT).unwrap();
    issue(&store, "dv_damaged", "parlor", ISSUED_AT);
    store
        .conn()
        .unwrap()
        .execute("UPDATE invites SET issued_at = 'not a time'", [])
        .unwrap();
    assert!(store.load_invites().is_err());
    store
        .add_member("mb_damaged", "damaged", "agent", ISSUED_AT, "master")
        .unwrap();
    store
        .conn()
        .unwrap()
        .execute("UPDATE members SET joined_at = 'not a time'", [])
        .unwrap();
    assert!(store.load_members().is_err());
}
