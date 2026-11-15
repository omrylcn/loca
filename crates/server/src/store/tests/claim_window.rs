//! A claimed membership key's delivery copy, at the store: what an ACK or an
//! expiry leaves in the database file, and what a busy database does.

use rusqlite::Connection;

use super::super::{secret_hash, MemberClaim, Store, CLAIM_DELIVERY_WINDOW_MS};
use super::{contains_bytes, files_containing, sql_values_containing};

/// A claim time shaped like a real one (milliseconds since the epoch).
const CLAIMED_AT: u64 = 1_800_000_000_000;

/// A key shaped like the server's around a marker nothing else in the file
/// contains, so a byte search is meaningful.
fn marker_key(tag: &str) -> String {
    format!("mb_CLAIMKEYRESIDUE{tag}Q7ZK3W9X2M")
}

/// A member whose principal the claimed keys belong to. Its original key is
/// unrelated to the claimed ones, which are what this module searches for.
fn member(store: &Store) -> String {
    store
        .add_member(
            "mb_originalmemberkey",
            "worker",
            "agent",
            CLAIMED_AT - 60_000,
            "master",
        )
        .unwrap()
        .principal_id
        .unwrap()
}

fn claim(store: &Store, principal: &str, setup: &str, key: &str, at: u64) -> MemberClaim {
    claim_on(store, "ivid_residue", principal, setup, key, at)
}

fn claim_on(
    store: &Store,
    invite: &str,
    principal: &str,
    setup: &str,
    key: &str,
    at: u64,
) -> MemberClaim {
    store
        .claim_member_key(
            invite,
            Some(&secret_hash(setup)),
            principal,
            "Membership key (claimed with davet for #parlor)",
            key,
            at,
        )
        .unwrap()
}

/// After an ACK and after an expiry a claimed key is in no column, no page
/// and no file: unlike a join request's key, it is stored nowhere else in
/// clear text (its credential holds only its hash).
#[test]
fn an_ack_or_an_expiry_leaves_the_claimed_key_nowhere_in_the_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("claim-residue.db");
    let store = Store::open_at(path.to_str(), CLAIMED_AT).unwrap();
    let principal = member(&store);
    // Neighbours (each on an invite of its own, so none meets the cap), so
    // the claim rows share pages with others.
    for i in 0..20 {
        assert!(matches!(
            claim_on(
                &store,
                &format!("ivid_neighbour{i}"),
                &principal,
                "cl_neighbourxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
                &format!("mb_neighbour{i}"),
                CLAIMED_AT,
            ),
            MemberClaim::Delivered { fresh: true, .. }
        ));
    }
    let acked = marker_key("ACKED");
    let lapsed = marker_key("LAPSED");
    let (setup_a, setup_l) = (
        "cl_ackedsetupxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
        "cl_lapsedsetupxxxxxxxxxxxxxxxxxxxxxxxxxxx",
    );
    for (setup, key) in [(setup_a, &acked), (setup_l, &lapsed)] {
        for at in [CLAIMED_AT, CLAIMED_AT + 1_000, CLAIMED_AT + 2_000] {
            assert!(matches!(
                claim(&store, &principal, setup, key, at),
                MemberClaim::Delivered { .. }
            ));
        }
    }
    // Before: each key is held exactly once, as its set-up's delivery copy.
    for key in [&acked, &lapsed] {
        let conn = store.conn().unwrap();
        assert_eq!(
            sql_values_containing(&conn, key),
            vec!["membership_claims.delivery_token".to_string()]
        );
    }
    assert_eq!(
        store
            .ack_member_claim("ivid_residue", &secret_hash(setup_a), CLAIMED_AT + 3_000)
            .unwrap(),
        Some(true)
    );
    // Twenty neighbours and the lapsed set-up are past their deadline.
    assert_eq!(
        store
            .expire_join_request_bootstraps(CLAIMED_AT + CLAIM_DELIVERY_WINDOW_MS)
            .unwrap(),
        21
    );
    assert!(!store.join_key_checkpoint_owed().unwrap());

    let conn = store.conn().unwrap();
    for key in [&acked, &lapsed] {
        assert_eq!(
            sql_values_containing(&conn, key),
            Vec::<String>::new(),
            "{key}"
        );
        assert_eq!(files_containing(&path, key), Vec::<String>::new(), "{key}");
        // The credential is there, by hash.
        let live: bool = conn
            .query_row(
                "SELECT revoked_at IS NULL FROM credentials WHERE secret_hash = ?1",
                [secret_hash(key)],
                |r| r.get(0),
            )
            .unwrap();
        assert!(live, "only the copy ended, not the credential");
    }
    assert!(!contains_bytes(
        &std::fs::read(directory.path().join("claim-residue.db-wal")).unwrap_or_default(),
        b"mb_CLAIMKEYRESIDUE"
    ));
}

/// A reader holding the file keeps the WAL from being emptied. The ACK, a
/// late claim, the open and the sweep go on, and the clean-up stays owed
/// until it can be done.
#[test]
fn a_busy_database_fails_no_claim_and_keeps_the_clean_up_owed() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("claim-busy.db");
    let acked = marker_key("BUSYACK");
    let lapsed = marker_key("BUSYLAPSE");
    let store = Store::open_at(path.to_str(), CLAIMED_AT).unwrap();
    let principal = member(&store);
    let (setup_a, setup_l) = (
        "cl_busyackedxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
        "cl_busylapsedxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
    );
    claim(&store, &principal, setup_a, &acked, CLAIMED_AT);
    claim(&store, &principal, setup_l, &lapsed, CLAIMED_AT);

    let reader = Connection::open(&path).unwrap();
    reader
        .execute_batch("BEGIN; SELECT count(*) FROM membership_claims;")
        .unwrap();

    // The ACK succeeds and the copy is gone from the row; the WAL still
    // holds it, and the debt is recorded.
    assert_eq!(
        store
            .ack_member_claim("ivid_residue", &secret_hash(setup_a), CLAIMED_AT + 2)
            .unwrap(),
        Some(true)
    );
    assert_eq!(
        store.member_claim_copy_raw("ivid_residue", &secret_hash(setup_a)),
        Some(None)
    );
    assert!(store.join_key_checkpoint_owed().unwrap());
    assert!(files_containing(&path, &acked).contains(&"claim-busy.db-wal".to_string()));

    // A late repeat, refused and cleared, while busy; nothing is minted.
    let late = CLAIMED_AT + CLAIM_DELIVERY_WINDOW_MS;
    assert_eq!(
        claim(&store, &principal, setup_l, "mb_mustnotbeminted", late),
        MemberClaim::DeliveryEnded
    );
    assert_eq!(
        store.member_claim_copy_raw("ivid_residue", &secret_hash(setup_l)),
        Some(None)
    );
    assert_eq!(store.member_claim_count("ivid_residue"), 2);
    assert!(store.join_key_checkpoint_owed().unwrap());

    // A restart while busy starts, and still owes it.
    drop(store);
    let store = Store::open_at(path.to_str(), late).expect("a busy WAL does not stop the start");
    assert!(store.join_key_checkpoint_owed().unwrap());

    // Once the reader is gone the sweep pays it.
    reader.execute_batch("COMMIT").unwrap();
    drop(reader);
    assert_eq!(store.expire_join_request_bootstraps(late).unwrap(), 0);
    assert!(!store.join_key_checkpoint_owed().unwrap());
    for key in [&acked, &lapsed] {
        assert_eq!(files_containing(&path, key), Vec::<String>::new(), "{key}");
    }
}
