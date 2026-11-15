use protocol::{
    AttentionAudience, AttentionStatus, CareReason, CareSignal, ChatMode, RoomSettings,
};
use rusqlite::Connection;

use super::Store;

mod claim_window;
mod invite_delivery;
mod join_window;
mod key_scopes;
mod member_keys;
mod notification_limit;
mod preferences;
mod profile_card;
mod upgrade_v0_13_0;

#[test]
fn failed_suspension_rolls_back_access_and_suspension_marker() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("suspension-rollback.db");
    let store = Store::open(path.to_str()).unwrap();
    let key = "rollback-fixture";
    let member = store
        .add_member(key, "worker", "agent", 1, "master")
        .unwrap();
    store.conn().unwrap().execute_batch("CREATE TEMP TRIGGER reject_suspend BEFORE UPDATE ON principals BEGIN SELECT RAISE(ABORT, 'fixture failure'); END;").unwrap();
    assert!(store.suspend_member(&member.id, "test", 2).is_err());
    assert!(!store.member_suspended(&member.id).unwrap());
    assert!(store.principal_for_credential(key).is_some());
}

#[test]
fn member_suspension_survives_restart_preserves_identity_and_cannot_undo_revocation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("suspension.db");
    let store = Store::open(Some(path.to_str().unwrap())).unwrap();
    let key = "suspension-fixture";
    let member = store
        .add_member(key, "worker", "agent", 1, "master")
        .unwrap();
    let id = store.principal_for_credential(key).unwrap().id;
    assert!(store.suspend_member(&member.id, "maintenance", 2).unwrap());
    assert!(store.member_suspended(&member.id).unwrap());
    assert!(store.principal_for_credential(key).is_none());
    drop(store);
    let store = Store::open(Some(path.to_str().unwrap())).unwrap();
    assert!(store.member_suspended(&member.id).unwrap());
    assert!(store.principal_for_credential(key).is_none());
    assert!(store.resume_member(&member.id).unwrap());
    assert_eq!(store.principal_for_credential(key).unwrap().id, id);
    assert!(!store.resume_member(&member.id).unwrap());
    store.suspend_member(&member.id, "maintenance", 3).unwrap();
    store.revoke_member_cascade(&member.id, 4).unwrap();
    assert!(!store.resume_member(&member.id).unwrap());
    assert!(store.principal_for_credential(key).is_none());
}

#[test]
fn room_founder_creation_is_atomic_and_survives_restart() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("founder.db");
    let store = Store::open(Some(path.to_str().unwrap())).unwrap();
    store
        .ensure_master_principal("fixture-root", "founder", 1)
        .unwrap();
    let founder = store.active_master_principal().unwrap().id;
    {
        let c = store.conn().unwrap();
        c.execute_batch(
            "CREATE TEMP TRIGGER reject_founder BEFORE INSERT ON room_operator_assignments
            BEGIN SELECT RAISE(ABORT, 'simulated appointment failure'); END;",
        )
        .unwrap();
    }
    assert!(store
        .create_room_with_operator("new-room", &RoomSettings::default(), &founder, None, 2)
        .is_err());
    {
        let c = store.conn().unwrap();
        let count: i64 = c
            .query_row(
                "SELECT count(*) FROM rooms WHERE room='new-room'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "failed appointment must roll back the room too");
        c.execute_batch("DROP TRIGGER reject_founder").unwrap();
    }
    let assigned = store
        .create_room_with_operator("new-room", &RoomSettings::default(), &founder, None, 3)
        .unwrap();
    assert_eq!(assigned.principal_id, founder);
    drop(store);
    let store = Store::open(Some(path.to_str().unwrap())).unwrap();
    assert_eq!(
        store.loca_operator("new-room").unwrap().principal_id,
        founder
    );
    assert!(matches!(
        store.create_room_with_operator("new-room", &RoomSettings::default(), &founder, None, 4),
        Err(super::LocaOperatorError::Conflict)
    ));
    assert_eq!(store.loca_operator_history("new-room").len(), 1);
}

#[test]
fn operator_appointment_follows_identity_not_renamed_or_reused_labels() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("operator-labels.db");
    let store = Store::open(Some(path.to_str().unwrap())).unwrap();
    store
        .ensure_master_principal("fixture-root", "founder", 1)
        .unwrap();
    let master = store.active_master_principal().unwrap().id;
    store
        .create_room_with_operator("r", &RoomSettings::default(), &master, None, 2)
        .unwrap();
    let add = |key: &str| {
        store
            .add_member(key, "alice", "user", 3, "founder")
            .unwrap()
    };
    add("old-person");
    let original = store.principal_for_credential("old-person").unwrap().id;
    store
        .appoint_loca_operator("r", &original, &master, 4)
        .unwrap();
    store
        .conn()
        .unwrap()
        .execute(
            "UPDATE principals SET display_name='renamed' WHERE id=?1",
            [&original],
        )
        .unwrap();
    assert_eq!(store.loca_operator("r").unwrap().principal_id, original);
    assert_eq!(store.loca_operator("r").unwrap().display_name, "renamed");
    add("new-person");
    let replacement = store.principal_for_credential("new-person").unwrap().id;
    assert_ne!(original, replacement);
    drop(store);
    let store = Store::open(Some(path.to_str().unwrap())).unwrap();
    let operator = store.loca_operator("r").unwrap();
    assert_eq!(operator.principal_id, original);
    assert_ne!(operator.principal_id, replacement);
}

#[test]
fn critical_save_room_does_not_erase_permanent_seal() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(Some(temp.path().join("sealed.db").to_str().unwrap())).unwrap();
    store.seal_room("sealed", 1).unwrap();
    assert!(store
        .save_room("sealed", &ChatMode::Free, &RoomSettings::default())
        .is_err());
    assert_eq!(store.sealed_rooms().unwrap(), vec!["sealed"]);
}

#[test]
fn memory_owner_migration_binds_once_and_never_grants_reused_or_late_names() {
    for scenario in ["one", "missing", "reused"] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("loca.db");
        let store = Store::open(Some(path.to_str().unwrap())).unwrap();
        let member = |store: &Store, key: &str| {
            store
                .add_member(key, "alice", "agent", 1, "master")
                .unwrap()
                .id
        };
        if scenario != "missing" {
            let old = member(&store, "old-owner");
            if scenario == "reused" {
                store.revoke_member_cascade(&old, 2).unwrap();
                member(&store, "new-owner");
            }
        }
        {
            let c = store.conn().unwrap();
            c.execute(
                "INSERT INTO loca_memory (room,owner) VALUES ('r','alice')",
                [],
            )
            .unwrap();
            c.execute("ALTER TABLE loca_memory DROP COLUMN owner_principal_id", [])
                .unwrap();
        }
        drop(store);
        let store = Store::open(Some(path.to_str().unwrap())).unwrap();
        let bound: Option<String> = store
            .conn()
            .unwrap()
            .query_row(
                "SELECT owner_principal_id FROM loca_memory WHERE room='r'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        if scenario == "one" {
            assert_eq!(
                bound,
                Some(store.principal_for_credential("old-owner").unwrap().id)
            );
        } else {
            assert!(bound.is_none());
        }
        if scenario == "missing" {
            member(&store, "late-owner");
            drop(store);
            let store = Store::open(Some(path.to_str().unwrap())).unwrap();
            let bound: Option<String> = store
                .conn()
                .unwrap()
                .query_row(
                    "SELECT owner_principal_id FROM loca_memory WHERE room='r'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(
                bound.is_none(),
                "late admission must not gain an old unassigned role"
            );
        }
    }
}

#[test]
fn corrupt_authorization_state_never_loads_as_a_free_room() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("loca.db");
    let store = Store::open(Some(path.to_str().unwrap())).unwrap();
    store
        .save_room("closed", &ChatMode::Free, &RoomSettings::default())
        .unwrap();
    for column in ["mode", "settings"] {
        {
            let c = store.conn().unwrap();
            c.execute(
                &format!("UPDATE rooms SET {column} = 'invalid-json' WHERE room = 'closed'"),
                [],
            )
            .unwrap();
        }
        assert!(
            store.load().is_err(),
            "bad {column} must stop startup, not erase restrictions"
        );
        store
            .save_room("closed", &ChatMode::Free, &RoomSettings::default())
            .unwrap();
    }
    store
        .conn()
        .unwrap()
        .execute("DROP TABLE rooms", [])
        .unwrap();
    assert!(store.load().is_err());
    assert!(store.sealed_rooms().is_err());
}

#[test]
fn durable_content_read_errors_are_not_empty_success() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("loca.db");
    let store = Store::open(Some(path.to_str().unwrap())).unwrap();
    store
        .conn()
        .unwrap()
        .execute_batch("DROP TABLE messages; DROP TABLE journal; DROP TABLE note_revisions;")
        .unwrap();
    assert!(store.search_messages("r", "hello", 20).is_err());
    assert!(store.load_journal("r").is_err());
    assert!(store.note_history("r", "note").is_err());
}

#[test]
fn archive_search_is_unicode_case_insensitive_and_literal() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("loca.db");
    let store = Store::open(Some(path.to_str().unwrap())).unwrap();
    store.conn().unwrap().execute("INSERT INTO messages (id,room,sender,sender_type,text,ts,kind) VALUES (1,'r','alice','agent','ŞİRKET 100% task_key','1','chat')", []).unwrap();
    for needle in ["şirket", "ŞİRKET", "100%", "task_key"] {
        assert_eq!(
            store.search_messages("r", needle, 20).unwrap().len(),
            1,
            "{needle}"
        );
    }
    for needle in ["x%", "taskXkey", "___"] {
        assert!(
            store.search_messages("r", needle, 20).unwrap().is_empty(),
            "{needle} is not a wildcard"
        );
    }
}

#[test]
fn note_replacement_failure_rolls_back_archived_revision() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("loca.db");
    let store = Store::open(Some(path.to_str().unwrap())).unwrap();
    let previous = protocol::Note {
        key: "proof".into(),
        title: "old".into(),
        body: "old".into(),
        can_write: vec![],
        updated_by: "alice".into(),
        updated_at: 1,
        rev: 1,
    };
    store.upsert_note("r", &previous).unwrap();
    let mut updated = previous.clone();
    updated.rev = 2;
    updated.body = "new".into();
    store.conn().unwrap().execute_batch("CREATE TRIGGER fail_note BEFORE INSERT ON notes BEGIN SELECT RAISE(ABORT, 'fault injection'); END;").unwrap();
    assert!(store.replace_note("r", &previous, &updated).is_err());
    assert!(store.note_history("r", "proof").unwrap().is_empty());
    assert_eq!(store.load().unwrap()[0].notes[0].body, "old");
}

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
    let member = store
        .add_member("mb_live", "worker", "agent", 10, "master")
        .expect("add member");
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
        .revoke_member_cascade(&member.id, 20)
        .expect("revoke member");
    store
        .revoke_smaster(&super::secret_hash("sm_live"), 21)
        .expect("revoke smaster");
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
        store
            .load()
            .unwrap()
            .iter()
            .any(|snapshot| snapshot.room == "proj"),
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

/// A collection as the join-request tests below were written against it,
/// all inside the delivery window: the key, or nothing.
fn claimed(store: &Store, id: &str, secret: &str, now: u64) -> Option<String> {
    match store
        .claim_join_request_bootstrap(id, secret, now)
        .expect("claim reads the store")
    {
        super::BootstrapClaim::Delivered(key) => Some(key),
        super::BootstrapClaim::Expired => panic!("these tests stay inside the window"),
        super::BootstrapClaim::Unavailable => None,
    }
}

fn acked(store: &Store, id: &str, secret: &str, now: u64) -> Option<bool> {
    store
        .ack_join_request_bootstrap(id, secret, now)
        .expect("ack reads the store")
}

/// `(status, name, bootstrap_ready)`, read well inside every window these
/// tests open (their approvals are at small times).
fn viewed(store: &Store, id: &str, secret: &str) -> Option<(String, String, bool)> {
    store
        .join_request_view(id, secret, 1_000)
        .expect("view reads the store")
        .map(|view| {
            assert!(!view.bootstrap_expired);
            (view.status, view.name, view.bootstrap_ready)
        })
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
    assert!(claimed(&store, "jr_c", "sc", 150).is_none());
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
    assert!(viewed(&store, "jr_1", "wrong").is_none());
    let (status, name, ready) = viewed(&store, "jr_1", "sekret").expect("view");
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
        (m.id.as_str(), m.name.as_str(), m.kind.as_str()),
        (
            super::hashed_id("mbid_", "member:mb_bot").as_str(),
            "bot",
            "agent"
        )
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
        claimed(&store, "jr_1", "sekret", 300).as_deref(),
        Some("mb_bot")
    );
    // Re-fetch (client crashed before persisting) still works — the window is open.
    assert_eq!(
        claimed(&store, "jr_1", "sekret", 301).as_deref(),
        Some("mb_bot")
    );
    // The requester's poll keeps signaling bootstrap_ready until the ACK.
    let (_, _, ready_before_ack) = viewed(&store, "jr_1", "sekret").expect("view");
    assert!(ready_before_ack, "bootstrap_ready must stay true until ACK");
    // A wrong secret can neither bootstrap nor ACK someone else's request, and
    // a failed ACK attempt changes NO state — the window stays open for the
    // rightful holder (the next real bootstrap still returns mb_).
    assert!(claimed(&store, "jr_1", "nope", 302).is_none());
    assert!(acked(&store, "jr_1", "nope", 303).is_none());
    assert_eq!(
        claimed(&store, "jr_1", "sekret", 304).as_deref(),
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
    assert_eq!(acked(&store, "jr_1", "sekret", 305), Some(true));
    assert_eq!(acked(&store, "jr_1", "sekret", 306), Some(false));
    assert!(claimed(&store, "jr_1", "sekret", 307).is_none());
    // And the poll no longer advertises readiness once finalized.
    let (_, _, ready_after_ack) = viewed(&store, "jr_1", "sekret").expect("view");
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
    assert!(claimed(&store, "jr_3", "s3", 400).is_none());
    let (status3, _, _) = viewed(&store, "jr_3", "s3").expect("view3");
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
        assert_eq!(claimed(&store, "jr_r", "sr", 300).as_deref(), Some("mb_r"));
        assert_eq!(acked(&store, "jr_r", "sr", 301), Some(true));
    } // drop → the connection closes, simulating a server restart.

    // Reopen the SAME file: the additive `ALTER TABLE ... ADD COLUMN` re-runs
    // harmlessly (an existing column is not an error), the ACK persisted, the
    // window stays closed, the credential copy stays scrubbed, and a repeat ACK
    // is still idempotent across the restart.
    let store2 = Store::open(Some(p)).expect("reopen");
    assert!(claimed(&store2, "jr_r", "sr", 400).is_none());
    assert_eq!(acked(&store2, "jr_r", "sr", 401), Some(false));
    assert_eq!(store2.join_request_mb_token_raw("jr_r"), Some(None));
    let (_, _, ready) = viewed(&store2, "jr_r", "sr").expect("view");
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
            claimed(&s, "jr_c", "sc", 300 + i)
        }));
    }
    let sack = Arc::clone(&store);
    let ack = std::thread::spawn(move || acked(&sack, "jr_c", "sc", 999));

    for h in handles {
        if let Some(mb) = h.join().expect("bootstrap thread") {
            assert_eq!(mb, "mb_c", "concurrent bootstraps never diverge");
        }
    }
    assert!(ack.join().expect("ack thread").is_some());

    // Once every thread has joined the ACK is committed: the window is closed and
    // the credential copy scrubbed, no matter how the threads interleaved above.
    assert!(claimed(&store, "jr_c", "sc", 1000).is_none());
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

/// (table, column, declared type) for every column of every table.
fn schema_columns(conn: &Connection) -> Vec<(String, String, String)> {
    let mut stmt = conn
        .prepare(
            "SELECT m.name, p.name, p.type FROM sqlite_master m
             JOIN pragma_table_info(m.name) p
             WHERE m.type = 'table' AND m.name NOT LIKE 'sqlite_%'
             ORDER BY m.name, p.cid",
        )
        .expect("schema");
    let rows = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .expect("columns");
    rows.map(|row| row.expect("column")).collect()
}

/// Every table the schema keys by a loca name, in a `room` column.
fn tables_with_a_room_column(columns: &[(String, String, String)]) -> Vec<String> {
    let mut tables: Vec<String> = columns
        .iter()
        .filter(|(_, column, _)| column == "room")
        .map(|(table, _, _)| table.clone())
        .collect();
    tables.dedup();
    tables
}

/// The guard for "a loca's records follow its name": every table with a
/// `room` column is either moved by `rename_room` or listed as a deliberate
/// exception, and every other column that names a loca is one the rename
/// handles by hand. Adding a loca table without deciding fails here.
#[test]
fn every_table_keyed_by_a_loca_name_is_classified_for_rename() {
    use std::collections::BTreeSet;
    let directory = tempfile::tempdir().expect("tempdir");
    let store = Store::open(directory.path().join("schema.db").to_str()).expect("store");
    let conn = store.conn().expect("connection");
    let columns = schema_columns(&conn);

    let with_room: BTreeSet<String> = tables_with_a_room_column(&columns).into_iter().collect();
    let classified: BTreeSet<String> = super::rooms::ROOM_KEYED_TABLES
        .iter()
        .chain(super::rooms::ROOM_TABLES_NEVER_RENAMED)
        .map(|table| table.to_string())
        .collect();
    assert_eq!(
        with_room, classified,
        "a table with a `room` column must be added to ROOM_KEYED_TABLES (rename_room moves it) \
         or to ROOM_TABLES_NEVER_RENAMED with the reason"
    );

    // Loca references under another column name. `care_outbox.delivery_room`
    // is rewritten by rename_room together with the signal JSON beside it.
    let other: BTreeSet<(&str, &str)> = columns
        .iter()
        .filter(|(_, column, _)| {
            column != "room" && (column.contains("room") || column.contains("loca"))
        })
        .map(|(table, column, _)| (table.as_str(), column.as_str()))
        .collect();
    assert_eq!(
        other,
        BTreeSet::from([("care_outbox", "delivery_room")]),
        "a new column naming a loca must be handled in rename_room and listed here"
    );
}

/// One row per loca table under the old name; after the rename none is left
/// behind and each is found under the new name. The tables are read from the
/// schema, not from the rename's own list, so a table the rename forgets
/// fails here.
#[test]
fn a_rename_leaves_no_row_behind_in_any_loca_table() {
    let directory = tempfile::tempdir().expect("tempdir");
    let store = Store::open(directory.path().join("rename-all.db").to_str()).expect("store");
    // The attention row carries JSON the rename parses: use the real writer.
    let signal = care_signal("delivery-all", "attention:old:goal:1:1000", "old");
    store.enqueue_care("old", &signal).expect("enqueue");
    let tables: Vec<String> = {
        let conn = store.conn().expect("connection");
        // Fixture rows stand alone: no principals, groups or messages exist.
        conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
        let columns = schema_columns(&conn);
        let tables: Vec<String> = tables_with_a_room_column(&columns)
            .into_iter()
            .filter(|table| !super::rooms::ROOM_TABLES_NEVER_RENAMED.contains(&table.as_str()))
            .collect();
        for table in &tables {
            if table == "attentions" {
                continue;
            }
            let own: Vec<&(String, String, String)> =
                columns.iter().filter(|(t, _, _)| t == table).collect();
            let names: Vec<&str> = own.iter().map(|(_, c, _)| c.as_str()).collect();
            let values: Vec<String> = own
                .iter()
                .map(|(_, column, kind)| match column.as_str() {
                    "room" => "'old'".to_string(),
                    "appointed_by_role" => "'master'".to_string(),
                    _ if kind.to_ascii_uppercase().contains("INT") => "1".to_string(),
                    _ => "'x'".to_string(),
                })
                .collect();
            let sql = format!(
                "INSERT INTO {table} ({}) VALUES ({})",
                names.join(", "),
                values.join(", ")
            );
            conn.execute(&sql, [])
                .unwrap_or_else(|error| panic!("fixture row for {table}: {error}"));
        }
        conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();
        tables
    };
    assert!(tables.len() >= 27, "the schema lists its loca tables");

    assert!(store.rename_room("old", "new").expect("rename"));

    let conn = store.conn().expect("connection");
    let mut left_behind = Vec::new();
    for table in &tables {
        let count = |room: &str| -> i64 {
            conn.query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE room = ?1"),
                [room],
                |r| r.get(0),
            )
            .expect("count")
        };
        if count("old") != 0 || count("new") != 1 {
            left_behind.push(table.as_str());
        }
    }
    assert!(
        left_behind.is_empty(),
        "these tables did not follow the loca to its new name: {left_behind:?}"
    );
}

/// A direct message is never renamed, and no loca is renamed into the direct message id
/// space: the refusal changes nothing.
#[test]
fn a_rename_never_touches_a_dm_or_its_id_space() {
    let directory = tempfile::tempdir().expect("tempdir");
    let store = Store::open(directory.path().join("rename-dm.db").to_str()).expect("store");
    store
        .save_room("plain", &ChatMode::Free, &RoomSettings::default())
        .expect("room");
    {
        let conn = store.conn().expect("connection");
        conn.execute(
            "INSERT INTO birebirs (room, a, b, opened_by, opened_at) VALUES ('bb-pair', 'a', 'b', 'a', 1)",
            [],
        )
        .expect("dm");
        conn.execute(
            "INSERT INTO journal (id, room, by, by_type, text, at) VALUES (1, 'bb-pair', 'a', 'user', 'x', 1)",
            [],
        )
        .expect("journal");
    }
    assert!(store.rename_room("bb-pair", "open-room").is_err());
    assert!(store.rename_room("plain", "bb-pair").is_err());
    assert!(store.rename_room("plain", "bb-anything").is_err());
    let conn = store.conn().expect("connection");
    let rooms: Vec<String> = conn
        .prepare("SELECT room FROM rooms ORDER BY room")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(rooms, ["plain"], "a refused rename changes nothing");
    let pair: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM birebirs WHERE room = 'bb-pair'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(pair, 1);
}

/// The care signal the ledger holds for `id`, read from its JSON.
fn ledger_signal(store: &Store, id: &str) -> Option<CareSignal> {
    let conn = store.conn().expect("connection");
    let json: Option<String> = conn
        .query_row("SELECT signal FROM attentions WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .ok();
    json.map(|json| serde_json::from_str(&json).expect("ledger signal"))
}

/// A relay delivered in Care about another loca (`room = iye`,
/// `source_room = old`). Renaming the SOURCE loca moves the ledger row, its
/// id, the JSON source and the outbox reference together; the delivery id
/// stays, so a late ACK still lands, and a resolved signal is not delivered
/// again. The same holds after the database is reopened.
#[test]
fn renaming_the_source_loca_keeps_a_rehomed_care_signal_whole() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("rename-care-source.db");
    let store = Store::open(path.to_str()).expect("store");
    let signal = care_signal_from("delivery-src", "attention:old:goal:1:1000", "iye", "old");
    store.enqueue_care("iye", &signal).expect("enqueue");

    assert!(store.rename_room("old", "new").expect("rename"));

    let check = |store: &Store, stage: &str| {
        assert!(
            store.attention("attention:old:goal:1:1000").is_none(),
            "{stage}: the old attention id is gone"
        );
        let attention = store
            .attention("attention:new:goal:1:1000")
            .unwrap_or_else(|| panic!("{stage}: the attention follows its loca"));
        assert_eq!(attention.room, "new", "{stage}: filed under the new origin");
        let ledger = ledger_signal(store, "attention:new:goal:1:1000").expect("ledger JSON");
        assert_eq!(
            (
                ledger.room.as_str(),
                ledger.origin_room(),
                ledger.attention_id.as_str()
            ),
            ("iye", "new", "attention:new:goal:1:1000"),
            "{stage}: ledger JSON keeps the delivery room and names the new origin"
        );
        assert!(
            store.attentions("old").is_empty(),
            "{stage}: nothing is listed under the old loca"
        );
    };
    check(&store, "after rename");
    let pending = store.pending_care("iye", "loca-care");
    assert_eq!(pending.len(), 1, "still one delivery waiting in Care");
    assert_eq!(
        (
            pending[0].id.as_str(),
            pending[0].attention_id.as_str(),
            pending[0].room.as_str(),
            pending[0].origin_room()
        ),
        ("delivery-src", "attention:new:goal:1:1000", "iye", "new"),
        "the outbox row points at the ledger row it belongs to"
    );

    drop(store);
    let store = Store::open(path.to_str()).expect("reopen");
    check(&store, "after reopen");
    assert_eq!(store.pending_care("iye", "loca-care").len(), 1);
    assert!(
        store
            .ack_care_scoped("delivery-src", "loca-care", None, 2_000)
            .expect("late ACK"),
        "the delivery id is unchanged, so a late ACK lands"
    );
    assert!(
        store
            .resolve_attention("attention:new:goal:1:1000", 3_000)
            .expect("resolve"),
        "the attention resolves under its new id"
    );
    drop(store);
    let store = Store::open(path.to_str()).expect("reopen again");
    assert!(
        store.pending_care("iye", "loca-care").is_empty(),
        "a resolved signal is not delivered again"
    );
    assert!(store
        .attention("attention:new:goal:1:1000")
        .expect("attention")
        .resolved_at
        .is_some());
}

/// The other direction: the loca where the relay is DELIVERED is renamed
/// (`room = hall`, `source_room = proj`). The attention keeps its id and its
/// origin; ledger JSON and outbox both name the new delivery room.
#[test]
fn renaming_the_delivery_loca_keeps_a_rehomed_care_signal_whole() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("rename-care-delivery.db");
    let store = Store::open(path.to_str()).expect("store");
    store
        .save_room("hall", &ChatMode::Free, &RoomSettings::default())
        .expect("delivery loca");
    let signal = care_signal_from("delivery-dst", "attention:proj:goal:1:1000", "hall", "proj");
    store.enqueue_care("hall", &signal).expect("enqueue");

    assert!(store.rename_room("hall", "desk").expect("rename"));

    let check = |store: &Store, stage: &str| {
        let attention = store
            .attention("attention:proj:goal:1:1000")
            .unwrap_or_else(|| panic!("{stage}: the attention keeps its id"));
        assert_eq!(
            attention.room, "proj",
            "{stage}: still filed under its origin"
        );
        let ledger = ledger_signal(store, "attention:proj:goal:1:1000").expect("ledger JSON");
        assert_eq!(
            (
                ledger.room.as_str(),
                ledger.origin_room(),
                ledger.attention_id.as_str()
            ),
            ("desk", "proj", "attention:proj:goal:1:1000"),
            "{stage}: ledger JSON names the new delivery room"
        );
        assert!(
            store.pending_care("hall", "loca-care").is_empty(),
            "{stage}: nothing waits under the old delivery room"
        );
    };
    check(&store, "after rename");
    let pending = store.pending_care("desk", "loca-care");
    assert_eq!(pending.len(), 1, "the delivery waits in the renamed loca");
    assert_eq!(
        (
            pending[0].id.as_str(),
            pending[0].attention_id.as_str(),
            pending[0].room.as_str(),
            pending[0].origin_room()
        ),
        ("delivery-dst", "attention:proj:goal:1:1000", "desk", "proj")
    );

    drop(store);
    let store = Store::open(path.to_str()).expect("reopen");
    check(&store, "after reopen");
    assert!(store
        .ack_care_scoped("delivery-dst", "loca-care", None, 2_000)
        .expect("late ACK"));
    assert!(store
        .resolve_attention("attention:proj:goal:1:1000", 3_000)
        .expect("resolve"));
    assert!(
        store.pending_care("desk", "loca-care").is_empty(),
        "a resolved signal is not delivered again"
    );
}

fn schema_dump(path: &str) -> Vec<(String, String, Option<String>)> {
    let conn = Connection::open(path).expect("inspect");
    let mut stmt = conn
        .prepare("SELECT type, name, sql FROM sqlite_master ORDER BY type, name")
        .expect("prepare");
    let rows = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("query");
    rows.collect::<rusqlite::Result<Vec<_>>>().expect("rows")
}

#[test]
fn reopening_an_upgraded_database_is_a_schema_no_op() {
    // Every column the upgrade adds already exists on the second open, so each
    // ALTER answers "duplicate column name". That answer, and only that one,
    // is success: the store opens and the schema is byte-for-byte unchanged.
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("reopen.db");
    let p = path.to_str().expect("path");
    drop(Store::open(Some(p)).expect("first open"));
    let first = schema_dump(p);
    assert!(
        first.iter().any(|(_, name, sql)| name == "rooms"
            && sql.as_deref().is_some_and(|sql| sql.contains("sealed_at"))),
        "the first open added the upgrade columns"
    );
    drop(Store::open(Some(p)).expect("second open"));
    assert_eq!(first, schema_dump(p));
}

#[test]
fn a_failed_column_upgrade_fails_open_instead_of_starting_half_upgraded() {
    // `rooms` exists as a VIEW, so `CREATE TABLE IF NOT EXISTS rooms` is a
    // no-op and adding `sealed_at` fails for a reason that is NOT "duplicate
    // column". The store must refuse to open with SQLite's own error rather
    // than start without the column.
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("half-upgraded.db");
    let p = path.to_str().expect("path");
    Connection::open(p)
        .expect("create")
        .execute_batch("CREATE VIEW rooms AS SELECT 'r' AS room, '{}' AS mode, '{}' AS settings;")
        .expect("view");
    let error = match Store::open(Some(p)) {
        Ok(_) => panic!("open must fail when a column cannot be added"),
        Err(error) => error.to_string(),
    };
    assert!(
        error.contains("Cannot add a column to a view"),
        "the operator sees the real cause, got: {error}"
    );
}

/// Sending answers with who it went to. Those counts are taken in the
/// transaction that stores the notification, so there is nothing left to fail
/// once it is committed, and a failure before that leaves nothing stored.
#[test]
fn sending_a_notification_counts_its_recipients_in_the_same_transaction() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("notification-counts.db");
    let store = Store::open(path.to_str()).unwrap();
    let mut ids = Vec::new();
    for (name, kind) in [("ali", "user"), ("bot", "agent"), ("scribe", "agent")] {
        let key = format!("{name}-fixture");
        store.add_member(&key, name, kind, 1, "master").unwrap();
        ids.push(store.principal_for_credential(&key).unwrap().id);
    }

    let unlimited = super::SendLimit {
        senders: &[],
        max: u32::MAX,
        window_ms: 1,
    };
    let created = match store
        .create_notification(
            "pr_sender",
            "Master",
            r#"{"all":true}"#,
            "Kept",
            "",
            10,
            &ids,
            unlimited,
        )
        .unwrap()
        .unwrap()
    {
        super::NotificationSend::Created(created) => created,
        refused => panic!("the first send was refused: {refused:?}"),
    };
    assert_eq!((created.recipients, created.agents), (3, 2));
    // The Sent list reads the same numbers later.
    let sent = store.sent_notifications("pr_sender", None, 10).unwrap();
    assert_eq!(
        (sent[0].id, sent[0].recipients, sent[0].agents, sent[0].read),
        (created.id, 3, 2, 0)
    );

    // A receipt that cannot be written: the send fails and stores nothing.
    store
        .conn()
        .unwrap()
        .execute_batch(
            "CREATE TEMP TRIGGER reject_receipt BEFORE INSERT ON notification_receipts
             BEGIN SELECT RAISE(ABORT, 'fixture failure'); END;",
        )
        .unwrap();
    assert!(store
        .create_notification(
            "pr_sender",
            "Master",
            r#"{"all":true}"#,
            "Lost",
            "",
            11,
            &ids,
            unlimited,
        )
        .is_err());
    let sent = store.sent_notifications("pr_sender", None, 10).unwrap();
    assert_eq!(
        sent.iter().map(|n| n.title.as_str()).collect::<Vec<_>>(),
        vec!["Kept"],
        "a failed send leaves no notification behind"
    );
}

// ---------------------------------------------------------------------------
// The Smaster registry stops holding codes. These build a registry the way
// v0.13.0 left it on disk (the code as the primary key, its identity bound in
// `principals`/`credentials`, a WAL not yet checkpointed) and open it here.
// ---------------------------------------------------------------------------

/// One Smaster as v0.13.0 recorded it.
struct LegacySmaster {
    code: String,
    issued_as: &'static str,
    renamed_to: Option<&'static str>,
    issued_at: u64,
    revoked_at: Option<u64>,
    last_used_at: Option<u64>,
    device_secret: Option<&'static str>,
}

impl LegacySmaster {
    fn name(&self) -> &'static str {
        self.renamed_to.unwrap_or(self.issued_as)
    }
    fn derived(&self, prefix: &str) -> String {
        super::hashed_id(prefix, &format!("smaster:{}", self.code))
    }
    /// The part of the code the ledger never shows: everything after the
    /// fingerprint's four characters.
    fn secret_tail(&self) -> &str {
        &self.code[7..]
    }
}

/// A synthetic code shaped like v0.13.0's (`sm_` and 64 characters) around a
/// marker nothing else in the file contains, so a byte search is meaningful.
fn synthetic_smaster_code(tag: &str) -> String {
    let mut code = format!("sm_QZXJ-synthetic-smaster-code-{tag}-");
    while code.len() < 67 {
        code.push('7');
    }
    code
}

/// A renamed one, a revoked one, one with an extra device credential, and one
/// that was never used.
fn legacy_smasters() -> Vec<LegacySmaster> {
    vec![
        LegacySmaster {
            code: synthetic_smaster_code("renamed"),
            issued_as: "yeni-0001",
            renamed_to: Some("ayse"),
            issued_at: 1_000,
            revoked_at: None,
            last_used_at: Some(1_500),
            device_secret: None,
        },
        LegacySmaster {
            code: synthetic_smaster_code("revoked"),
            issued_as: "bora",
            renamed_to: None,
            issued_at: 2_000,
            revoked_at: Some(2_500),
            last_used_at: Some(2_100),
            device_secret: None,
        },
        LegacySmaster {
            code: synthetic_smaster_code("device"),
            issued_as: "cem",
            renamed_to: None,
            issued_at: 3_000,
            revoked_at: None,
            last_used_at: Some(3_300),
            device_secret: Some("ak_synthetic-device-credential-for-cem"),
        },
        LegacySmaster {
            code: synthetic_smaster_code("unused"),
            issued_as: "deniz",
            renamed_to: None,
            issued_at: 4_000,
            revoked_at: None,
            last_used_at: None,
            device_secret: None,
        },
    ]
}

/// Every row of `principals` and `credentials`, as text, in a stable order.
fn identity_rows(conn: &Connection) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    for sql in [
        "SELECT id, display_name, kind, building_role, created_at, disabled_at
         FROM principals ORDER BY id",
        "SELECT id, principal_id, label, secret_hash, created_at, last_used_at, revoked_at,
                legacy_source
         FROM credentials ORDER BY id",
    ] {
        let mut statement = conn.prepare(sql).unwrap();
        let columns = statement.column_count();
        let rows = statement
            .query_map([], |row| {
                (0..columns)
                    .map(|i| Ok(format!("{:?}", row.get_ref(i)?)))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap();
        out.extend(rows.map(Result::unwrap));
    }
    out
}

/// How v0.13.0's files look when the upgrade opens them.
#[derive(Clone, Copy)]
enum LegacyLayout {
    /// A stopped server's database plus the WAL it had not checkpointed.
    PendingWal,
    /// Everything checkpointed into the main file, after rows that once held
    /// a code were deleted: pages and cells freed before the upgrade.
    CheckpointedWithFreedPages,
}

/// Write the registry the way v0.13.0 did and leave the files at
/// `legacy.db` in `layout`. `bound` false leaves the Smasters' principals
/// and credentials unbound, as a database from before identity v2. Returns
/// the path and the identity rows as v0.13.0 left them.
fn legacy_smaster_database(
    dir: &std::path::Path,
    smasters: &[LegacySmaster],
    bound: bool,
) -> (std::path::PathBuf, Vec<Vec<String>>) {
    legacy_smaster_files(dir, smasters, bound, LegacyLayout::PendingWal)
}

fn legacy_smaster_files(
    dir: &std::path::Path,
    smasters: &[LegacySmaster],
    bound: bool,
    layout: LegacyLayout,
) -> (std::path::PathBuf, Vec<Vec<String>>) {
    use rusqlite::params;
    let build = dir.join("build.db");
    drop(Store::open(build.to_str()).expect("current schema"));
    let conn = legacy_build_connection(&build);
    write_legacy_smasters(&conn, smasters, bound);
    let identities = identity_rows(&conn);
    if let LegacyLayout::CheckpointedWithFreedPages = layout {
        for (i, s) in smasters.iter().enumerate().cycle().take(400) {
            conn.execute(
                "INSERT OR REPLACE INTO smasters (token, name, issued_at, revoked_at)
                 VALUES (?1, 'churn', 1, NULL)",
                params![format!("{}-copy-{i}", s.code)],
            )
            .unwrap();
        }
        conn.execute("DELETE FROM smasters WHERE name = 'churn'", [])
            .unwrap();
    }
    (finish_legacy_files(dir, &build, conn, layout), identities)
}

/// A connection to the fixture being built that keeps every write in the WAL
/// until `finish_legacy_files` decides what the old version left on disk.
fn legacy_build_connection(build: &std::path::Path) -> Connection {
    let conn = Connection::open(build).expect("build");
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA wal_autocheckpoint = 0;",
    )
    .expect("WAL kept");
    conn
}

/// Leave the fixture at `legacy.db` in `layout`: with its WAL pending, or
/// checkpointed into the main file after the caller's churn freed pages.
fn finish_legacy_files(
    dir: &std::path::Path,
    build: &std::path::Path,
    conn: Connection,
    layout: LegacyLayout,
) -> std::path::PathBuf {
    if let LegacyLayout::CheckpointedWithFreedPages = layout {
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
            .unwrap();
    }
    let legacy = dir.join("legacy.db");
    std::fs::copy(build, &legacy).expect("database file");
    if let LegacyLayout::PendingWal = layout {
        std::fs::copy(dir.join("build.db-wal"), dir.join("legacy.db-wal")).expect("its WAL");
    }
    drop(conn);
    legacy
}

/// The registry v0.13.0 wrote, with the bindings it made when `bound`.
fn write_legacy_smasters(conn: &Connection, smasters: &[LegacySmaster], bound: bool) {
    use rusqlite::params;
    conn.execute_batch(
        "DROP TABLE smasters;
         CREATE TABLE smasters (
             token TEXT PRIMARY KEY,
             name TEXT NOT NULL,
             issued_at INTEGER NOT NULL,
             revoked_at INTEGER
         );",
    )
    .expect("v0.13.0 registry");
    for s in smasters {
        let hash = super::secret_hash(&s.code);
        conn.execute(
            "INSERT OR REPLACE INTO smasters (token, name, issued_at, revoked_at)
             VALUES (?1, ?2, ?3, NULL)",
            params![s.code, s.issued_as, s.issued_at],
        )
        .unwrap();
        if bound {
            conn.execute(
                "INSERT INTO principals (id, display_name, kind, building_role, created_at)
                 VALUES (?1, ?2, 'human', 'smaster', ?3)",
                params![s.derived("pr_"), s.issued_as, s.issued_at],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO credentials
                 (id, principal_id, label, secret_hash, created_at, last_used_at, revoked_at,
                  legacy_source)
                 VALUES (?1, ?2, 'Smaster access', ?3, ?4, ?5, NULL, ?6)",
                params![
                    s.derived("cr_"),
                    s.derived("pr_"),
                    hash,
                    s.issued_at,
                    s.last_used_at,
                    format!("smaster:{hash}")
                ],
            )
            .unwrap();
            if let Some(device) = s.device_secret {
                conn.execute(
                    "INSERT INTO credentials
                     (id, principal_id, label, secret_hash, created_at, last_used_at)
                     VALUES (?1, ?2, 'Laptop', ?3, ?4, ?5)",
                    params![
                        super::hashed_id("cr_", &format!("credential:{device}")),
                        s.derived("pr_"),
                        super::secret_hash(device),
                        s.issued_at + 10,
                        s.issued_at + 50
                    ],
                )
                .unwrap();
            }
        }
        if let Some(name) = s.renamed_to {
            conn.execute(
                "UPDATE smasters SET name = ?2 WHERE token = ?1 AND revoked_at IS NULL",
                params![s.code, name],
            )
            .unwrap();
            if bound {
                conn.execute(
                    "UPDATE principals SET display_name = ?2 WHERE id = ?1",
                    params![s.derived("pr_"), name],
                )
                .unwrap();
            }
        }
        if let Some(at) = s.revoked_at {
            conn.execute(
                "UPDATE smasters SET revoked_at = ?2 WHERE token = ?1 AND revoked_at IS NULL",
                params![s.code, at],
            )
            .unwrap();
            if bound {
                conn.execute(
                    "UPDATE credentials SET revoked_at = ?2 WHERE secret_hash = ?1",
                    params![hash, at],
                )
                .unwrap();
                conn.execute(
                    "UPDATE principals SET disabled_at = ?2 WHERE id = ?1",
                    params![s.derived("pr_"), at],
                )
                .unwrap();
            }
        }
    }
}

/// Every value in every table of `conn` that contains `needle`, as
/// `table.column`.
fn sql_values_containing(conn: &Connection, needle: &str) -> Vec<String> {
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut found = Vec::new();
    for table in tables {
        let mut statement = conn.prepare(&format!("SELECT * FROM \"{table}\"")).unwrap();
        let names: Vec<String> = statement.column_names().map(str::to_string).collect();
        let mut rows = statement.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            for (i, column) in names.iter().enumerate() {
                let bytes = match row.get_ref(i).unwrap() {
                    rusqlite::types::ValueRef::Text(bytes) => bytes.to_vec(),
                    rusqlite::types::ValueRef::Blob(bytes) => bytes.to_vec(),
                    other => format!("{other:?}").into_bytes(),
                };
                if contains_bytes(&bytes, needle.as_bytes()) {
                    found.push(format!("{table}.{column}"));
                }
            }
        }
    }
    found
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// The files of the database at `path` (itself, `-wal`, `-shm`) that contain
/// `needle` byte for byte.
fn files_containing(path: &std::path::Path, needle: &str) -> Vec<String> {
    ["", "-wal", "-shm"]
        .into_iter()
        .filter_map(|suffix| {
            let file = std::path::PathBuf::from(format!("{}{suffix}", path.display()));
            let bytes = std::fs::read(&file).ok()?;
            contains_bytes(&bytes, needle.as_bytes())
                .then(|| file.file_name().unwrap().to_string_lossy().into_owned())
        })
        .collect()
}

fn hub_on(store: Store) -> crate::hub::Hub {
    crate::hub::Hub::build(
        crate::hub::HubConfig {
            admin_token: "smaster-migration-root".into(),
            room_token: String::new(),
            require_sessions: true,
            require_invite: true,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: std::collections::HashSet::new(),
        },
        std::sync::Arc::new(store),
        RoomSettings::default(),
        1,
    )
}

#[test]
fn smaster_code_migration_keeps_every_identity_and_sign_in() {
    let directory = tempfile::tempdir().unwrap();
    let smasters = legacy_smasters();
    let (path, before) = legacy_smaster_database(directory.path(), &smasters, true);
    let store = Store::open(path.to_str()).expect("the upgrade opens a v0.13.0 database");
    {
        let c = store.conn().unwrap();
        // No principal or credential was added, changed or lost: the same
        // ids, names, revocations and last uses as v0.13.0 left them.
        assert_eq!(identity_rows(&c), before);
        for s in &smasters {
            let row: (String, String, String, String, String, u64, Option<u64>) = c
                .query_row(
                    "SELECT management_id, principal_id, credential_id, fingerprint, name,
                            issued_at, revoked_at
                     FROM smasters WHERE secret_hash = ?1",
                    [super::secret_hash(&s.code)],
                    |r| {
                        Ok((
                            r.get(0)?,
                            r.get(1)?,
                            r.get(2)?,
                            r.get(3)?,
                            r.get(4)?,
                            r.get(5)?,
                            r.get(6)?,
                        ))
                    },
                )
                .expect("the registry keeps each Smaster under its code's hash");
            assert_eq!(
                row,
                (
                    s.derived("smid_"),
                    s.derived("pr_"),
                    s.derived("cr_"),
                    super::keys::fingerprint(&s.code),
                    s.name().to_string(),
                    s.issued_at,
                    s.revoked_at
                ),
                "identifiers are exactly what v0.13.0 derived from {}",
                s.name()
            );
        }
    }
    // The ledger shows the same ids, fingerprints, uses and revocations.
    let ledger = store.key_ledger(10_000, 7 * 24 * 60 * 60 * 1_000).unwrap();
    for s in &smasters {
        let entry = ledger
            .iter()
            .find(|k| k.kind == "smaster" && k.id == s.derived("smid_"))
            .expect("listed by its smid_");
        assert_eq!(entry.fingerprint, super::keys::fingerprint(&s.code));
        assert_eq!(entry.for_name.as_deref(), Some(s.name()));
        assert_eq!(entry.issued_at, s.issued_at);
        assert_eq!(entry.last_used_at, s.last_used_at);
        assert_eq!(entry.revoked_at, s.revoked_at);
    }
    // The Hub's legacy path knows each live code by its hash, with its name.
    let hub = hub_on(store);
    for s in &smasters {
        assert_eq!(
            hub.smaster_name(Some(&s.code)).as_deref(),
            s.revoked_at.is_none().then(|| s.name())
        );
    }
    // The seven-day sweep (these fixture codes were issued in 1970) takes
    // back only the code that was never used, judged by the last use v0.13.0
    // recorded on its credential. Nothing has signed in since the upgrade.
    hub.expire_unused_smasters();
    let live = |s: &LegacySmaster| s.revoked_at.is_none() && s.last_used_at.is_some();
    for s in &smasters {
        assert_eq!(
            hub.smaster_name(Some(&s.code)).is_some(),
            live(s),
            "{} after the sweep",
            s.name()
        );
    }
    // The same codes sign in as the same principals; the revoked and the
    // expired one do not, and the device credential still belongs to its
    // Smaster.
    for s in &smasters {
        let authority = hub.resolve_authority(Some(&s.code), None);
        if live(s) {
            assert_eq!(authority.principal_id, Some(s.derived("pr_")));
            assert_eq!(authority.display_name.as_deref(), Some(s.name()));
            assert_eq!(authority.building_role, Some(super::BuildingRole::Smaster));
        } else {
            assert!(!authority.is_building_admin(), "{} signs in", s.name());
        }
        if let Some(device) = s.device_secret {
            assert_eq!(
                hub.resolve_authority(Some(device), None).principal_id,
                Some(s.derived("pr_"))
            );
        }
    }
}

#[test]
fn smaster_codes_leave_no_trace_in_the_database_files_or_a_snapshot() {
    for layout in [
        LegacyLayout::PendingWal,
        LegacyLayout::CheckpointedWithFreedPages,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let smasters = legacy_smasters();
        let (path, _) = legacy_smaster_files(directory.path(), &smasters, true, layout);
        // The fixture is meaningful: before the upgrade the codes are on disk.
        assert!(smasters
            .iter()
            .all(|s| !files_containing(&path, s.secret_tail()).is_empty()));

        let store = Store::open(path.to_str()).expect("upgrade");
        for s in &smasters {
            for needle in [s.code.as_str(), s.secret_tail()] {
                assert_eq!(
                    sql_values_containing(&store.conn().unwrap(), needle),
                    Vec::<String>::new(),
                    "no column of any table holds {}'s code",
                    s.name()
                );
                assert_eq!(
                    files_containing(&path, needle),
                    Vec::<String>::new(),
                    "no byte of the open database, its WAL or shm holds {}'s code",
                    s.name()
                );
            }
        }
        drop(store);
        for s in &smasters {
            assert_eq!(
                files_containing(&path, s.secret_tail()),
                Vec::<String>::new()
            );
        }

        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../scripts/storage_snapshot.py");
        let snapshot = directory.path().join("snapshot");
        let python = std::env::var("PYTHON").unwrap_or_else(|_| "python3".into());
        let backup = std::process::Command::new(&python)
            .arg(&script)
            .arg("backup")
            .arg(&path)
            .arg(&snapshot)
            .arg("--server-stopped")
            .output()
            .expect("python3 runs the snapshot tool");
        assert!(
            backup.status.success(),
            "{}",
            String::from_utf8_lossy(&backup.stderr)
        );
        for s in &smasters {
            assert_eq!(
                files_containing(&snapshot.join("loca.sqlite3"), s.secret_tail()),
                Vec::<String>::new(),
                "a snapshot taken after the upgrade holds no code"
            );
        }
    }
}

#[test]
fn reopening_a_migrated_smaster_registry_changes_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let smasters = legacy_smasters();
    // Unbound: the first open binds each Smaster, the second must not add or
    // change anything (no principal minted from a hash or a placeholder).
    let (path, _) = legacy_smaster_database(directory.path(), &smasters, false);
    let dump = |store: &Store| {
        let c = store.conn().unwrap();
        let mut rows = identity_rows(&c);
        let mut statement = c
            .prepare(
                "SELECT management_id, secret_hash, principal_id, credential_id, fingerprint,
                        name, issued_at, revoked_at
                 FROM smasters ORDER BY management_id",
            )
            .expect("the registry has its hashed shape");
        let registry = statement
            .query_map([], |row| {
                (0..8)
                    .map(|i| Ok(format!("{:?}", row.get_ref(i)?)))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap()
            .map(Result::unwrap);
        rows.extend(registry);
        rows
    };
    let first = Store::open(path.to_str()).expect("first open");
    let after_first = dump(&first);
    let principals = |rows: &[Vec<String>]| rows.iter().filter(|r| r.len() == 6).count();
    assert_eq!(
        principals(&after_first),
        smasters.len(),
        "one principal per Smaster, nothing else"
    );
    for s in &smasters {
        assert_eq!(
            first.principal_for_credential(&s.code).map(|who| who.id),
            s.revoked_at.is_none().then(|| s.derived("pr_"))
        );
    }
    let after_sign_in = dump(&first);
    drop(first);
    let schema = schema_dump(path.to_str().unwrap());
    let second = Store::open(path.to_str()).expect("second open");
    assert_eq!(dump(&second), after_sign_in, "the second open is a no-op");
    drop(second);
    assert_eq!(schema_dump(path.to_str().unwrap()), schema);
}

#[test]
fn a_failed_smaster_migration_leaves_the_old_registry_and_fails_open() {
    let read_old = |path: &std::path::Path| -> Vec<(String, String, u64, Option<u64>)> {
        let c = Connection::open(path).unwrap();
        let mut statement = c
            .prepare("SELECT token, name, issued_at, revoked_at FROM smasters ORDER BY token")
            .expect("the old registry is still readable in its old shape");
        let rows = statement
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        rows
    };
    let smasters = legacy_smasters();
    let mut expected: Vec<_> = smasters
        .iter()
        .map(|s| {
            (
                s.code.clone(),
                s.name().to_string(),
                s.issued_at,
                s.revoked_at,
            )
        })
        .collect();
    expected.sort();

    // 1. Binding a Smaster's credential is refused (a trigger, as the other
    //    fault-injection tests do).
    let directory = tempfile::tempdir().unwrap();
    let (path, _) = legacy_smaster_database(directory.path(), &smasters, false);
    Connection::open(&path)
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER refuse_smaster_binding BEFORE INSERT ON credentials
             WHEN NEW.legacy_source LIKE 'smaster:%'
             BEGIN SELECT RAISE(ABORT, 'injected: Smaster binding refused'); END;",
        )
        .unwrap();
    let error = match Store::open(path.to_str()) {
        Ok(_) => panic!("open must fail when the migration fails"),
        Err(error) => error.to_string(),
    };
    assert!(
        error.contains("injected: Smaster binding refused"),
        "{error}"
    );
    assert_eq!(read_old(&path), expected);

    // 2. The rebuild itself fails after every binding was written: an
    //    operator's view over the old column cannot follow the new table.
    //    Nothing of the transaction stays, the bindings included.
    let directory = tempfile::tempdir().unwrap();
    let (path, _) = legacy_smaster_database(directory.path(), &smasters, false);
    Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE VIEW smaster_codes AS SELECT token FROM smasters;")
        .unwrap();
    let error = match Store::open(path.to_str()) {
        Ok(_) => panic!("open must fail when the rebuild fails"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("smaster_codes"), "the real cause: {error}");
    assert_eq!(read_old(&path), expected);
    let c = Connection::open(&path).unwrap();
    let bound: i64 = c
        .query_row(
            "SELECT count(*) FROM principals WHERE building_role = 'smaster'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(bound, 0, "the bindings rolled back with the rebuild");
    let leftovers: i64 = c
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name = 'smasters_hashed'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(leftovers, 0);
}

#[test]
fn a_busy_checkpoint_is_not_a_finished_scrub_and_the_next_open_finishes_it() {
    let directory = tempfile::tempdir().unwrap();
    let smasters = legacy_smasters();
    let (path, _) = legacy_smaster_database(directory.path(), &smasters, true);
    // Another process reading the file pins the WAL: the migration commits,
    // but its old frames cannot be cut away while that read is open.
    let reader = Connection::open(&path).unwrap();
    reader
        .execute_batch("BEGIN; SELECT count(*) FROM smasters;")
        .unwrap();
    let error = match Store::open(path.to_str()) {
        Ok(_) => panic!("a busy checkpoint must not count as a removed residue"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("checkpoint busy"), "{error}");
    reader.execute_batch("COMMIT").unwrap();
    drop(reader);
    // The chore was recorded with the migration, so the next open runs it
    // although there is nothing left to migrate.
    let store = Store::open(path.to_str()).expect("the next open finishes the scrub");
    let pending: i64 = store
        .conn()
        .unwrap()
        .query_row("SELECT count(*) FROM storage_maintenance", [], |r| r.get(0))
        .unwrap();
    assert_eq!(pending, 0);
    for s in &smasters {
        assert_eq!(
            files_containing(&path, s.secret_tail()),
            Vec::<String>::new()
        );
    }
}

// ---------------------------------------------------------------------------
// Persisted admin sessions stop holding their tokens. These write
// `admin_sessions` the way v0.13.0 and the build before this change left it
// (the session token as the primary key, beside the `principal_sessions` row
// the same sign-in wrote under the token's hash) and open it here.
// ---------------------------------------------------------------------------

/// The root key `hub_on` runs with.
const SESSION_FIXTURE_ROOT: &str = "smaster-migration-root";
/// The Smaster whose session the fixture keeps when it has no v0.13.0
/// registry of its own.
const SESSION_FIXTURE_SMASTER: &str = "sm_QZXJ-synthetic-session-fixture-smaster";

/// A session token shaped like the server's (`st_` and 64 characters) around
/// a marker nothing else in the file contains, so a byte search is
/// meaningful.
fn synthetic_session_token(tag: &str) -> String {
    let mut token = format!("st_QZXJ-synthetic-admin-session-{tag}-");
    while token.len() < 67 {
        token.push('7');
    }
    token
}

/// One persisted admin session as the previous version left it.
struct LegacySession {
    token: String,
    name: &'static str,
    principal_id: String,
    credential_id: String,
    role: super::BuildingRole,
    expires_at: u64,
}

impl LegacySession {
    fn live(&self) -> bool {
        self.expires_at > super::current_time_ms()
    }
    /// Everything after the `st_QZXJ` prefix: what must not survive anywhere.
    fn secret_tail(&self) -> &str {
        &self.token[7..]
    }
}

/// Build a database whose admin sessions are stored the old way, at
/// `legacy.db` in `layout`: a live Master session, a live Smaster session and
/// an expired Master session. With `smasters`, the Smaster registry is
/// v0.13.0's too (a database that needs both migrations) and the Smaster
/// session belongs to the first live one of them.
fn legacy_session_files(
    dir: &std::path::Path,
    smasters: &[LegacySmaster],
    layout: LegacyLayout,
) -> (std::path::PathBuf, Vec<LegacySession>) {
    use rusqlite::params;
    let build = dir.join("build.db");
    {
        let store = Store::open(build.to_str()).expect("current schema");
        if smasters.is_empty() {
            store
                .add_smaster(SESSION_FIXTURE_SMASTER, "selin", 1)
                .unwrap();
        }
        // Starting a Hub binds the Master principal to the root key.
        drop(hub_on(store));
    }
    let (master_id, master_credential) = {
        let store = Store::open(build.to_str()).unwrap();
        (
            store
                .principal_for_credential(SESSION_FIXTURE_ROOT)
                .expect("Master principal")
                .id,
            store
                .credential_id_for_secret(SESSION_FIXTURE_ROOT)
                .expect("Master credential"),
        )
    };
    let (smaster_id, smaster_credential, smaster_name) =
        match smasters.iter().find(|s| s.revoked_at.is_none()) {
            Some(s) => (s.derived("pr_"), s.derived("cr_"), s.name()),
            None => {
                let store = Store::open(build.to_str()).unwrap();
                (
                    store
                        .principal_for_credential(SESSION_FIXTURE_SMASTER)
                        .expect("Smaster principal")
                        .id,
                    store
                        .credential_id_for_secret(SESSION_FIXTURE_SMASTER)
                        .expect("Smaster credential"),
                    "selin",
                )
            }
        };
    let now = super::current_time_ms();
    let week = 7 * 24 * 60 * 60 * 1_000;
    let sessions = vec![
        LegacySession {
            token: synthetic_session_token("master"),
            name: "Master",
            principal_id: master_id.clone(),
            credential_id: master_credential.clone(),
            role: super::BuildingRole::Master,
            expires_at: now + week,
        },
        LegacySession {
            token: synthetic_session_token("smaster"),
            name: smaster_name,
            principal_id: smaster_id,
            credential_id: smaster_credential,
            role: super::BuildingRole::Smaster,
            expires_at: now + week - 60_000,
        },
        LegacySession {
            token: synthetic_session_token("expired"),
            name: "Master",
            principal_id: master_id,
            credential_id: master_credential,
            role: super::BuildingRole::Master,
            expires_at: now - 60_000,
        },
    ];
    let conn = legacy_build_connection(&build);
    if !smasters.is_empty() {
        write_legacy_smasters(&conn, smasters, true);
    }
    conn.execute_batch(
        "DROP TABLE admin_sessions;
         CREATE TABLE admin_sessions (
             token TEXT PRIMARY KEY,
             name TEXT NOT NULL,
             kind TEXT NOT NULL,
             expires_at INTEGER NOT NULL
         );",
    )
    .expect("the previous admin_sessions table");
    for s in &sessions {
        // What a sign-in wrote: the principal session under the token's hash
        // (`save_principal_session`) and the admin session under the token.
        conn.execute(
            "INSERT OR REPLACE INTO principal_sessions
             (id, principal_id, credential_id, created_at, expires_at, revoked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
            params![
                super::secret_hash(&s.token),
                s.principal_id,
                s.credential_id,
                now - 120_000,
                s.expires_at
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO admin_sessions (token, name, kind, expires_at)
             VALUES (?1, ?2, 'user', ?3)",
            params![s.token, s.name, s.expires_at],
        )
        .unwrap();
    }
    if let LegacyLayout::CheckpointedWithFreedPages = layout {
        // Sessions that came and went before the upgrade: their rows were
        // deleted, their cells and pages freed, and everything checkpointed.
        for (i, s) in sessions.iter().enumerate().cycle().take(400) {
            conn.execute(
                "INSERT OR REPLACE INTO admin_sessions (token, name, kind, expires_at)
                 VALUES (?1, 'churn', 'user', 1)",
                params![format!("{}-copy-{i}", s.token)],
            )
            .unwrap();
        }
        conn.execute("DELETE FROM admin_sessions WHERE name = 'churn'", [])
            .unwrap();
    }
    (finish_legacy_files(dir, &build, conn, layout), sessions)
}

/// `admin_sessions` as stored, keyed by whatever its first column is, in a
/// stable order. Fails on a table that is not in the hashed shape.
fn admin_session_rows(conn: &Connection) -> Vec<(String, String, String, u64)> {
    let mut statement = conn
        .prepare(
            "SELECT secret_hash, name, kind, expires_at FROM admin_sessions
             ORDER BY secret_hash",
        )
        .expect("admin_sessions is keyed by the token's hash");
    let rows = statement
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    rows
}

#[test]
fn admin_session_migration_keeps_everyone_signed_in() {
    let directory = tempfile::tempdir().unwrap();
    let (path, sessions) = legacy_session_files(directory.path(), &[], LegacyLayout::PendingWal);
    let store = Store::open(path.to_str()).expect("the upgrade opens the database");
    {
        let c = store.conn().unwrap();
        assert!(
            !super::table_has_column(&c, "admin_sessions", "token").unwrap(),
            "the token is no longer a column"
        );
        // Every live row carried over under its token's hash, with its name,
        // kind and expiry; expired ones go at the open (the session sweep).
        let mut expected: Vec<_> = sessions
            .iter()
            .filter(|s| s.live())
            .map(|s| {
                (
                    super::secret_hash(&s.token),
                    s.name.to_string(),
                    "user".to_string(),
                    s.expires_at,
                )
            })
            .collect();
        expected.sort();
        assert_eq!(admin_session_rows(&c), expected);
    }
    let hub = hub_on(store);
    for s in &sessions {
        let identity = hub.session_identity(Some(&s.token));
        let authority = hub.resolve_authority(None, Some(&s.token));
        if s.live() {
            let identity = identity.expect("a live session survives the upgrade");
            assert_eq!(
                identity.principal_id.as_deref(),
                Some(s.principal_id.as_str())
            );
            assert_eq!(identity.building_role, s.role);
            assert_eq!(identity.expires_at, s.expires_at, "same expiry");
            assert!(identity.admin);
            assert_eq!(
                authority.principal_id.as_deref(),
                Some(s.principal_id.as_str())
            );
            assert_eq!(authority.building_role, Some(s.role));
        } else {
            assert!(identity.is_none(), "an expired session stays expired");
            assert!(!authority.is_building_admin());
        }
    }
    drop(hub);
    let c = Connection::open(&path).unwrap();
    let mut live: Vec<_> = sessions
        .iter()
        .filter(|s| s.live())
        .map(|s| {
            (
                super::secret_hash(&s.token),
                s.name.to_string(),
                "user".to_string(),
                s.expires_at,
            )
        })
        .collect();
    live.sort();
    assert_eq!(admin_session_rows(&c), live, "the expired row was removed");
}

#[test]
fn admin_session_tokens_leave_no_trace_in_the_database_files_or_a_snapshot() {
    for layout in [
        LegacyLayout::PendingWal,
        LegacyLayout::CheckpointedWithFreedPages,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let (path, sessions) = legacy_session_files(directory.path(), &[], layout);
        // The fixture is meaningful: before the upgrade the tokens are on disk.
        assert!(sessions
            .iter()
            .all(|s| !files_containing(&path, s.secret_tail()).is_empty()));

        let store = Store::open(path.to_str()).expect("upgrade");
        for s in &sessions {
            for needle in [s.token.as_str(), s.secret_tail()] {
                assert_eq!(
                    sql_values_containing(&store.conn().unwrap(), needle),
                    Vec::<String>::new(),
                    "no column of any table holds the {} session's token",
                    s.name
                );
                assert_eq!(
                    files_containing(&path, needle),
                    Vec::<String>::new(),
                    "no byte of the open database, its WAL or shm holds the {} session's token",
                    s.name
                );
            }
        }
        // Signing in with the tokens after the upgrade writes none of them.
        let hub = hub_on(store);
        for s in sessions.iter().filter(|s| s.live()) {
            assert!(hub
                .resolve_authority(None, Some(&s.token))
                .is_building_admin());
        }
        drop(hub);
        for s in &sessions {
            assert_eq!(
                files_containing(&path, s.secret_tail()),
                Vec::<String>::new()
            );
        }

        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../scripts/storage_snapshot.py");
        let snapshot = directory.path().join("snapshot");
        let python = std::env::var("PYTHON").unwrap_or_else(|_| "python3".into());
        let backup = std::process::Command::new(&python)
            .arg(&script)
            .arg("backup")
            .arg(&path)
            .arg(&snapshot)
            .arg("--server-stopped")
            .output()
            .expect("python3 runs the snapshot tool");
        assert!(
            backup.status.success(),
            "{}",
            String::from_utf8_lossy(&backup.stderr)
        );
        for s in &sessions {
            assert_eq!(
                files_containing(&snapshot.join("loca.sqlite3"), s.secret_tail()),
                Vec::<String>::new(),
                "a snapshot taken after the upgrade holds no session token"
            );
        }
    }
}

#[test]
fn reopening_migrated_admin_sessions_changes_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let (path, sessions) = legacy_session_files(directory.path(), &[], LegacyLayout::PendingWal);
    let dump = |c: &Connection| {
        let mut rows: Vec<Vec<String>> = admin_session_rows(c)
            .into_iter()
            .map(|(hash, name, kind, expires_at)| vec![hash, name, kind, expires_at.to_string()])
            .collect();
        let mut statement = c
            .prepare(
                "SELECT id, principal_id, credential_id, created_at, expires_at, revoked_at
                 FROM principal_sessions ORDER BY id",
            )
            .unwrap();
        rows.extend(
            statement
                .query_map([], |row| {
                    (0..6)
                        .map(|i| Ok(format!("{:?}", row.get_ref(i)?)))
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .unwrap()
                .map(Result::unwrap),
        );
        let pending: i64 = c
            .query_row("SELECT count(*) FROM storage_maintenance", [], |r| r.get(0))
            .unwrap();
        rows.push(vec![format!("pending chores: {pending}")]);
        rows
    };
    // The first open migrates; a Hub on it restores the sessions.
    let hub = hub_on(Store::open(path.to_str()).expect("first open"));
    for s in sessions.iter().filter(|s| s.live()) {
        assert!(hub
            .resolve_authority(None, Some(&s.token))
            .is_building_admin());
    }
    drop(hub);
    let after_first = dump(&Connection::open(&path).unwrap());
    let schema = schema_dump(path.to_str().unwrap());
    // The second open finds nothing to migrate and nothing to clean.
    let second = Store::open(path.to_str()).expect("second open");
    assert_eq!(dump(&second.conn().unwrap()), after_first, "a no-op");
    drop(second);
    assert_eq!(schema_dump(path.to_str().unwrap()), schema);
    // And the sessions it restores are the same ones.
    let hub = hub_on(Store::open(path.to_str()).expect("third open"));
    for s in &sessions {
        assert_eq!(
            hub.session_identity(Some(&s.token))
                .and_then(|identity| identity.principal_id),
            s.live().then(|| s.principal_id.clone())
        );
    }
}

#[test]
fn a_failed_admin_session_migration_leaves_the_old_table_and_fails_open() {
    let read_old = |path: &std::path::Path| -> Vec<(String, String, String, u64)> {
        let c = Connection::open(path).unwrap();
        let mut statement = c
            .prepare("SELECT token, name, kind, expires_at FROM admin_sessions ORDER BY token")
            .expect("the old table is still readable in its old shape");
        let rows = statement
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        rows
    };
    let nothing_left = |path: &std::path::Path| {
        let c = Connection::open(path).unwrap();
        let leftovers: i64 = c
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'admin_sessions_hashed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(leftovers, 0, "the new table rolled back");
        let chores: i64 = c
            .query_row(
                "SELECT count(*) FROM storage_maintenance WHERE task = 'scrub_admin_session_tokens'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(chores, 0, "the chore rolled back with it");
    };

    // 1. Recording the clean-up chore is refused, after every hashed row was
    //    written (a trigger, as the other fault-injection tests do).
    let directory = tempfile::tempdir().unwrap();
    let (path, _) = legacy_session_files(directory.path(), &[], LegacyLayout::PendingWal);
    let expected = read_old(&path);
    assert_eq!(expected.len(), 3);
    Connection::open(&path)
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER refuse_session_scrub BEFORE INSERT ON storage_maintenance
             WHEN NEW.task = 'scrub_admin_session_tokens'
             BEGIN SELECT RAISE(ABORT, 'injected: session scrub refused'); END;",
        )
        .unwrap();
    let error = match Store::open(path.to_str()) {
        Ok(_) => panic!("open must fail when the migration fails"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("injected: session scrub refused"), "{error}");
    assert_eq!(read_old(&path), expected);
    nothing_left(&path);

    // 2. The rebuild itself fails: an operator's view over the old column
    //    cannot follow the new table.
    let directory = tempfile::tempdir().unwrap();
    let (path, _) = legacy_session_files(directory.path(), &[], LegacyLayout::PendingWal);
    let expected = read_old(&path);
    Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE VIEW session_tokens AS SELECT token FROM admin_sessions;")
        .unwrap();
    let error = match Store::open(path.to_str()) {
        Ok(_) => panic!("open must fail when the rebuild fails"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("session_tokens"), "the real cause: {error}");
    assert_eq!(read_old(&path), expected);
    nothing_left(&path);
}

#[test]
fn a_database_needing_both_migrations_ends_with_no_code_and_no_token_on_disk() {
    for layout in [
        LegacyLayout::PendingWal,
        LegacyLayout::CheckpointedWithFreedPages,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let smasters = legacy_smasters();
        let (path, sessions) = legacy_session_files(directory.path(), &smasters, layout);
        let secrets: Vec<String> = smasters
            .iter()
            .map(|s| s.secret_tail().to_string())
            .chain(sessions.iter().map(|s| s.secret_tail().to_string()))
            .collect();
        assert!(secrets
            .iter()
            .all(|secret| !files_containing(&path, secret).is_empty()));

        let store = Store::open(path.to_str()).expect("one open runs both migrations");
        {
            let c = store.conn().unwrap();
            assert!(!super::table_has_column(&c, "smasters", "token").unwrap());
            assert!(!super::table_has_column(&c, "admin_sessions", "token").unwrap());
            let pending: i64 = c
                .query_row("SELECT count(*) FROM storage_maintenance", [], |r| r.get(0))
                .unwrap();
            assert_eq!(pending, 0, "one clean-up cleared both chores");
            for secret in &secrets {
                assert_eq!(sql_values_containing(&c, secret), Vec::<String>::new());
            }
        }
        for secret in &secrets {
            assert_eq!(files_containing(&path, secret), Vec::<String>::new());
        }
        // The Smaster signs in with its code, and its session, taken before
        // the upgrade, is still its own.
        let hub = hub_on(store);
        let smaster_session = &sessions[1];
        assert_eq!(
            hub.resolve_authority(None, Some(&smaster_session.token))
                .principal_id
                .as_deref(),
            Some(smaster_session.principal_id.as_str())
        );
        drop(hub);
        for secret in &secrets {
            assert_eq!(files_containing(&path, secret), Vec::<String>::new());
        }
    }
}

#[test]
fn a_busy_checkpoint_after_both_migrations_leaves_both_chores_for_the_next_open() {
    let directory = tempfile::tempdir().unwrap();
    let smasters = legacy_smasters();
    let (path, sessions) =
        legacy_session_files(directory.path(), &smasters, LegacyLayout::PendingWal);
    let reader = Connection::open(&path).unwrap();
    reader
        .execute_batch("BEGIN; SELECT count(*) FROM admin_sessions;")
        .unwrap();
    let error = match Store::open(path.to_str()) {
        Ok(_) => panic!("a busy checkpoint must not count as a removed residue"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("checkpoint busy"), "{error}");
    assert!(
        error.contains("scrub_admin_session_tokens") && error.contains("scrub_smaster_codes"),
        "{error}"
    );
    reader.execute_batch("COMMIT").unwrap();
    drop(reader);
    let mut chores: Vec<String> = {
        let c = Connection::open(&path).unwrap();
        let mut statement = c.prepare("SELECT task FROM storage_maintenance").unwrap();
        let rows = statement
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        rows
    };
    chores.sort();
    assert_eq!(
        chores,
        ["scrub_admin_session_tokens", "scrub_smaster_codes"]
    );
    let store = Store::open(path.to_str()).expect("the next open finishes the scrub");
    let pending: i64 = store
        .conn()
        .unwrap()
        .query_row("SELECT count(*) FROM storage_maintenance", [], |r| r.get(0))
        .unwrap();
    assert_eq!(pending, 0);
    drop(store);
    for secret in smasters
        .iter()
        .map(LegacySmaster::secret_tail)
        .chain(sessions.iter().map(LegacySession::secret_tail))
    {
        assert_eq!(files_containing(&path, secret), Vec::<String>::new());
    }
}

#[test]
fn older_messages_are_attributed_once_and_only_from_a_smaster_seat() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("attribution.db");
    let store = Store::open(Some(path.to_str().unwrap())).unwrap();
    let deputy = store
        .add_smaster("sm_attrib", "deputy", 1)
        .unwrap()
        .principal_id;
    {
        // A database written before the column existed.
        let c = store.conn().unwrap();
        c.execute("ALTER TABLE messages DROP COLUMN sender_principal", [])
            .unwrap();
        let rows: [(u64, Option<String>); 7] = [
            (1, Some(format!("sm:pr:{deputy}"))),
            (2, Some("sm:pr:pr_nobody_has_this".into())),
            (3, Some("mb:mb_synthetic_member_key".into())),
            (4, Some("dv:dv_synthetic_davet".into())),
            (5, Some("@master".into())),
            (6, Some("user:deputy".into())),
            (7, None),
        ];
        for (id, principal) in rows {
            c.execute(
                "INSERT INTO messages (id, room, sender, sender_type, text, ts, kind, principal, op_id)
                 VALUES (?1, 'ops', 'deputy', 'user', 'old words', ?1, 'say', ?2,
                         CASE WHEN ?2 IS NULL THEN NULL ELSE 'op-' || ?1 END)",
                rusqlite::params![id, principal],
            )
            .unwrap();
        }
    }
    drop(store);
    let attributed = |store: &Store| -> Vec<(u64, Option<String>, Option<String>)> {
        let c = store.conn().unwrap();
        let mut statement = c
            .prepare("SELECT id, principal, sender_principal FROM messages ORDER BY id")
            .unwrap();
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    let store = Store::open(Some(path.to_str().unwrap())).unwrap();
    let rows = attributed(&store);
    let writer = |id: u64| rows.iter().find(|row| row.0 == id).unwrap().2.clone();
    assert_eq!(
        writer(1),
        Some(deputy.clone()),
        "a Smaster seat names its writer"
    );
    for id in 2..=7 {
        assert_eq!(writer(id), None, "row {id} has no certain writer");
    }
    // The operation scope itself is untouched.
    assert_eq!(
        rows.iter().map(|row| row.1.clone()).collect::<Vec<_>>()[..2],
        [
            Some(format!("sm:pr:{deputy}")),
            Some("sm:pr:pr_nobody_has_this".into())
        ]
    );

    // Once: a later open does not attribute again.
    store
        .conn()
        .unwrap()
        .execute(
            "INSERT INTO messages (id, room, sender, sender_type, text, ts, kind, principal, op_id)
             VALUES (8, 'ops', 'deputy', 'user', 'later', 8, 'say', ?1, 'op-8')",
            [format!("sm:pr:{deputy}")],
        )
        .unwrap();
    drop(store);
    let store = Store::open(Some(path.to_str().unwrap())).unwrap();
    let rows = attributed(&store);
    assert_eq!(rows.iter().find(|row| row.0 == 8).unwrap().2, None);
}
