use protocol::{
    AttentionAudience, AttentionStatus, CareReason, CareSignal, ChatMode, RoomSettings,
};
use rusqlite::Connection;

use super::Store;

#[test]
fn identity_v2_migrates_legacy_roles_and_keeps_one_master_with_many_credentials() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("identity-v2.db");
    let legacy = Connection::open(&path).expect("legacy db");
    legacy
        .execute_batch(
            r#"
            CREATE TABLE members (
                token TEXT PRIMARY KEY, name TEXT NOT NULL, kind TEXT NOT NULL,
                joined_at INTEGER NOT NULL, admitted_by TEXT NOT NULL, revoked_at INTEGER
            );
            CREATE TABLE smasters (
                token TEXT PRIMARY KEY, name TEXT NOT NULL,
                issued_at INTEGER NOT NULL, revoked_at INTEGER
            );
            INSERT INTO members VALUES ('member-secret', 'bob', 'user', 10, 'master', NULL);
            INSERT INTO smasters VALUES ('smaster-secret', 'alice', 11, NULL);
            "#,
        )
        .expect("legacy identities");
    drop(legacy);

    let store = Store::open(Some(path.to_str().expect("db path"))).expect("migrate");
    store
        .ensure_master_principal("root-one", "operator", 12)
        .expect("bootstrap master");
    store
        .ensure_master_principal("root-two", "operator", 13)
        .expect("attach rotated root");
    // Run the complete migration again: neither principals nor credentials
    // duplicate, and the second root remains a credential of the same Master.
    drop(store);
    let store = Store::open(Some(path.to_str().expect("db path"))).expect("migrate twice");
    let c = store.conn().expect("persistent store");
    let masters: i64 = c
        .query_row(
            "SELECT count(*) FROM principals WHERE building_role = 'master' AND disabled_at IS NULL",
            [],
            |row| row.get(0),
        )
        .expect("master count");
    let roles: i64 = c
        .query_row("SELECT count(*) FROM principals", [], |row| row.get(0))
        .expect("principal count");
    let master_credentials: i64 = c
        .query_row(
            "SELECT count(*) FROM credentials c JOIN principals p ON p.id = c.principal_id
             WHERE p.building_role = 'master'",
            [],
            |row| row.get(0),
        )
        .expect("master credentials");
    assert_eq!(masters, 1);
    assert_eq!(roles, 3, "Master, Smaster, and Member principals");
    assert_eq!(
        master_credentials, 2,
        "one Master may have many credentials"
    );
    let raw_secret_rows: i64 = c
        .query_row(
            "SELECT count(*) FROM credentials
             WHERE secret_hash IN ('member-secret', 'smaster-secret', 'root-one', 'root-two')",
            [],
            |row| row.get(0),
        )
        .expect("secret audit");
    assert_eq!(
        raw_secret_rows, 0,
        "raw credentials never enter identity v2"
    );
}

#[test]
fn identity_v2_database_rejects_orphans_cross_principal_sessions_and_two_active_masters() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("identity-v2-constraints.db");
    let store = Store::open(Some(path.to_str().expect("db path"))).expect("store");
    store
        .ensure_master_principal("root-one", "operator", 10)
        .expect("master");
    let c = store.conn().expect("persistent store");

    let foreign_keys: i64 = c
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .expect("foreign key pragma");
    assert_eq!(
        foreign_keys, 1,
        "foreign keys must be enforced per connection"
    );

    let orphan = c.execute(
        "INSERT INTO credentials
         (id, principal_id, label, secret_hash, created_at)
         VALUES ('cr_orphan', 'pr_missing', 'attack', 'hash-orphan', 11)",
        [],
    );
    assert!(orphan.is_err(), "an orphan credential must fail closed");

    c.execute(
        "INSERT INTO principals
         (id, display_name, kind, building_role, created_at)
         VALUES ('pr_member', 'member', 'human', 'member', 11)",
        [],
    )
    .expect("member principal");
    c.execute(
        "INSERT INTO credentials
         (id, principal_id, label, secret_hash, created_at)
         VALUES ('cr_member', 'pr_member', 'member key', 'hash-member', 11)",
        [],
    )
    .expect("member credential");
    let master_id: String = c
        .query_row(
            "SELECT id FROM principals WHERE building_role = 'master'",
            [],
            |row| row.get(0),
        )
        .expect("master id");
    let crossed = c.execute(
        "INSERT INTO principal_sessions
         (id, principal_id, credential_id, created_at, expires_at)
         VALUES ('ss_crossed', ?1, 'cr_member', 12, 100)",
        rusqlite::params![master_id],
    );
    assert!(
        crossed.is_err(),
        "a session cannot claim a different principal than its credential owner"
    );

    let second_master = c.execute(
        "INSERT INTO principals
         (id, display_name, kind, building_role, created_at)
         VALUES ('pr_second_master', 'attacker', 'human', 'master', 12)",
        [],
    );
    assert!(
        second_master.is_err(),
        "the database must reject a second active Master"
    );
}

#[test]
fn live_member_and_smaster_mutations_keep_identity_v2_in_sync() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("identity-v2-live.db");
    let store = Store::open(Some(path.to_str().expect("db path"))).expect("store");
    let member = protocol::Membership {
        token: "mb_live".into(),
        name: "worker".into(),
        kind: "agent".into(),
        joined_at: 10,
        admitted_by: "master".into(),
    };
    store.add_member(&member).expect("add member");
    store
        .add_smaster("sm_live", "alice", 11)
        .expect("add smaster");
    assert_eq!(
        store
            .principal_for_credential("mb_live")
            .expect("member principal")
            .role,
        super::BuildingRole::Member
    );
    assert_eq!(
        store
            .principal_for_credential("sm_live")
            .expect("smaster principal")
            .role,
        super::BuildingRole::Smaster
    );

    store
        .revoke_member_cascade("mb_live", 20)
        .expect("revoke member");
    store.revoke_smaster("sm_live", 21).expect("revoke smaster");
    assert!(store.principal_for_credential("mb_live").is_none());
    assert!(store.principal_for_credential("sm_live").is_none());
}

#[test]
fn legacy_goal_and_task_rows_gain_explicit_progress_at_creation_time() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("legacy.db");
    let legacy = Connection::open(&path).expect("legacy db");
    legacy
        .execute_batch(
            r#"
            CREATE TABLE tasks (
                id INTEGER NOT NULL, room TEXT NOT NULL, title TEXT NOT NULL,
                created_by TEXT NOT NULL, from_message INTEGER, assigned_to TEXT,
                status TEXT NOT NULL, created_at INTEGER NOT NULL, closed_at INTEGER,
                PRIMARY KEY (room, id)
            );
            CREATE TABLE goals (
                id INTEGER NOT NULL, room TEXT NOT NULL, outcome TEXT NOT NULL,
                created_by TEXT NOT NULL, completion TEXT NOT NULL, task_ids TEXT NOT NULL,
                status TEXT NOT NULL, created_at INTEGER NOT NULL, closed_at INTEGER,
                PRIMARY KEY (room, id)
            );
            INSERT INTO tasks VALUES
                (1, 'proj', 'ship', 'operator', NULL, 'worker', 'taken', 1234, NULL);
            INSERT INTO goals VALUES
                (1, 'proj', 'public', 'operator', 'all_tasks', '[1]', 'active', 1200, NULL);
            "#,
        )
        .expect("legacy rows");
    drop(legacy);

    let store = Store::open(Some(path.to_str().expect("db path"))).expect("migrate");
    let task = store.load_tasks("proj").pop().expect("task");
    let goal = store.load_goals("proj").pop().expect("goal");
    assert_eq!(task.progress_at, task.created_at);
    assert_eq!(goal.progress_at, goal.created_at);
}

#[test]
fn legacy_care_outbox_is_promoted_to_attention_ledger() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("legacy-care.db");
    let legacy = Connection::open(&path).expect("legacy db");
    legacy
        .execute_batch(
            r#"
            CREATE TABLE care_outbox (
                id TEXT PRIMARY KEY,
                delivery_room TEXT NOT NULL,
                owner TEXT NOT NULL,
                signal TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                acked_at INTEGER
            );
            INSERT INTO care_outbox VALUES (
                'legacy-delivery', 'proj', 'lead',
                '{"id":"legacy-delivery","room":"proj","reason":"goal_reminder","owner":"lead","target":null,"participants":[],"subject":"goal stalled","context":[],"attempt":1,"at":1000,"escalated":false}',
                1000, NULL
            );
            "#,
        )
        .expect("legacy care row");
    drop(legacy);

    let store = Store::open(Some(path.to_str().expect("db path"))).expect("migrate");
    let attention = store.attentions("proj").pop().expect("attention backfill");
    assert_eq!(attention.id, "legacy-delivery");
    assert_eq!(attention.owner.as_deref(), Some("lead"));
    let delivery = store.pending_care("proj", "lead").pop().expect("delivery");
    assert_eq!(delivery.attention_id, "legacy-delivery");
    assert!(
        store.load().iter().any(|snapshot| snapshot.room == "proj"),
        "an attention-only loca must be discovered after restart"
    );
}

fn care_signal(id: &str, attention_id: &str, room: &str) -> CareSignal {
    care_signal_from(id, attention_id, room, "")
}

fn care_signal_from(id: &str, attention_id: &str, room: &str, source_room: &str) -> CareSignal {
    CareSignal {
        id: id.into(),
        attention_id: attention_id.into(),
        room: room.into(),
        source_room: source_room.into(),
        reason: CareReason::GoalReminder,
        audience: AttentionAudience::Person {
            name: "loca-care".into(),
        },
        owner: Some("loca-care".into()),
        owner_principal_id: None,
        group: None,
        target: None,
        participants: Vec::new(),
        subject: "stalled goal".into(),
        created_by: "care".into(),
        context: Vec::new(),
        attempt: 1,
        at: 1_000,
        escalated: false,
        state: protocol::ReminderState::Running,
    }
}

#[test]
fn room_rename_migrates_attention_identity_but_keeps_delivery_receipt() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("rename-attention.db");
    let store = Store::open(Some(path.to_str().expect("db path"))).expect("store");
    // Delivery is homed to the same loca the attention lives in (envelope
    // room == delivery room, the post-fix invariant); renaming that loca
    // must migrate the attention identity yet keep the delivery receipt.
    let signal = care_signal("delivery-stable", "attention:old:goal:1:1000", "old");
    store.enqueue_care("old", &signal).expect("enqueue");
    assert!(store.rename_room("old", "new").expect("rename"));
    assert!(store.attention("attention:old:goal:1:1000").is_none());
    let migrated = store
        .attention("attention:new:goal:1:1000")
        .expect("migrated attention");
    assert_eq!(migrated.room, "new");
    let pending = store
        .pending_care("new", "loca-care")
        .pop()
        .expect("pending delivery");
    assert_eq!(pending.id, "delivery-stable");
    assert_eq!(pending.attention_id, "attention:new:goal:1:1000");
    assert_eq!(pending.room, "new");
    assert!(store
        .ack_care_scoped("delivery-stable", "loca-care", None, 2_000)
        .expect("late ACK"));
}

#[test]
fn archive_pauses_attention_delivery_and_seal_resolves_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("archive-attention.db");
    let store = Store::open(Some(path.to_str().expect("db path"))).expect("store");
    // A re-homed caretaker relay: delivered in the home loca (iye) while its
    // attention ledger belongs to the source loca (proj). Archiving the
    // SOURCE loca must still pause the caretaker's home-loca delivery.
    let first = care_signal_from("delivery-1", "attention:proj:goal:1:1000", "iye", "proj");
    store.enqueue_care("iye", &first).expect("enqueue");
    let archived = RoomSettings {
        archived: true,
        ..RoomSettings::default()
    };
    store
        .save_room("proj", &ChatMode::default(), &archived)
        .expect("archive");
    assert!(store.pending_care("iye", "loca-care").is_empty());
    assert_eq!(
        store
            .attention("attention:proj:goal:1:1000")
            .expect("open attention retained")
            .status,
        AttentionStatus::Open
    );

    let second = care_signal_from("delivery-2", "attention:proj:goal:1:1000", "iye", "proj");
    store.enqueue_care("iye", &second).expect("resume delivery");
    assert_eq!(store.pending_care("iye", "loca-care").len(), 1);
    store.seal_room("proj", 3_000).expect("seal");
    assert!(store.pending_care("iye", "loca-care").is_empty());
    assert_eq!(
        store
            .attention("attention:proj:goal:1:1000")
            .expect("sealed attention history")
            .status,
        AttentionStatus::Resolved
    );
}

#[test]
fn admission_stock_mints_consumes_once_and_ignores_expired() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("admission.db");
    let store = Store::open(Some(path.to_str().expect("db path"))).expect("open");

    // A Master pre-mints 2 rights that expire at t=1000. Consumption is exercised
    // through the real consumer — `approve_join_request_atomic` — so the stock
    // semantics are tested on the path that actually runs in production.
    assert_eq!(
        store
            .mint_admission_rights(&["adm_0".into(), "adm_1".into()], "pr_master", 10, 1000)
            .expect("mint"),
        2
    );
    assert_eq!(store.admission_stock_counts(100), (2, 2));

    // Each approval consumes exactly one available right.
    store
        .create_join_request("jr_a", "sa", "agentA", "agent", 100)
        .expect("a");
    assert!(matches!(
        store.approve_join_request_atomic("jr_a", "mb_a", "master", 100),
        super::ApproveTxn::Committed(_)
    ));
    assert_eq!(store.admission_stock_counts(100), (2, 1));

    store
        .create_join_request("jr_b", "sb", "agentB", "agent", 100)
        .expect("b");
    assert!(matches!(
        store.approve_join_request_atomic("jr_b", "mb_b", "master", 100),
        super::ApproveTxn::Committed(_)
    ));
    assert_eq!(store.admission_stock_counts(100), (2, 0));

    // Stock drained -> the next approval is refused NoStock, consumes nothing,
    // and leaves the request PENDING (no stranded `approving`, retryable).
    store
        .create_join_request("jr_c", "sc", "agentC", "agent", 100)
        .expect("c");
    assert!(matches!(
        store.approve_join_request_atomic("jr_c", "mb_c", "master", 100),
        super::ApproveTxn::NoStock
    ));
    assert_eq!(store.admission_stock_counts(100), (2, 0));
    assert!(store
        .claim_join_request_bootstrap("jr_c", "sc", 150)
        .is_none());
    // After replenishing, the same still-pending request approves cleanly.
    assert_eq!(
        store
            .mint_admission_rights(&["adm_2".into()], "pr_master", 10, 1000)
            .expect("mint2"),
        1
    );
    assert!(matches!(
        store.approve_join_request_atomic("jr_c", "mb_c", "master", 120),
        super::ApproveTxn::Committed(_)
    ));

    // An already-expired right is stored but is never available or consumable.
    assert_eq!(
        store
            .mint_admission_rights(&["adm_exp".into()], "pr_master", 10, 50)
            .expect("mint expired"),
        1
    );
    assert_eq!(store.admission_stock_counts(100), (4, 0));
    store
        .create_join_request("jr_e", "se", "agentE", "agent", 100)
        .expect("e");
    assert!(matches!(
        store.approve_join_request_atomic("jr_e", "mb_e", "master", 100),
        super::ApproveTxn::NoStock
    ));
}

#[test]
fn join_request_approve_is_exactly_once_and_bootstrap_is_crash_safe() {
    let directory = tempfile::tempdir().expect("tempdir");
    let store =
        Store::open(Some(directory.path().join("jr.db").to_str().expect("path"))).expect("open");
    // Stock for the approvals below.
    store
        .mint_admission_rights(&["adm_0".into(), "adm_1".into()], "pr_master", 10, 10_000)
        .expect("mint");

    store
        .create_join_request("jr_1", "sekret", "bot", "agent", 100)
        .expect("create");
    // Only the matching secret can see the request; a wrong one learns nothing.
    assert!(store.join_request_view("jr_1", "wrong").is_none());
    let (status, name, ready) = store.join_request_view("jr_1", "sekret").expect("view");
    assert_eq!(
        (status.as_str(), name.as_str(), ready),
        ("pending", "bot", false)
    );
    assert_eq!(store.list_pending_join_requests().len(), 1);
    // A live request reserves its name (uniqueness guard for the takeover fix).
    assert!(store.has_pending_join_request_named("bot"));
    assert!(!store.has_pending_join_request_named("someone-else"));

    // Approve is exactly-once and ATOMIC: the first commits (consumes one right,
    // inserts member `bot`, marks approved, returns the fresh membership); a
    // second approve of the same id is AlreadyDecided and consumes no more stock.
    let out = store.approve_join_request_atomic("jr_1", "mb_bot", "master", 200);
    let super::ApproveTxn::Committed(m) = out else {
        panic!("expected Committed, got {out:?}");
    };
    assert_eq!(
        (m.token.as_str(), m.name.as_str(), m.kind.as_str()),
        ("mb_bot", "bot", "agent")
    );
    assert_eq!(store.admission_stock_counts(200), (2, 1));
    // The issued mb_ must AUTHENTICATE immediately on this persistent (file-backed)
    // store: approve writes the identity-v2 principal+credential rows too, not just
    // the `members` row, so the Lobby credential lookup resolves NOW rather than
    // only after the next restart-time migration. RED before the fix (approve wrote
    // only `members`, so this was None until a restart).
    let cred = store
        .principal_for_credential("mb_bot")
        .expect("approve-issued mb_ must resolve as a live credential on a persistent store");
    assert_eq!(cred.role, super::BuildingRole::Member);
    assert!(matches!(
        store.approve_join_request_atomic("jr_1", "mb_again", "master", 201),
        super::ApproveTxn::AlreadyDecided
    ));
    assert_eq!(store.admission_stock_counts(200), (2, 1)); // no extra consume

    // Crash-safe bootstrap: the mb_ is RE-FETCHABLE with the same secret until
    // the client ACKs, so a crash between receiving it and persisting it cannot
    // lose the credential. Repeated bootstraps return the SAME mb_.
    assert_eq!(
        store
            .claim_join_request_bootstrap("jr_1", "sekret", 300)
            .as_deref(),
        Some("mb_bot")
    );
    // Re-fetch (client crashed before persisting) still works — the window is open.
    assert_eq!(
        store
            .claim_join_request_bootstrap("jr_1", "sekret", 301)
            .as_deref(),
        Some("mb_bot")
    );
    // The requester's poll keeps signaling bootstrap_ready until the ACK.
    let (_, _, ready_before_ack) = store.join_request_view("jr_1", "sekret").expect("view");
    assert!(ready_before_ack, "bootstrap_ready must stay true until ACK");
    // A wrong secret can neither bootstrap nor ACK someone else's request, and
    // a failed ACK attempt changes NO state — the window stays open for the
    // rightful holder (the next real bootstrap still returns mb_).
    assert!(store
        .claim_join_request_bootstrap("jr_1", "nope", 302)
        .is_none());
    assert!(store
        .ack_join_request_bootstrap("jr_1", "nope", 303)
        .is_none());
    assert_eq!(
        store
            .claim_join_request_bootstrap("jr_1", "sekret", 304)
            .as_deref(),
        Some("mb_bot"),
        "a wrong-secret ACK must not close the window"
    );
    // Before the ACK the row still holds the plaintext mb_ copy.
    assert_eq!(
        store.join_request_mb_token_raw("jr_1"),
        Some(Some("mb_bot".to_string()))
    );
    // ACK finalizes: first returns Some(true); a repeat by the same secret is
    // idempotent Some(false); afterwards the bootstrap window is CLOSED (None).
    assert_eq!(
        store.ack_join_request_bootstrap("jr_1", "sekret", 305),
        Some(true)
    );
    assert_eq!(
        store.ack_join_request_bootstrap("jr_1", "sekret", 306),
        Some(false)
    );
    assert!(store
        .claim_join_request_bootstrap("jr_1", "sekret", 307)
        .is_none());
    // And the poll no longer advertises readiness once finalized.
    let (_, _, ready_after_ack) = store.join_request_view("jr_1", "sekret").expect("view");
    assert!(!ready_after_ack, "bootstrap_ready must be false after ACK");
    // The ACK scrubbed the plaintext mb_ copy from the request row — a DB dump
    // retains no delivered credential (the live membership lives in the members
    // table, unaffected: it still resolves).
    assert_eq!(store.join_request_mb_token_raw("jr_1"), Some(None));
    assert_eq!(
        store
            .principal_for_credential("mb_bot")
            .expect("live membership survives the request-row scrub")
            .role,
        super::BuildingRole::Member
    );

    // Name-collision (identity-takeover fix): a second request for a name that is
    // now a live member is refused NameTaken — no second member is inserted, no
    // stock is consumed, the request stays pending, and no foreign mb_ leaks.
    store
        .create_join_request("jr_3", "s3", "bot", "agent", 100)
        .expect("create3");
    assert!(matches!(
        store.approve_join_request_atomic("jr_3", "mb_impostor", "master", 200),
        super::ApproveTxn::NameTaken
    ));
    assert_eq!(store.admission_stock_counts(200), (2, 1)); // unchanged
    assert!(store
        .claim_join_request_bootstrap("jr_3", "s3", 400)
        .is_none());
    let (status3, _, _) = store.join_request_view("jr_3", "s3").expect("view3");
    assert_eq!(status3, "pending");

    // Deny is one-shot too: a pending request denies once, then no longer.
    store
        .create_join_request("jr_2", "s2", "bot2", "agent", 100)
        .expect("create2");
    assert!(store.deny_join_request("jr_2", "master", 200));
    assert!(!store.deny_join_request("jr_2", "master", 201));
}

#[test]
fn join_request_bootstrap_ack_survives_restart_and_reopen_migration() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("r.db");
    let p = path.to_str().expect("path");
    {
        let store = Store::open(Some(p)).expect("open");
        store
            .mint_admission_rights(&["adm_r".into()], "pr_master", 10, 10_000)
            .expect("mint");
        store
            .create_join_request("jr_r", "sr", "rbot", "agent", 100)
            .expect("create");
        let super::ApproveTxn::Committed(_) =
            store.approve_join_request_atomic("jr_r", "mb_r", "master", 200)
        else {
            panic!("approve");
        };
        assert_eq!(
            store
                .claim_join_request_bootstrap("jr_r", "sr", 300)
                .as_deref(),
            Some("mb_r")
        );
        assert_eq!(
            store.ack_join_request_bootstrap("jr_r", "sr", 301),
            Some(true)
        );
    } // drop → the connection closes, simulating a server restart.

    // Reopen the SAME file: the additive `ALTER TABLE ... ADD COLUMN` re-runs
    // harmlessly (duplicate-column error is swallowed), the ACK persisted, the
    // window stays closed, the credential copy stays scrubbed, and a repeat ACK
    // is still idempotent across the restart.
    let store2 = Store::open(Some(p)).expect("reopen");
    assert!(store2
        .claim_join_request_bootstrap("jr_r", "sr", 400)
        .is_none());
    assert_eq!(
        store2.ack_join_request_bootstrap("jr_r", "sr", 401),
        Some(false)
    );
    assert_eq!(store2.join_request_mb_token_raw("jr_r"), Some(None));
    let (_, _, ready) = store2.join_request_view("jr_r", "sr").expect("view");
    assert!(!ready, "restart must not reopen the bootstrap window");
}

#[test]
fn join_request_bootstrap_ack_is_atomic_under_concurrency() {
    use std::sync::Arc;
    let directory = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(
        Store::open(Some(directory.path().join("c.db").to_str().expect("path"))).expect("open"),
    );
    store
        .mint_admission_rights(&["adm_c".into()], "pr_master", 10, 10_000)
        .expect("mint");
    store
        .create_join_request("jr_c", "sc", "cbot", "agent", 100)
        .expect("create");
    let super::ApproveTxn::Committed(_) =
        store.approve_join_request_atomic("jr_c", "mb_c", "master", 200)
    else {
        panic!("approve");
    };

    // Fire many concurrent bootstraps alongside one ACK. The store's single held
    // connection guard serializes every op, so there is no torn state: each
    // bootstrap that succeeds returns the SAME credential (never a divergent one),
    // and no bootstrap succeeds once the ACK has committed.
    let mut handles = vec![];
    for i in 0..8u64 {
        let s = Arc::clone(&store);
        handles.push(std::thread::spawn(move || {
            s.claim_join_request_bootstrap("jr_c", "sc", 300 + i)
        }));
    }
    let sack = Arc::clone(&store);
    let ack = std::thread::spawn(move || sack.ack_join_request_bootstrap("jr_c", "sc", 999));

    for h in handles {
        if let Some(mb) = h.join().expect("bootstrap thread") {
            assert_eq!(mb, "mb_c", "concurrent bootstraps never diverge");
        }
    }
    assert!(ack.join().expect("ack thread").is_some());

    // Once every thread has joined the ACK is committed: the window is closed and
    // the credential copy scrubbed, no matter how the threads interleaved above.
    assert!(store
        .claim_join_request_bootstrap("jr_c", "sc", 1000)
        .is_none());
    assert_eq!(store.join_request_mb_token_raw("jr_c"), Some(None));
}

#[test]
fn approve_records_the_deciding_authority_for_audit() {
    // The approving authority is written to `decided_by`, so a Smaster approval
    // is auditable as `smaster:<name>` (the route computes that; here we lock
    // that the store faithfully records whatever authority approved).
    let directory = tempfile::tempdir().expect("tempdir");
    let store = Store::open(Some(
        directory.path().join("audit.db").to_str().expect("path"),
    ))
    .expect("open");
    store
        .mint_admission_rights(&["adm_0".into()], "pr_master", 10, 10_000)
        .expect("mint");
    store
        .create_join_request("jr_a", "sa", "guest", "agent", 100)
        .expect("create");
    assert!(matches!(
        store.approve_join_request_atomic("jr_a", "mb_g", "smaster:deputy", 200),
        super::ApproveTxn::Committed(_)
    ));
    let decided_by: String = store
        .conn()
        .expect("persistent store")
        .query_row(
            "SELECT decided_by FROM join_requests WHERE id = ?1",
            ["jr_a"],
            |r| r.get(0),
        )
        .expect("decided_by");
    assert_eq!(
        decided_by, "smaster:deputy",
        "the approving authority is recorded for audit"
    );
}
