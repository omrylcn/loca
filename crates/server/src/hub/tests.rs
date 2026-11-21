use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use protocol::{
    AttentionAudience, AttentionStatus, CareReason, CreateAttention, CreateGoal, CreateTask,
    GoalCompletion, GoalStatus, PostMessage, ReminderRecipient, RoomSettings, RuntimeHealthUpdate,
    SenderType, ServerFrame, SetWait, TaskStatus, UpdateGoal, UpdateTask,
};

use super::{
    validate_display_name, AttentionError, CareDraft, GoalError, Hub, HubConfig, ModerationError,
    NameError, ProfileRenameError,
};
use crate::store::{BuildingRole, InviteRecord, MemberRecord, Store};

/// Admit a new member and keep the key the hub hands over once, at the
/// admission: the store keeps only its hash.
fn admit_with_key(hub: &Hub, name: &str, kind: &str, by: &str) -> (MemberRecord, String) {
    let (member, key) = hub.admit_member_once(name, kind, by).unwrap();
    (
        member,
        key.expect("a member admitted now is handed its key"),
    )
}

/// An invite minted in a test: the invite itself, which the hub hands over once,
/// with its record's fields. `member` is the membership's id.
#[derive(Debug, Clone)]
struct Minted {
    token: String,
    room: String,
    member: String,
    name: String,
    kind: String,
    issued_by: String,
}

impl From<(InviteRecord, String)> for Minted {
    fn from((record, invite): (InviteRecord, String)) -> Self {
        Self {
            token: invite,
            room: record.room,
            member: record.member_id,
            name: record.name,
            kind: record.kind,
            issued_by: record.issued_by,
        }
    }
}

/// End the invite `invite` (a test holds the invite; the hub, its hash).
fn revoke_invite(hub: &Hub, invite: &str) -> bool {
    hub.revoke_invite_by_hash(&crate::store::secret_hash(invite))
        .unwrap()
}

/// Record an invite `invite` for `member` in `room`, as a mint does.
fn add_test_invite(hub: &Hub, invite_key: &str, room: &str, member: &MemberRecord, issued_at: u64) {
    hub.add_invite(
        InviteRecord::issued(
            invite_key,
            room,
            &member.id,
            &member.name,
            &member.kind,
            issued_at,
            "master",
        ),
        invite_key,
    )
    .unwrap();
}

/// A test that posts as a fixed sender names its operation scope directly.
impl From<&str> for super::OperationScope {
    fn from(key: &str) -> Self {
        Self {
            key: key.into(),
            principal: None,
            legacy: None,
        }
    }
}

mod claim_window;
mod continuity_core;
mod invite_delivery;
mod join_window;
mod key_ids;
mod lead_principal;
mod loca_seats;
mod loca_title;
mod name_claims;
mod notification_limit;
mod profile_card;
mod reissue;
mod session_lifetime;
mod task_assignment;

static TEST_NOW: AtomicU64 = AtomicU64::new(0);
static TEST_CLOCK_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn disabled_focus_never_generates_automatic_goal_reminders() {
    let _clock = TEST_CLOCK_LOCK.lock().expect("clock test lock");
    let mut hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        Arc::new(Store::open(None).unwrap()),
        RoomSettings {
            care_goal_secs: 1,
            ..RoomSettings::default()
        },
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);
    hub.set_lead("proj", Some("lead".into()), "operator")
        .unwrap();
    hub.create_goal(
        "proj",
        CreateGoal {
            outcome: "retained while disabled".into(),
            checkpoint: None,
            stale_after_secs: None,
            completion: GoalCompletion::Manual,
            task_ids: Vec::new(),
            by: "operator".into(),
        },
    )
    .unwrap();
    hub.set_extensions("proj", &[], "operator", SenderType::User)
        .unwrap();
    TEST_NOW.store(31_000, Ordering::Relaxed);
    hub.tick_care();
    assert!(
        hub.attentions("proj").is_empty(),
        "disabled Focus must not wake agents into a 403"
    );
}

#[test]
fn suspension_and_concurrent_invite_creation_cannot_leave_a_revivable_seat() {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(directory.path().join("suspend-race.db").to_str()).unwrap());
    let hub = critical_restart_hub(store, "race-fixture-root");
    let member = hub.admit_member("race-worker", "agent", "master").unwrap();
    let reference = member.id.clone();
    for _ in 0..20 {
        let token = hub.new_invite_token();
        let invite = InviteRecord::issued(
            &token,
            "race-room",
            &member.id,
            &member.name,
            &member.kind,
            1,
            "master",
        );
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let writer = hub.clone();
        let ready = barrier.clone();
        let invite_key = token.clone();
        let thread = std::thread::spawn(move || {
            ready.wait();
            let _ = writer.add_invite(invite, &invite_key);
        });
        barrier.wait();
        assert!(hub.set_member_suspended(&reference, true, "test").unwrap());
        thread.join().unwrap();
        assert!(!hub.has_active_invite("race-room", &member.id));
        assert!(hub.set_member_suspended(&reference, false, "").unwrap());
        assert!(hub.invite_for("race-room", Some(&token)).is_none());
    }
}

fn critical_restart_hub(store: Arc<Store>, root: &str) -> Hub {
    Hub::build(
        HubConfig {
            admin_token: root.into(),
            room_token: String::new(),
            require_sessions: true,
            require_invite: true,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        store,
        RoomSettings::default(),
        1,
    )
}

fn session_request(name: &str) -> protocol::CreateSession {
    protocol::CreateSession {
        name: name.into(),
        kind: Some(SenderType::User),
        runtime: None,
        capabilities: vec![],
    }
}

/// The route resolves the membership behind a member's own key, then records
/// the session. Hold it between the two while the Master revokes the
/// membership (memory-only store: no principal binds the session, the
/// membership does): no session may be recorded from the stale copy.
#[test]
fn a_membership_revoked_after_it_was_resolved_records_no_session() {
    let hub = critical_restart_hub(Arc::new(Store::open(None).unwrap()), "race-root");
    let (member, key) = admit_with_key(&hub, "late-member", "user", "master");
    let resolved = hub.member_for_credential(Some(&key));
    assert!(resolved.is_some(), "the route resolved a live membership");
    assert!(hub.revoke_member(&member.id).unwrap());
    let created = hub
        .create_session_scoped(session_request("late-member"), None, None, false, resolved)
        .unwrap();
    assert!(
        created.is_none(),
        "a session was recorded for a revoked membership"
    );
}

/// The same window for an invite: resolved, then revoked on its own, then the
/// session is recorded. The invite's session must not outlive the invite.
#[test]
fn an_invite_revoked_after_it_was_resolved_records_no_session() {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(directory.path().join("invite-race.db").to_str()).unwrap());
    let hub = critical_restart_hub(store, "race-root");
    let member = hub.admit_member("late-guest", "agent", "master").unwrap();
    let token = hub.new_invite_token();
    add_test_invite(&hub, &token, "late-room", &member, 1_767_225_600_000);
    let resolved = hub.invite_by_token(&token).expect("a live invite");
    assert!(hub.revoke_invite_by_hash(&resolved.secret_hash).unwrap());
    let created = hub
        .create_session_scoped(
            session_request("late-guest"),
            Some(&resolved),
            None,
            false,
            None,
        )
        .unwrap();
    assert!(
        created.is_none(),
        "a session was recorded for a revoked invite"
    );
}

/// Persistent store: a member's own-key session is bound to its principal,
/// which suspension and revocation disable. Resolved before either, recorded
/// after, it must still open nothing.
#[test]
fn a_principal_session_recorded_after_suspension_or_revocation_is_dead() {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(directory.path().join("grant-race.db").to_str()).unwrap());
    let hub = critical_restart_hub(store, "race-root");
    for (name, suspend) in [("late-suspended", true), ("late-revoked", false)] {
        let (member, key) = admit_with_key(&hub, name, "user", "master");
        let grant = hub
            .admin_session_grant(Some(&key), Hub::ADMIN_SESSION_TTL_MS)
            .expect("a member's own key binds its principal");
        if suspend {
            let reference = member.id.clone();
            assert!(hub.set_member_suspended(&reference, true, "test").unwrap());
        } else {
            assert!(hub.revoke_member(&member.id).unwrap());
        }
        let created = hub
            .create_session_scoped(session_request(name), None, Some(grant), false, None)
            .unwrap();
        if let Some(session) = created {
            assert!(
                hub.session_identity(Some(&session.session_token)).is_none(),
                "{name}: the session outlived its principal"
            );
        }
    }
}

fn critical_admin_session(hub: &Hub, key: &str) -> String {
    hub.create_session_scoped(
        protocol::CreateSession {
            name: "untrusted-body-name".into(),
            kind: Some(SenderType::User),
            runtime: None,
            capabilities: vec![],
        },
        None,
        hub.admin_session_grant(Some(key), Hub::ADMIN_SESSION_TTL_MS),
        false,
        None,
    )
    .unwrap()
    .expect("session recorded")
    .session_token
}

#[test]
fn critical_restart_preserves_smaster_and_refuses_unbound_legacy_session() {
    let temp = tempfile::tempdir().unwrap();
    let store =
        Arc::new(Store::open(Some(temp.path().join("restart.db").to_str().unwrap())).unwrap());
    store.add_smaster("sm_deputy", "deputy", 1).unwrap();
    let hub = critical_restart_hub(store.clone(), "root");
    let smaster = critical_admin_session(&hub, "sm_deputy");
    let master = critical_admin_session(&hub, "root");
    store
        .save_admin_session("st_unbound", "legacy", SenderType::User, hub.now() + 60_000)
        .unwrap();
    drop(hub);
    let hub = critical_restart_hub(store.clone(), "root");
    assert_eq!(
        hub.resolve_authority(None, Some(&smaster)).building_role,
        Some(BuildingRole::Smaster)
    );
    assert!(!hub.resolve_authority(None, Some(&smaster)).is_master());
    assert!(hub.resolve_authority(None, Some(&master)).is_master());
    assert!(!hub
        .resolve_authority(None, Some("st_unbound"))
        .is_building_admin());
    hub.revoke_smaster_ref("sm_deputy").unwrap();
    drop(hub);
    let hub = critical_restart_hub(store, "root");
    assert!(!hub
        .resolve_authority(None, Some(&smaster))
        .is_building_admin());
}

/// The Hub keeps sessions under their token's hash, so after a restart it
/// has nothing else. A restored session must still end on sign-out and on
/// revocation, in memory and in the database, and stay ended across the
/// next restart.
#[test]
fn restored_admin_sessions_are_keyed_by_hash_and_still_end() {
    use crate::store::secret_hash;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("restored-sessions.db");
    let store = Arc::new(Store::open(Some(path.to_str().unwrap())).unwrap());
    store.add_smaster("sm_deputy", "deputy", 1).unwrap();
    let hub = critical_restart_hub(store.clone(), "root");
    let master_id = hub
        .resolve_authority(Some("root"), None)
        .principal_id
        .expect("Master principal");
    let (device, device_secret) = hub.create_profile_credential(&master_id, "Laptop").unwrap();
    let master = critical_admin_session(&hub, "root");
    let signed_out = critical_admin_session(&hub, "root");
    let on_device = critical_admin_session(&hub, &device_secret);
    let smaster = critical_admin_session(&hub, "sm_deputy");
    drop(hub);

    let keys = |hub: &Hub| {
        let mut keys: Vec<String> = hub.sessions.lock().unwrap().keys().cloned().collect();
        keys.sort();
        keys
    };
    let hashes = |tokens: &[&String]| {
        let mut hashes: Vec<String> = tokens.iter().map(|t| secret_hash(t)).collect();
        hashes.sort();
        hashes
    };
    let stored = || {
        let c = rusqlite::Connection::open(&path).unwrap();
        let mut statement = c
            .prepare("SELECT secret_hash FROM admin_sessions ORDER BY secret_hash")
            .unwrap();
        let rows: Vec<String> = statement
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        rows
    };
    let revoked_in_db = |token: &str| -> bool {
        rusqlite::Connection::open(&path)
            .unwrap()
            .query_row(
                "SELECT revoked_at IS NOT NULL FROM principal_sessions WHERE id = ?1",
                [secret_hash(token)],
                |r| r.get(0),
            )
            .unwrap()
    };

    // Restart: only hashes, in the database and in memory.
    let hub = critical_restart_hub(store.clone(), "root");
    let all = [&master, &signed_out, &on_device, &smaster];
    assert_eq!(keys(&hub), hashes(&all), "sessions are keyed by hash");
    assert_eq!(stored(), hashes(&all));
    for token in all {
        assert!(hub.resolve_authority(None, Some(token)).is_building_admin());
    }

    // Sign-out of a restored session.
    assert!(hub.revoke_session(&signed_out).unwrap());
    assert!(!keys(&hub).contains(&secret_hash(&signed_out)));
    assert!(!stored().contains(&secret_hash(&signed_out)));
    assert!(revoked_in_db(&signed_out));
    assert!(!hub
        .resolve_authority(None, Some(&signed_out))
        .is_building_admin());
    assert!(
        !hub.revoke_session(&signed_out).unwrap(),
        "a second sign-out finds nothing"
    );

    // Revoking the credential a restored session was taken with drops it
    // from memory at once, not only at its next lookup.
    hub.revoke_profile_credential(&master_id, &device.id)
        .unwrap();
    assert!(!keys(&hub).contains(&secret_hash(&on_device)));
    assert!(revoked_in_db(&on_device));
    assert!(!hub
        .resolve_authority(None, Some(&on_device))
        .is_building_admin());

    // Revoking the Smaster ends its restored session.
    assert!(hub.revoke_smaster_ref("sm_deputy").unwrap());
    assert!(!hub
        .resolve_authority(None, Some(&smaster))
        .is_building_admin());
    assert!(hub.resolve_authority(None, Some(&master)).is_master());
    drop(hub);

    // Nothing comes back with the next restart.
    let hub = critical_restart_hub(store, "root");
    assert_eq!(keys(&hub), hashes(&[&master]));
    assert_eq!(stored(), hashes(&[&master]));
    for token in [&signed_out, &on_device, &smaster] {
        assert!(!hub.resolve_authority(None, Some(token)).is_building_admin());
    }
    assert!(hub.resolve_authority(None, Some(&master)).is_master());
}

/// A seat key built from a session (a Smaster session without a principal,
/// as in a memory-only sandbox, or a plain building-key session) names the
/// session by its hash. The key leaves the process in `Evicted` frames.
#[test]
fn a_seat_key_names_a_session_by_its_hash_never_its_token() {
    use crate::store::secret_hash;
    let hub = critical_restart_hub(Arc::new(Store::open(None).unwrap()), "root");
    let code = hub.add_smaster("deputy").unwrap();
    let smaster = critical_admin_session(&hub, &code);
    let identity = hub.session_identity(Some(&smaster)).expect("live session");
    assert_eq!(identity.building_role, BuildingRole::Smaster);
    assert!(identity.principal_id.is_none(), "the path under test");
    let seat = hub.seat_identity(None, None, Some(&identity), Some(&smaster), "deputy");
    assert_eq!(seat, format!("sm:st:{}", secret_hash(&smaster)));

    let plain = hub
        .create_session_scoped(
            protocol::CreateSession {
                name: "guest".into(),
                kind: Some(SenderType::Agent),
                runtime: None,
                capabilities: vec![],
            },
            None,
            None,
            true,
            None,
        )
        .unwrap()
        .expect("session recorded")
        .session_token;
    let guest = hub.session_identity(Some(&plain)).expect("live session");
    assert_eq!(
        hub.seat_identity(None, None, Some(&guest), Some(&plain), "guest"),
        format!("st:{}", secret_hash(&plain))
    );

    // A second connection on the same seat under another name evicts the
    // first; the frame names the seat, and so carries no token.
    let mut rx = hub.subscribe("ops").0;
    assert!(hub.join("ops", &seat, "deputy", SenderType::User, 1));
    assert!(hub.join("ops", &seat, "deputy-laptop", SenderType::User, 2));
    let mut evicted = 0;
    while let Ok(frame) = rx.try_recv() {
        let wire = serde_json::to_string(&frame).unwrap();
        assert!(!wire.contains(&smaster[3..]), "a frame carries the token");
        if let ServerFrame::Evicted { identity, .. } = frame {
            assert_eq!(identity, seat);
            evicted += 1;
        }
    }
    assert_eq!(evicted, 1);
}

#[test]
fn smaster_and_master_browser_sessions_take_separate_seats() {
    let temp = tempfile::tempdir().unwrap();
    let store =
        Arc::new(Store::open(Some(temp.path().join("seats.db").to_str().unwrap())).unwrap());
    store.add_smaster("sm_deputy", "deputy", 1).unwrap();
    let hub = critical_restart_hub(store, "root");
    let master_a = critical_admin_session(&hub, "root");
    let master_b = critical_admin_session(&hub, "root");
    let smaster = critical_admin_session(&hub, "sm_deputy");
    let seat = |tok: &str, name: &str| {
        let s = hub.session_identity(Some(tok)).expect("live session");
        hub.seat_identity(None, None, Some(&s), Some(tok), name)
    };
    let master_seat = seat(&master_a, "Master");
    // Two Master screens are still one person, one seat.
    assert_eq!(master_seat, seat(&master_b, "Master"));
    let smaster_seat = seat(&smaster, "deputy");
    assert_ne!(
        master_seat, smaster_seat,
        "a Smaster must not wear the Master's seat"
    );

    let mut rx = hub.subscribe("ops").0;
    assert!(hub.join("ops", &master_seat, "Master", SenderType::User, 1));
    assert!(hub.join("ops", &smaster_seat, "deputy", SenderType::User, 2));
    while let Ok(frame) = rx.try_recv() {
        assert!(
            !matches!(frame, ServerFrame::Evicted { .. }),
            "Master and Smaster must not evict each other"
        );
    }
}

#[test]
fn display_names_are_labels_never_titles() {
    for ok in ["omrylcn", "ayse.k", "dev_ops-2", "a"] {
        assert_eq!(validate_display_name(&format!("  {ok} ")), Ok(ok));
    }
    for bad in [
        "",
        "   ",
        "two words",
        "ömer",
        "a@b",
        "_ops",
        ".dot",
        "-dash",
        "x".repeat(65).as_str(),
    ] {
        assert_eq!(
            validate_display_name(bad),
            Err(NameError::Invalid),
            "{bad:?}"
        );
    }
    // A title is refused in any letter case, Turkish dotted I included.
    for title in [
        "master", "Master", "SMASTER", "Operator", "lead", "ALL", "admin", "root",
    ] {
        assert_eq!(
            validate_display_name(title),
            Err(NameError::Reserved),
            "{title}"
        );
    }
    assert_eq!(
        validate_display_name("ADM\u{0130}N"),
        Err(NameError::Invalid)
    );
    // Agent identities with their own fixed names stay valid.
    assert_eq!(validate_display_name("loca-care"), Ok("loca-care"));
}

#[test]
fn loca_roster_wears_each_building_rank_beside_the_name() {
    let temp = tempfile::tempdir().unwrap();
    let store =
        Arc::new(Store::open(Some(temp.path().join("roster.db").to_str().unwrap())).unwrap());
    store.add_smaster("sm_deputy", "deputy", 1).unwrap();
    let hub = critical_restart_hub(store, "root");
    let member = hub.admit_member("worker", "agent", "master").unwrap();
    let master = critical_admin_session(&hub, "root");
    let smaster = critical_admin_session(&hub, "sm_deputy");
    let seat = |tok: &str, name: &str| {
        let s = hub.session_identity(Some(tok)).expect("live session");
        hub.seat_identity(None, None, Some(&s), Some(tok), name)
    };
    let mut rx = hub.subscribe("ops").0;
    assert!(hub.join(
        "ops",
        &seat(&master, "Master"),
        "Master",
        SenderType::User,
        1
    ));
    assert!(hub.join(
        "ops",
        &seat(&smaster, "deputy"),
        "deputy",
        SenderType::User,
        2
    ));
    let worker_seat = format!("mb:{}", member.id.clone());
    assert!(hub.join("ops", &worker_seat, "worker", SenderType::Agent, 3));
    let mut last = None;
    while let Ok(frame) = rx.try_recv() {
        if let ServerFrame::Members { members } = frame {
            last = Some(members);
        }
    }
    let roles: Vec<(String, Option<String>)> = last
        .expect("a roster frame")
        .into_iter()
        .map(|m| (m.name, m.role))
        .collect();
    assert_eq!(
        roles,
        vec![
            ("Master".into(), Some("master".into())),
            ("deputy".into(), Some("smaster".into())),
            ("worker".into(), None),
        ]
    );

    // Ordinary members carry no role on the wire, and an old frame without
    // the field still parses.
    let wire = serde_json::to_value(protocol::Member {
        name: "worker".into(),
        kind: SenderType::Agent,
        role: None,
        full_name: None,
        principal_id: None,
        icon: None,
        avatar_color: None,
        avatar_shape: None,
        kit_version: None,
    })
    .unwrap();
    assert!(wire.get("role").is_none());
    let old: protocol::Member =
        serde_json::from_value(serde_json::json!({ "name": "x", "type": "user" })).unwrap();
    assert_eq!(old.role, None);

    // The Building directory lists every rank — seated or not — with its locas.
    let ranks = hub.building_ranks();
    let summary: Vec<(String, &str, Vec<String>)> = ranks
        .into_iter()
        .map(|r| (r.name, r.role, r.locas))
        .collect();
    assert_eq!(
        summary,
        vec![
            ("Master".into(), "master", vec!["ops".to_string()]),
            ("deputy".into(), "smaster", vec!["ops".to_string()]),
        ]
    );
    hub.leave("ops", &seat(&smaster, "deputy"), 2);
    let deputy = hub
        .building_ranks()
        .into_iter()
        .find(|r| r.role == "smaster")
        .expect("an unseated Smaster is still part of the Building");
    assert!(deputy.locas.is_empty());
}

/// An identity is seated exactly while one of its connections is, whatever
/// order a reconnect's join and the old connection's leave arrive in.
#[test]
fn a_seat_is_held_while_any_of_its_connections_is_seated() {
    let hub = critical_restart_hub(Arc::new(Store::open(None).unwrap()), "root");
    let seated = |identity: &str| {
        hub.members("ops")
            .iter()
            .any(|member| member.name == identity)
    };

    // An agent reconnects: the new connection (2) evicts the old one (1),
    // whose leave arrives late. The seat stays with the new connection.
    assert!(hub.join("ops", "name:agent", "agent", SenderType::Agent, 1));
    assert!(hub.join("ops", "name:agent", "agent", SenderType::Agent, 2));
    hub.leave("ops", "name:agent", 1);
    assert!(seated("agent"), "a late leave freed the reconnected seat");
    hub.leave("ops", "name:agent", 2);
    assert!(
        !seated("agent"),
        "the last connection's leave frees the seat"
    );

    // The old leave arriving first is the ordinary case.
    assert!(hub.join("ops", "name:agent", "agent", SenderType::Agent, 3));
    hub.leave("ops", "name:agent", 3);
    assert!(!seated("agent"));
    assert!(hub.join("ops", "name:agent", "agent", SenderType::Agent, 4));
    assert!(seated("agent"));
    hub.leave("ops", "name:agent", 4);

    // Two web clients of one human share the seat until both have left, in
    // either order, and a repeated leave changes nothing.
    assert!(hub.join("ops", "name:human", "human", SenderType::User, 5));
    assert!(hub.join("ops", "name:human", "human", SenderType::User, 6));
    hub.leave("ops", "name:human", 6);
    hub.leave("ops", "name:human", 6);
    assert!(seated("human"), "the other screen still reads");
    hub.leave("ops", "name:human", 5);
    assert!(!seated("human"));
}

#[test]
fn a_rank_renames_its_own_profile_everywhere_it_is_worn() {
    let temp = tempfile::tempdir().unwrap();
    let store =
        Arc::new(Store::open(Some(temp.path().join("rename.db").to_str().unwrap())).unwrap());
    store.add_smaster("sm_deputy", "smaster", 1).unwrap();
    store.add_smaster("sm_other", "selin", 1).unwrap();
    let hub = critical_restart_hub(store.clone(), "root");
    let (_, member_key) = admit_with_key(&hub, "worker", "agent", "master");
    let session = critical_admin_session(&hub, "sm_deputy");
    let identity = hub.session_identity(Some(&session)).unwrap();
    let seat = hub.seat_identity(None, None, Some(&identity), Some(&session), "smaster");
    assert!(hub.join("ops", &seat, "smaster", SenderType::User, 1));
    let mut rx = hub.subscribe("ops").0;

    let actor = hub.resolve_authority(None, Some(&session));
    // A title, a malformed label, or another person's name is refused.
    assert_eq!(
        hub.rename_profile(&actor, "master"),
        Err(ProfileRenameError::Name(NameError::Reserved))
    );
    assert_eq!(
        hub.rename_profile(&actor, "two words"),
        Err(ProfileRenameError::Name(NameError::Invalid))
    );
    assert_eq!(
        hub.rename_profile(&actor, "SELIN"),
        Err(ProfileRenameError::NameTaken)
    );
    assert_eq!(
        hub.rename_profile(&actor, "worker"),
        Err(ProfileRenameError::NameTaken)
    );

    let renamed = hub.rename_profile(&actor, "ayse").expect("rename");
    assert_eq!(renamed.display_name, "ayse");
    assert_eq!(renamed.role, BuildingRole::Smaster);
    // Same principal, same rank, new label — in the store, the registry,
    // the live session, the request authority and the loca seat.
    assert_eq!(renamed.id, actor.principal_id.clone().unwrap());
    assert_eq!(hub.smaster_name(Some("sm_deputy")).as_deref(), Some("ayse"));
    assert_eq!(hub.session_identity(Some(&session)).unwrap().name, "ayse");
    let after = hub.resolve_authority(None, Some(&session));
    assert_eq!(after.display_name.as_deref(), Some("ayse"));
    assert_eq!(after.building_role, Some(BuildingRole::Smaster));
    let mut roster = None;
    while let Ok(frame) = rx.try_recv() {
        if let ServerFrame::Members { members } = frame {
            roster = Some(members);
        }
    }
    let roster = roster.expect("the rename is broadcast to the loca");
    assert_eq!(roster.len(), 1, "a rename keeps the one seat");
    assert_eq!(roster[0].name, "ayse");
    assert_eq!(roster[0].role.as_deref(), Some("smaster"));
    // Renaming to one's own current name in another case is not a clash.
    assert!(hub.rename_profile(&after, "Ayse").is_ok());

    // The new name survives a restart.
    drop(hub);
    let hub = critical_restart_hub(store, "root");
    assert_eq!(hub.smaster_name(Some("sm_deputy")).as_deref(), Some("Ayse"));
    assert!(hub
        .profiles()
        .iter()
        .any(|p| p.display_name == "Ayse" && p.role == BuildingRole::Smaster));

    // The Master renames itself too; an ordinary member cannot use this path.
    let master = hub.resolve_authority(Some("root"), None);
    assert_eq!(
        hub.rename_profile(&master, "omrylcn").unwrap().role,
        BuildingRole::Master
    );
    assert_eq!(hub.master_principal().unwrap().display_name, "omrylcn");
    let worker = hub.resolve_authority(Some(&member_key), None);
    assert_eq!(
        hub.rename_profile(&worker, "boss"),
        Err(ProfileRenameError::AuthorityRequired)
    );
}

#[test]
fn a_rename_carries_the_rank_s_past_messages_but_not_a_member_s() {
    let temp = tempfile::tempdir().unwrap();
    let store =
        Arc::new(Store::open(Some(temp.path().join("relabel.db").to_str().unwrap())).unwrap());
    store.add_smaster("sm_deputy", "smaster2", 1).unwrap();
    let hub = critical_restart_hub(store.clone(), "root");
    let session = critical_admin_session(&hub, "sm_deputy");
    let actor = hub.resolve_authority(None, Some(&session));
    let deputy = actor.principal_id.clone().expect("deputy principal");
    let say =
        |sender: &str, text: &str, op: Option<&str>, principal: &str, writer: Option<&str>| {
            hub.post(
                "ops",
                PostMessage {
                    kind: Default::default(),
                    sender: sender.into(),
                    sender_type: SenderType::User,
                    target: None,
                    text: text.into(),
                    reply_to: None,
                    op_id: op.map(Into::into),
                    attachments: Vec::new(),
                },
                true,
                &principal.into(),
                writer,
            )
            .unwrap()
        };
    let first = say(
        "smaster2",
        "with op id",
        Some("op-1"),
        "user:smaster2",
        Some(&deputy),
    );
    let second = say(
        "smaster2",
        "without op id",
        None,
        "user:smaster2",
        Some(&deputy),
    );
    // A member that once wore the same label is someone else.
    say(
        "smaster2",
        "a member's words",
        Some("op-2"),
        "mb:mbid_someone",
        Some("pr_someone_else"),
    );
    let mut rx = hub.subscribe("ops").0;

    hub.rename_profile(&actor, "zhd").expect("rename");

    let live: Vec<(String, String)> = hub
        .subscribe("ops")
        .1
        .into_iter()
        .map(|m| (m.sender, m.text))
        .collect();
    assert!(live.contains(&("zhd".into(), "with op id".into())));
    assert!(live.contains(&("zhd".into(), "without op id".into())));
    assert_eq!(
        rename_frames(&mut rx),
        vec![
            Told::ById("zhd".into(), vec![first.id, second.id], Vec::new()),
            Told::ByName("smaster2".into(), "zhd".into()),
        ],
        "open feeds are told exactly which messages changed, then by name as before"
    );

    // On disk too: a restart shows the new name on the rank's messages and
    // leaves the member's untouched.
    drop(hub);
    let stored = store
        .messages_after("ops", 0, 0, u64::MAX, 50)
        .unwrap()
        .unwrap();
    let by_text = |t: &str| stored.iter().find(|m| m.text == t).unwrap().sender.clone();
    assert_eq!(by_text("with op id"), "zhd");
    assert_eq!(by_text("without op id"), "zhd");
    assert_eq!(by_text("a member's words"), "smaster2");
    assert_eq!(
        stored.iter().find(|m| m.id == first.id).unwrap().sender,
        "zhd"
    );
}

#[test]
fn loca_moderation_cannot_reach_up_the_building_ranks() {
    use protocol::ModAction::*;
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(Some(temp.path().join("rank.db").to_str().unwrap())).unwrap());
    store.add_smaster("sm_alice", "alice", 1).unwrap();
    store.add_smaster("sm_bob", "bob", 1).unwrap();
    let hub = critical_restart_hub(store, "root");
    hub.admit_member("worker", "agent", "master").unwrap();
    let master_name = hub
        .master_principal()
        .expect("master principal")
        .display_name;

    let master = hub.resolve_authority(Some("root"), None);
    let smaster = hub.resolve_authority(Some("sm_alice"), None);
    let operator = hub.resolve_authority(None, None);

    for action in [Mute, Kick, Ban, Release] {
        // Nobody moderates the Master — not a Smaster, not a Loca Operator.
        assert!(!hub.may_moderate("parlor", &smaster, action, &master_name));
        assert!(!hub.may_moderate("parlor", &operator, action, &master_name));
        // Only the Master moderates a Smaster; peers and Operators cannot.
        assert!(hub.may_moderate("parlor", &master, action, "alice"));
        assert!(!hub.may_moderate("parlor", &smaster, action, "bob"));
        assert!(!hub.may_moderate("parlor", &operator, action, "alice"));
        // Ordinary participants stay under every operator's moderation.
        assert!(hub.may_moderate("parlor", &operator, action, "worker"));
        assert!(hub.may_moderate("parlor", &smaster, action, "worker"));
    }
    // Lifting a nonexistent restriction does not override anyone's decision.
    assert!(hub.may_moderate("parlor", &operator, Unmute, &master_name));
    assert!(hub.may_moderate("parlor", &smaster, Unban, "bob"));
}

#[test]
fn memory_only_restrictions_preserve_rank_under_concurrent_moderation() {
    use protocol::ModAction::{Mute, Unmute};
    let hub = critical_restart_hub(Arc::new(Store::open(None).unwrap()), "root");
    let master = hub.resolve_authority(Some("root"), None);
    let operator = hub.resolve_authority(None, None);
    for _ in 0..10 {
        hub.moderate("parlor", Unmute, "worker", &master).unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let higher = {
            let hub = hub.clone();
            let master = master.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                hub.moderate("parlor", Mute, "worker", &master).unwrap();
            })
        };
        let lower = {
            let hub = hub.clone();
            let operator = operator.clone();
            std::thread::spawn(move || {
                barrier.wait();
                let _ = hub.moderate("parlor", Unmute, "worker", &operator);
            })
        };
        higher.join().unwrap();
        lower.join().unwrap();
        assert!(hub.mod_state("parlor").muted.contains(&"worker".into()));
        assert!(matches!(
            hub.moderate("parlor", Unmute, "worker", &operator),
            Err(ModerationError::Forbidden)
        ));
    }
}

#[test]
fn critical_root_rotation_revokes_old_root_and_derived_session() {
    let temp = tempfile::tempdir().unwrap();
    let store =
        Arc::new(Store::open(Some(temp.path().join("rotation.db").to_str().unwrap())).unwrap());
    let hub = critical_restart_hub(store.clone(), "root-one");
    let session = critical_admin_session(&hub, "root-one");
    let principal = store.active_master_principal().unwrap();
    store
        .create_credential(&principal.id, "independent device", "ak_device", hub.now())
        .unwrap();
    let device_session = critical_admin_session(&hub, "ak_device");
    drop(hub);
    let hub = critical_restart_hub(store.clone(), "root-two");
    assert!(!hub
        .resolve_authority(Some("root-one"), None)
        .is_building_admin());
    assert!(!hub
        .resolve_authority(None, Some(&session))
        .is_building_admin());
    assert!(hub.resolve_authority(Some("root-two"), None).is_master());
    assert!(hub.resolve_authority(Some("ak_device"), None).is_master());
    assert!(hub
        .resolve_authority(None, Some(&device_session))
        .is_master());
    let current = critical_admin_session(&hub, "root-two");
    drop(hub);
    let hub = critical_restart_hub(store.clone(), "root-two");
    assert!(hub.resolve_authority(None, Some(&current)).is_master());
    drop(hub);
    let hub = critical_restart_hub(store, "root-one");
    assert!(hub.resolve_authority(Some("root-one"), None).is_master());
    assert!(!hub
        .resolve_authority(None, Some(&session))
        .is_building_admin());
}

#[test]
fn critical_sealed_room_cannot_be_revived_by_invitation() {
    let temp = tempfile::tempdir().unwrap();
    let store =
        Arc::new(Store::open(Some(temp.path().join("sealed.db").to_str().unwrap())).unwrap());
    store.seal_room("sealed", 1).unwrap();
    let agent = store
        .add_member("mb_agent", "agent", "agent", 1, "master")
        .unwrap();
    let hub = critical_restart_hub(store, "root");
    assert!(hub
        .invite_member_to_room(&agent.id, "sealed", "master")
        .is_err());
    assert!(hub.is_deleted("sealed"));
    assert!(hub.invites_of("sealed").is_empty());
}

#[test]
fn critical_message_counter_includes_sealed_history() {
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(Some(temp.path().join("ids.db").to_str().unwrap())).unwrap());
    store
        .insert_message(
            &protocol::Message {
                id: 100,
                room: "sealed".into(),
                sender: "operator".into(),
                sender_type: SenderType::User,
                target: None,
                text: "retained history".into(),
                kind: protocol::MessageKind::Say,
                reply_to: None,
                reply_to_sender: None,
                sender_principal: None,
                ts: 1,
                attachments: vec![],
            },
            None,
        )
        .unwrap();
    store.seal_room("sealed", 1).unwrap();
    let hub = critical_restart_hub(store, "root");
    assert!(
        hub.next_session_id() > 100,
        "global allocator must not reuse sealed message ids"
    );
}

fn test_now_ms() -> u64 {
    TEST_NOW.load(Ordering::Relaxed)
}

#[test]
fn mutation_actor_rejects_expired_and_revoked_sessions() {
    let _clock = TEST_CLOCK_LOCK.lock().unwrap();
    TEST_NOW.store(1_000, Ordering::Relaxed);
    let mut hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: true,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        Arc::new(Store::open(None).unwrap()),
        RoomSettings::default(),
        1,
    );
    hub.now_ms = test_now_ms;
    let mint = || {
        hub.create_session_scoped(
            protocol::CreateSession {
                name: "body-name".into(),
                kind: Some(SenderType::User),
                runtime: None,
                capabilities: vec![],
            },
            None,
            Some(hub.master_pairing_grant(10)),
            false,
            None,
        )
        .unwrap()
        .expect("session recorded")
        .session_token
    };
    let token = mint();
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("x-session-token", token.parse().unwrap());
    assert_eq!(
        crate::routes::mutation_actor(&hub, &headers, "forged".into(), SenderType::Agent).unwrap(),
        ("operator".into(), SenderType::User)
    );
    TEST_NOW.store(1_010, Ordering::Relaxed);
    assert!(matches!(
        crate::routes::mutation_actor(&hub, &headers, "forged".into(), SenderType::Agent),
        Err(axum::http::StatusCode::UNAUTHORIZED)
    ));
    let token = mint();
    headers.insert("x-session-token", token.parse().unwrap());
    hub.revoke_session(&token).unwrap();
    assert!(matches!(
        crate::routes::mutation_actor(&hub, &headers, "forged".into(), SenderType::Agent),
        Err(axum::http::StatusCode::UNAUTHORIZED)
    ));
}

#[test]
fn authority_resolves_server_side_principals_and_revocation_takes_effect_live() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("authority.db");
    let store = Arc::new(Store::open(Some(path.to_str().expect("db path"))).expect("store"));
    store
        .ensure_master_principal("MASTER", "operator", 1)
        .expect("master principal");
    store
        .add_smaster("sm_alice", "alice", 2)
        .expect("smaster principal");
    let hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        store,
        RoomSettings::default(),
        1,
    );

    let master = hub.resolve_authority(Some("MASTER"), None);
    assert_eq!(master.building_role, Some(BuildingRole::Master));
    assert!(master.principal_id.is_some());
    assert!(master.is_master());

    let smaster = hub.resolve_authority(Some("sm_alice"), None);
    assert_eq!(smaster.building_role, Some(BuildingRole::Smaster));
    assert!(smaster.principal_id.is_some());
    assert!(smaster.is_building_admin());
    assert!(!smaster.is_master());

    let unknown = hub.resolve_authority(Some("sm_alice-but-client-claims-master"), None);
    assert_eq!(unknown.building_role, None);
    assert!(!unknown.is_building_admin());

    hub.revoke_smaster_ref("sm_alice").expect("revoke smaster");
    let revoked = hub.resolve_authority(Some("sm_alice"), None);
    assert_eq!(revoked.building_role, None);
    assert!(!revoked.is_building_admin());
}

#[test]
fn a_reply_to_a_caretaker_is_addressed_once() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("addressing.db");
    let store = Arc::new(Store::open(Some(path.to_str().expect("db path"))).expect("store"));
    let hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::from(["care-two".into()]),
        },
        store,
        RoomSettings::default(),
        1,
    );

    let base = protocol::Message {
        id: 5,
        room: "reviewer".into(),
        sender: "someone".into(),
        sender_type: SenderType::Agent,
        target: None,
        text: "answering".into(),
        kind: protocol::MessageKind::Say,
        reply_to: Some(1),
        reply_to_sender: Some("care-two".into()),
        sender_principal: None,
        ts: 1,
        attachments: Vec::new(),
    };

    // A reply whose author is a caretaker addresses them — the new path, with no
    // explicit target and no @mention.
    assert_eq!(
        hub.addressed_caretakers(&base),
        vec!["care-two".to_string()]
    );

    // When the same caretaker is ALSO the target and @mentioned, sort+dedup keep
    // exactly one summon (never three).
    let triple = protocol::Message {
        target: Some("care-two".into()),
        text: "@care-two answering".into(),
        ..base.clone()
    };
    assert_eq!(
        hub.addressed_caretakers(&triple),
        vec!["care-two".to_string()]
    );

    // A non-caretaker reply author addresses nobody.
    let non_caretaker = protocol::Message {
        reply_to_sender: Some("someone-else".into()),
        sender_principal: None,
        ..base.clone()
    };
    assert!(hub.addressed_caretakers(&non_caretaker).is_empty());
}

#[test]
fn reminder_state_exposes_progress_retry_and_caretaker_fallback() {
    assert_eq!(
        Hub::reminder_state(Some("lead"), 1, false),
        protocol::ReminderState::Running
    );
    assert_eq!(
        Hub::reminder_state(Some("lead"), 2, false),
        protocol::ReminderState::Overdue
    );
    assert_eq!(
        Hub::reminder_state(Some("loca-care"), 1, false),
        protocol::ReminderState::Stalled
    );
}

#[test]
fn stalled_goal_falls_back_to_caretaker_without_guessing_at_replacement() {
    let _clock = TEST_CLOCK_LOCK.lock().expect("clock test lock");
    let mut hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::from(["loca-care".into()]),
        },
        Arc::new(Store::open(None).expect("memory store")),
        RoomSettings {
            care_goal_secs: 1,
            care_cooldown_secs: 1,
            ..RoomSettings::default()
        },
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);
    assert!(hub.join("proj", "member:lead", "lead", SenderType::Agent, 1));
    assert!(hub.join("iye", "member:loca-care", "loca-care", SenderType::Agent, 1,));
    hub.set_lead("proj", Some("lead".into()), "operator")
        .expect("lead");
    let health = |wake: &str| RuntimeHealthUpdate {
        wake: wake.into(),
        ack: "OK".into(),
        delivery_id: None,
        attention_id: None,
        stored: false,
        accepted: false,
        first_response: false,
        final_response: false,
        turn_completed: false,
    };
    hub.report_runtime_health("lead", health("FAILED"))
        .expect("failed lead health");
    hub.report_runtime_health("loca-care", health("IDLE"))
        .expect("caretaker health");
    let (mut events, _) = hub.subscribe("proj");
    let goal = hub
        .create_goal(
            "proj",
            CreateGoal {
                outcome: "release is verified".into(),
                checkpoint: None,
                stale_after_secs: None,
                completion: GoalCompletion::Manual,
                task_ids: Vec::new(),
                by: "operator".into(),
            },
        )
        .expect("goal");
    while events.try_recv().is_ok() {}

    TEST_NOW.store(2_001, Ordering::Relaxed);
    hub.tick_care();
    let signal = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("missing stalled goal escalation: {error}"),
        }
    };
    assert_eq!(signal.reason, CareReason::GoalReminder);
    assert_eq!(signal.owner.as_deref(), Some("loca-care"));
    assert_eq!(signal.state, protocol::ReminderState::Stalled);
    assert_eq!(
        signal.subject,
        "Goal: release is verified · Lead unavailable · loca-care holding continuity"
    );
    assert_eq!(
        hub.goals("proj")
            .into_iter()
            .find(|candidate| candidate.id == goal.id)
            .expect("goal retained")
            .status,
        GoalStatus::Active,
        "caretaker escalation must not complete or cancel the goal"
    );

    while events.try_recv().is_ok() {}
    hub.report_runtime_health("lead", health("IDLE"))
        .expect("recovered lead health");
    TEST_NOW.store(3_002, Ordering::Relaxed);
    hub.tick_care();
    let recovered = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("missing recovered lead reminder: {error}"),
        }
    };
    assert_eq!(recovered.owner.as_deref(), Some("lead"));
    assert_ne!(recovered.state, protocol::ReminderState::Stalled);
    assert_eq!(recovered.subject, "Goal: release is verified");
}

#[test]
fn stalled_goal_language_waits_for_bounded_escalation() {
    let mut first = "Goal: release is verified".to_string();
    Hub::goal_caretaker_subject(&mut first, false);
    assert_eq!(
        first,
        "Goal: release is verified · Lead unavailable · loca-care holding continuity"
    );

    let mut exhausted = "Goal: release is verified".to_string();
    Hub::goal_caretaker_subject(&mut exhausted, true);
    assert_eq!(
        exhausted,
        "Goal: release is verified · Lead remains unavailable · operator review needed"
    );
}

#[test]
fn long_lived_bearer_tokens_are_full_width_csprng_values() {
    let tokens: HashSet<String> = (0..1_000).map(|_| Hub::secure_token("dv_", 32)).collect();
    assert_eq!(
        tokens.len(),
        1_000,
        "generated bearer tokens must be unique"
    );
    assert!(tokens.iter().all(|token| {
        token.len() == 67
            && token.starts_with("dv_")
            && token[3..].bytes().all(|byte| byte.is_ascii_hexdigit())
    }));
}

#[test]
fn an_active_goal_requires_a_room_lead() {
    let hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        Arc::new(Store::open(None).expect("memory store")),
        RoomSettings::default(),
        1,
    );
    let result = hub.create_goal(
        "proj",
        CreateGoal {
            outcome: "ship safely".into(),
            checkpoint: None,
            stale_after_secs: None,
            completion: GoalCompletion::Manual,
            task_ids: Vec::new(),
            by: "operator".into(),
        },
    );
    assert!(matches!(result, Err(GoalError::LeadRequired)));

    hub.set_lead("proj", Some("lead".into()), "operator")
        .expect("lead");
    let goal = hub
        .create_goal(
            "proj",
            CreateGoal {
                outcome: "ship safely".into(),
                checkpoint: None,
                stale_after_secs: None,
                completion: GoalCompletion::Manual,
                task_ids: Vec::new(),
                by: "operator".into(),
            },
        )
        .expect("goal with lead");
    hub.update_goal(
        "proj",
        goal.id,
        UpdateGoal {
            outcome: None,
            checkpoint: None,
            stale_after_secs: None,
            completion: None,
            task_ids: None,
            status: Some(GoalStatus::Cancelled),
            by: "operator".into(),
        },
    )
    .expect("cancel");
    hub.set_lead("proj", None, "operator").expect("clear lead");
    assert!(matches!(
        hub.update_goal(
            "proj",
            goal.id,
            UpdateGoal {
                outcome: None,
                checkpoint: None,
                stale_after_secs: None,
                completion: None,
                task_ids: None,
                status: Some(GoalStatus::Active),
                by: "operator".into(),
            },
        ),
        Err(GoalError::LeadRequired)
    ));
}

#[test]
fn unused_admin_pairing_expires_after_five_minutes() {
    let _clock = TEST_CLOCK_LOCK.lock().expect("clock test lock");
    let mut hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: true,
            require_invite: true,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        Arc::new(Store::open(None).expect("memory store")),
        RoomSettings::default(),
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(10_000, Ordering::Relaxed);
    let (code, expires_at) = hub
        .rotate_admin_pairing_for(Hub::ADMIN_SESSION_TTL_MS)
        .expect("pairing");
    assert_eq!(
        expires_at,
        10_000 + Hub::ADMIN_PAIRING_TTL_MS,
        "the code has its own short deadline, independent of session lifetime"
    );
    TEST_NOW.store(expires_at, Ordering::Relaxed);
    assert_eq!(
        hub.consume_admin_pairing(&code),
        None,
        "an unused code is rejected at its deadline"
    );
}

#[test]
fn goal_care_ages_from_explicit_progress_not_unrelated_chat() {
    let _clock = TEST_CLOCK_LOCK.lock().expect("clock test lock");
    let mut hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        Arc::new(Store::open(None).expect("memory store")),
        RoomSettings {
            care_goal_secs: 10,
            care_task_secs: 0,
            care_silence_secs: 0,
            care_cooldown_secs: 1,
            care_max_attempts: 2,
            ..RoomSettings::default()
        },
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);
    hub.set_lead("proj", Some("lead".into()), "operator")
        .expect("lead");
    // Tasks live on the Loca Board, which a goal closed by its tasks needs.
    hub.set_extensions(
        "proj",
        &["focus".into(), "board".into()],
        "operator",
        SenderType::User,
    )
    .expect("the Loca Board");
    let (mut events, _) = hub.subscribe("proj");
    assert!(hub.join("proj", "member:lead", "lead", SenderType::Agent, 1));
    let heartbeat = || RuntimeHealthUpdate {
        wake: "IDLE".into(),
        ack: "OK".into(),
        delivery_id: None,
        attention_id: None,
        stored: false,
        accepted: false,
        first_response: false,
        final_response: false,
        turn_completed: false,
    };
    hub.report_runtime_health("lead", heartbeat())
        .expect("runtime health");
    let task = hub
        .create_task(
            "proj",
            CreateTask {
                title: "ship the release".into(),
                from_message: None,
                assigned_to: Some("lead".into()),
                by: "operator".into(),
            },
            None,
        )
        .expect("task");
    let goal = hub
        .create_goal(
            "proj",
            CreateGoal {
                outcome: "public release is live".into(),
                checkpoint: Some("review receipt".into()),
                stale_after_secs: None,
                completion: GoalCompletion::AllTasks,
                task_ids: vec![task.id],
                by: "operator".into(),
            },
        )
        .expect("goal");
    assert_eq!(goal.progress_at, 1_000);

    // Room conversation is context, not evidence that the goal advanced.
    TEST_NOW.store(9_000, Ordering::Relaxed);
    hub.post(
        "proj",
        PostMessage {
            kind: Default::default(),
            sender: "operator".into(),
            sender_type: SenderType::User,
            target: None,
            text: "unrelated chat".into(),
            reply_to: None,
            op_id: None,
            attachments: Vec::new(),
        },
        true,
        &"operator".into(),
        None,
    )
    .expect("chat");
    while events.try_recv().is_ok() {}

    TEST_NOW.store(11_001, Ordering::Relaxed);
    hub.report_runtime_health("lead", heartbeat())
        .expect("runtime health");
    hub.tick_care();
    let first = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("goal care was reset by unrelated chat: {error}"),
        }
    };
    assert_eq!(first.reason, protocol::CareReason::GoalReminder);
    assert_eq!(first.subject, "Goal: public release is live");
    assert_eq!(first.state, protocol::ReminderState::Running);
    TEST_NOW.store(12_002, Ordering::Relaxed);
    hub.report_runtime_health("lead", heartbeat())
        .expect("runtime health");
    hub.tick_care();
    let retry = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("goal care retry missing: {error}"),
        }
    };
    assert_ne!(first.id, retry.id, "each delivery attempt has its own id");
    assert_eq!(retry.state, protocol::ReminderState::Overdue);
    assert_eq!(
        first.attention_id, retry.attention_id,
        "retries must remain one durable attention"
    );
    assert_eq!(hub.attentions("proj").len(), 1);

    // A real linked-task transition is goal progress and restarts its age.
    TEST_NOW.store(13_000, Ordering::Relaxed);
    hub.update_task(
        "proj",
        task.id,
        UpdateTask {
            status: Some(TaskStatus::Taken),
            assigned_to: None,
            note: None,
            reason: None,
            by: "lead".into(),
        },
        false,
        None,
    )
    .expect("task progress");
    let progressed_goal_at = hub.goals("proj")[0].progress_at;
    assert!(progressed_goal_at >= 13_000);
    TEST_NOW.store(14_000, Ordering::Relaxed);
    hub.update_task(
        "proj",
        task.id,
        UpdateTask {
            status: Some(TaskStatus::Taken),
            assigned_to: None,
            note: None,
            reason: None,
            by: "lead".into(),
        },
        false,
        None,
    )
    .expect("idempotent task patch");
    assert_eq!(
        hub.tasks("proj")[0].progress_at,
        13_000,
        "a no-op task patch must not manufacture progress"
    );
    assert_eq!(
        hub.goals("proj")[0].progress_at,
        progressed_goal_at,
        "a no-op task patch must not postpone goal care"
    );
    while events.try_recv().is_ok() {}

    TEST_NOW.store(progressed_goal_at + 9_999, Ordering::Relaxed);
    hub.report_runtime_health("lead", heartbeat())
        .expect("runtime health");
    hub.tick_care();
    assert!(
        !matches!(events.try_recv(), Ok(ServerFrame::Care { .. })),
        "goal reminded before its progress interval elapsed"
    );
    TEST_NOW.store(progressed_goal_at + 10_001, Ordering::Relaxed);
    hub.report_runtime_health("lead", heartbeat())
        .expect("runtime health");
    hub.tick_care();
    assert!(matches!(events.try_recv(), Ok(ServerFrame::Care { .. })));
    let attentions = hub.attentions("proj");
    assert_eq!(
        attentions.len(),
        2,
        "new explicit progress starts a new attention generation"
    );
    assert_eq!(
        attentions
            .iter()
            .filter(|attention| attention.status == AttentionStatus::Resolved)
            .count(),
        1,
        "explicit progress resolves the previous stalled condition"
    );

    // The loop remains active while even one linked task is incomplete, then
    // closes deterministically and leaves no orphan reminder behind. On the
    // Loca Board the holder's done asks for review; the operator's is final.
    TEST_NOW.store(progressed_goal_at + 11_000, Ordering::Relaxed);
    hub.update_task(
        "proj",
        task.id,
        UpdateTask {
            status: Some(TaskStatus::Done),
            assigned_to: None,
            note: None,
            reason: None,
            by: "operator".into(),
        },
        true,
        None,
    )
    .expect("finish linked task");
    assert_eq!(hub.goals("proj")[0].status, GoalStatus::Achieved);
    assert!(hub
        .attentions("proj")
        .iter()
        .all(|attention| attention.status == AttentionStatus::Resolved));
    while events.try_recv().is_ok() {}
    TEST_NOW.store(progressed_goal_at + 30_000, Ordering::Relaxed);
    hub.tick_care();
    assert!(
        !matches!(events.try_recv(), Ok(ServerFrame::Care { .. })),
        "a terminal goal must not re-arm its reminder loop"
    );
}

#[test]
fn goal_completion_conditions_are_explicit_and_non_vacuous() {
    let hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        Arc::new(Store::open(None).expect("memory store")),
        RoomSettings::default(),
        1,
    );
    hub.set_lead("proj", Some("lead".into()), "operator")
        .expect("lead");
    // Tasks live on the Loca Board, which a goal closed by its tasks needs.
    hub.set_extensions(
        "proj",
        &["focus".into(), "board".into()],
        "operator",
        SenderType::User,
    )
    .expect("the Loca Board");
    let manual = hub
        .create_goal(
            "proj",
            CreateGoal {
                outcome: "operator verifies the release".into(),
                checkpoint: Some("operator says GO".into()),
                stale_after_secs: None,
                completion: GoalCompletion::Manual,
                task_ids: Vec::new(),
                by: "operator".into(),
            },
        )
        .expect("taskless manual goal");
    assert_eq!(manual.status, GoalStatus::Active);
    hub.update_goal(
        "proj",
        manual.id,
        UpdateGoal {
            outcome: None,
            checkpoint: None,
            stale_after_secs: None,
            completion: None,
            task_ids: None,
            status: Some(GoalStatus::Cancelled),
            by: "operator".into(),
        },
    )
    .expect("operator closes manual goal");
    assert!(matches!(
        hub.create_goal(
            "proj",
            CreateGoal {
                outcome: "empty task set cannot finish itself".into(),
                checkpoint: None,
                stale_after_secs: None,
                completion: GoalCompletion::AllTasks,
                task_ids: Vec::new(),
                by: "operator".into(),
            },
        ),
        Err(GoalError::InvalidTasks)
    ));
}

#[test]
fn operator_can_route_reminders_to_one_specific_healthy_person() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let dir = tempfile::tempdir().expect("temp store");
    let db = dir.path().join("person-reminder.sqlite");
    let mut hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        Arc::new(Store::open(db.to_str()).expect("sqlite store")),
        RoomSettings {
            care_goal_secs: 1,
            care_cooldown_secs: 0,
            care_recipient: ReminderRecipient::Person {
                name: "reviewer".into(),
            },
            ..RoomSettings::default()
        },
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);
    hub.set_lead("proj", Some("lead".into()), "operator")
        .expect("lead");
    let (mut events, _) = hub.subscribe("proj");
    assert!(hub.join("proj", "member:lead", "lead", SenderType::Agent, 1));
    assert!(hub.join("proj", "member:reviewer", "reviewer", SenderType::Agent, 1));
    let heartbeat = || RuntimeHealthUpdate {
        wake: "IDLE".into(),
        ack: "OK".into(),
        delivery_id: None,
        attention_id: None,
        stored: false,
        accepted: false,
        first_response: false,
        final_response: false,
        turn_completed: false,
    };
    hub.report_runtime_health("lead", heartbeat())
        .expect("lead health");
    hub.report_runtime_health("reviewer", heartbeat())
        .expect("reviewer health");
    let goal = hub
        .create_goal(
            "proj",
            CreateGoal {
                outcome: "publish safely".into(),
                checkpoint: None,
                stale_after_secs: None,
                completion: GoalCompletion::Manual,
                task_ids: Vec::new(),
                by: "operator".into(),
            },
        )
        .expect("goal");
    while events.try_recv().is_ok() {}

    TEST_NOW.store(goal.progress_at + 1_001, Ordering::Relaxed);
    hub.tick_care();
    let signal = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("missing person reminder: {error}"),
        }
    };
    assert_eq!(signal.owner.as_deref(), Some("reviewer"));
    assert_eq!(
        signal.audience,
        AttentionAudience::Person {
            name: "reviewer".into()
        }
    );
    assert_eq!(hub.pending_care("proj", "reviewer").len(), 1);
    assert!(hub.pending_care("proj", "lead").is_empty());

    let mut rooms = hub.rooms.lock().expect("rooms");
    let room = rooms.get_mut("proj").expect("room");
    room.settings.care_recipient = ReminderRecipient::All;
    let all_signal = hub.make_care_signal(
        "proj",
        room,
        CareDraft {
            attention_key: "goal:all:1".into(),
            owner: Some("lead".into()),
            owner_principal_id: None,
            group: None,
            reason: CareReason::GoalReminder,
            target: None,
            participants: Vec::new(),
            subject: "whole-loca reminder".into(),
            attempt: 1,
            at: 2_000,
            escalated: false,
        },
    );
    assert_eq!(
        all_signal.audience,
        AttentionAudience::Group {
            names: vec!["lead".into(), "reviewer".into()]
        }
    );
}

// ---- Everyone (durable multi-recipient) reminder fan-out ----

/// Build a hub whose new rooms default to an `Everyone` room-silence reminder,
/// and seat a canonical roster of `names` (each admitted — which mints a
/// principal — and invited into `room`). Returns the hub with a controllable
/// clock already at `t=1_000`.
fn everyone_hub_from_store(store: Arc<Store>) -> Hub {
    let mut hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        store,
        RoomSettings {
            care_silence_secs: 1,
            care_cooldown_secs: 0,
            care_max_attempts: 3,
            care_recipient: ReminderRecipient::All,
            ..RoomSettings::default()
        },
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);
    hub
}

fn everyone_hub(db: &std::path::Path, room: &str, names: &[&str]) -> Hub {
    let hub = everyone_hub_from_store(Arc::new(Store::open(db.to_str()).expect("sqlite store")));
    hub.create_loca(room, &hub.resolve_authority(Some("MASTER"), None), None)
        .expect("explicit reminder fixture loca");
    for name in names {
        let member = hub.admit_member(name, "agent", "operator").expect("admit");
        hub.invite_member_to_room(&member.id, room, "operator")
            .expect("invite");
    }
    hub
}

fn arm_silence(hub: &Hub, room: &str, sender: &str) {
    hub.post(
        room,
        PostMessage {
            kind: Default::default(),
            sender: sender.into(),
            sender_type: SenderType::Agent,
            target: None,
            text: "hi".into(),
            reply_to: None,
            op_id: None,
            attachments: Vec::new(),
        },
        true,
        &sender.into(),
        None,
    )
    .expect("seed message");
}

fn silence_attentions(hub: &Hub, room: &str) -> Vec<(String, protocol::Attention)> {
    let rooms = hub.rooms.lock().expect("rooms");
    let r = rooms.get(room).expect("room");
    let mut out: Vec<(String, protocol::Attention)> = r
        .attentions
        .iter()
        .filter(|(id, _)| id.contains(":silence:"))
        .map(|(id, a)| (id.clone(), a.clone()))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[test]
fn everyone_reminder_fans_out_to_a_durable_attention_per_member() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let dir = tempfile::tempdir().expect("temp store");
    let hub = everyone_hub(
        &dir.path().join("fanout.sqlite"),
        "proj",
        &["alice", "bob", "carol"],
    );
    arm_silence(&hub, "proj", "alice");

    TEST_NOW.store(5_000, Ordering::Relaxed);
    hub.tick_care();

    let silence = silence_attentions(&hub, "proj");
    assert_eq!(
        silence.len(),
        3,
        "one attention per roster member, not one lead"
    );
    let mut owners: Vec<String> = silence
        .iter()
        .filter_map(|(_, a)| a.owner.clone())
        .collect();
    owners.sort();
    assert_eq!(owners, vec!["alice", "bob", "carol"]);
    // Every per-member attention shares the silence generation prefix (so the
    // durable + in-memory prefix resolvers close them together) and carries the
    // group audience (so Chat renders a single @all).
    let mut groups = std::collections::HashSet::new();
    for (id, attention) in &silence {
        assert!(
            id.starts_with("attention:proj:silence:"),
            "shared generation prefix: {id}"
        );
        // Each per-member signal is addressed to ONE person (no group audience →
        // no N×N wake); the generation identity for the single @all lives in
        // `group`, shared across members and carrying no secret token.
        assert!(
            matches!(attention.audience, AttentionAudience::Person { .. }),
            "per-member Person audience, not a group broadcast"
        );
        let group = attention
            .group
            .clone()
            .expect("per-member attention carries a group id");
        assert!(
            !group.contains("mb_") && !group.contains("dv_"),
            "group id is secret-free"
        );
        groups.insert(group);
    }
    assert_eq!(groups.len(), 1, "all members share one generation group id");
}

#[test]
fn everyone_reminder_reaches_offline_members_via_the_durable_outbox() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let dir = tempfile::tempdir().expect("temp store");
    // Nobody joins — every member is offline. They must still each get a durable
    // care_outbox row so the reminder is delivered on their first reconnect.
    let hub = everyone_hub(
        &dir.path().join("offline.sqlite"),
        "proj",
        &["alice", "bob", "carol"],
    );
    arm_silence(&hub, "proj", "alice");
    TEST_NOW.store(5_000, Ordering::Relaxed);
    hub.tick_care();

    for name in ["alice", "bob", "carol"] {
        let pending = hub.store.pending_care("proj", name);
        assert_eq!(
            pending.len(),
            1,
            "{name} (offline) has one durable reminder queued"
        );
        assert_eq!(pending[0].reason, CareReason::RoomSilence);
    }
}

#[test]
fn everyone_reminder_is_all_or_nothing_when_a_principal_cannot_resolve() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let dir = tempfile::tempdir().expect("temp store");
    let db = dir.path().join("all-or-nothing.sqlite");
    let hub = everyone_hub(&db, "proj", &["alice", "bob", "carol"]);
    arm_silence(&hub, "proj", "alice");

    // Break bob's principal resolution while his invite + membership stay active:
    // his membership record no longer names the principal it was bound to
    // (the store's row cannot lose it: `members.principal_id` is NOT NULL and
    // a foreign key). The roster is now inconsistent.
    {
        let bob = hub.member_by_name("bob").expect("bob").id;
        hub.members
            .lock()
            .expect("members")
            .get_mut(&bob)
            .expect("bob's membership")
            .principal_id = None;
    }

    TEST_NOW.store(5_000, Ordering::Relaxed);
    hub.tick_care();

    // All or nothing: not one attention, not one outbox row, and the mark is
    // untouched so the scheduler retries — never a partial "Everyone".
    assert!(
        silence_attentions(&hub, "proj").is_empty(),
        "a single unresolvable principal aborts the whole fan-out"
    );
    for name in ["alice", "carol"] {
        assert!(
            hub.store.pending_care("proj", name).is_empty(),
            "{name} got no outbox row from the aborted generation"
        );
    }
    let rooms = hub.rooms.lock().expect("rooms");
    assert!(
        !rooms
            .get("proj")
            .expect("room")
            .care_marks
            .contains_key("silence"),
        "the care mark did not advance"
    );
}

#[test]
fn everyone_reminder_retry_re_delivers_without_duplicating() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let dir = tempfile::tempdir().expect("temp store");
    let hub = everyone_hub(
        &dir.path().join("retry.sqlite"),
        "proj",
        &["alice", "bob", "carol"],
    );
    arm_silence(&hub, "proj", "alice");

    TEST_NOW.store(5_000, Ordering::Relaxed);
    hub.tick_care();
    assert_eq!(silence_attentions(&hub, "proj").len(), 3);

    // A later sweep re-delivers to the SAME generation; the per-member attention
    // ids are stable, so no new attention is created.
    TEST_NOW.store(9_000, Ordering::Relaxed);
    hub.tick_care();
    assert_eq!(
        silence_attentions(&hub, "proj").len(),
        3,
        "retry re-delivers to the same generation, never duplicates"
    );
}

#[test]
fn a_room_message_resolves_every_everyone_attention_together() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let dir = tempfile::tempdir().expect("temp store");
    let hub = everyone_hub(
        &dir.path().join("resolve.sqlite"),
        "proj",
        &["alice", "bob", "carol"],
    );
    arm_silence(&hub, "proj", "alice");
    TEST_NOW.store(5_000, Ordering::Relaxed);
    hub.tick_care();
    assert_eq!(silence_attentions(&hub, "proj").len(), 3);

    // A new room message breaks the silence: every per-member attention of that
    // generation resolves together (the prefix resolver), not just one.
    TEST_NOW.store(6_000, Ordering::Relaxed);
    arm_silence(&hub, "proj", "bob");
    let silence = silence_attentions(&hub, "proj");
    assert_eq!(silence.len(), 3);
    assert!(
        silence
            .iter()
            .all(|(_, a)| a.status == AttentionStatus::Resolved),
        "a room message resolves all Everyone attentions, not one"
    );
}

#[test]
fn everyone_roster_drops_a_revoked_member() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let dir = tempfile::tempdir().expect("temp store");
    let hub = everyone_hub(
        &dir.path().join("revoke.sqlite"),
        "proj",
        &["alice", "bob", "carol"],
    );
    assert_eq!(
        hub.everyone_recipients("proj").expect("full roster").len(),
        3
    );

    let carol = hub.member_by_name("carol").expect("carol").id;
    hub.revoke_member(&carol).expect("revoke carol");

    let roster = hub
        .everyone_recipients("proj")
        .expect("roster after revoke");
    assert_eq!(
        roster.len(),
        2,
        "a revoked member leaves the Everyone roster"
    );
    assert!(roster.iter().all(|(_, name)| name != "carol"));
}

#[test]
fn everyone_reconnect_replay_drops_a_revoked_principals_row() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let dir = tempfile::tempdir().expect("temp store");
    // Everyone is offline, so the fan-out writes a durable outbox row per member.
    let hub = everyone_hub(
        &dir.path().join("revoke-replay.sqlite"),
        "proj",
        &["alice", "carol"],
    );
    arm_silence(&hub, "proj", "alice");
    TEST_NOW.store(5_000, Ordering::Relaxed);
    hub.tick_care();

    // carol's canonical principal, from the pre-revoke roster.
    let carol_pid = hub
        .everyone_recipients("proj")
        .expect("roster")
        .into_iter()
        .find(|(_, name)| name == "carol")
        .expect("carol on roster")
        .0;

    // Before revoke: carol's durable row replays to her principal exactly once.
    assert_eq!(
        hub.pending_care_scoped("proj", "carol", Some(&carol_pid))
            .len(),
        1,
        "carol's enqueued reminder replays before revoke"
    );

    // Revoke carol's invite AFTER her row was already enqueued.
    hub.revoke_invites_for("proj", "carol")
        .expect("revoke carol's invite");

    // The principal-scoped replay re-checks the LIVE roster and drops the stale
    // row: a revoked principal receives zero on reconnect.
    assert!(
        hub.pending_care_scoped("proj", "carol", Some(&carol_pid))
            .is_empty(),
        "a revoked principal's enqueued reminder is dropped on reconnect"
    );
    // The durable receipt is retained UNACKNOWLEDGED (audit truth) — it is
    // filtered at delivery, never fake-ACK'd.
    assert_eq!(
        hub.store.pending_care("proj", "carol").len(),
        1,
        "the outbox row is retained unacknowledged, not fake-ACK'd"
    );
}

#[test]
fn everyone_treats_two_same_named_principals_as_distinct_recipients() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let dir = tempfile::tempdir().expect("temp store");
    let db = dir.path().join("same-name.sqlite");
    let hub = everyone_hub_from_store(Arc::new(Store::open(db.to_str()).expect("store")));
    hub.create_loca("proj", &hub.resolve_authority(Some("MASTER"), None), None)
        .expect("explicit same-name fixture loca");
    // Two distinct members with distinct canonical principals, both invited into
    // the same room.
    let dave = hub.admit_member("dave", "agent", "operator").expect("dave");
    let other = hub
        .admit_member("dave-two", "agent", "operator")
        .expect("other");
    hub.invite_member_to_room(&dave.id, "proj", "operator")
        .expect("invite dave");
    hub.invite_member_to_room(&other.id, "proj", "operator")
        .expect("invite other");
    // Rename the second member to the SAME display name — in the store and the
    // in-memory roster — so two distinct principals now share a display name.
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        conn.execute(
            "UPDATE members SET name = 'dave' WHERE id = ?1",
            rusqlite::params![other.id],
        )
        .unwrap();
    }
    hub.members
        .lock()
        .expect("members")
        .get_mut(&other.id)
        .expect("other membership")
        .name = "dave".into();

    // Deduped by canonical principal, never by name → two recipients, both "dave".
    let roster = hub.everyone_recipients("proj").expect("roster");
    assert_eq!(roster.len(), 2, "two principals, same name, two recipients");
    assert!(roster.iter().all(|(_, name)| name == "dave"));
    let principals: HashSet<String> = roster.iter().map(|(pid, _)| pid.clone()).collect();
    assert_eq!(principals.len(), 2, "distinct canonical principals");

    arm_silence(&hub, "proj", "dave");
    TEST_NOW.store(5_000, Ordering::Relaxed);
    hub.tick_care();
    let silence = silence_attentions(&hub, "proj");
    assert_eq!(
        silence.len(),
        2,
        "one attention per principal, not merged by name"
    );
    let ids: HashSet<String> = silence.iter().map(|(id, _)| id.clone()).collect();
    assert_eq!(ids.len(), 2, "attention ids differ by principal");
    assert!(silence
        .iter()
        .all(|(_, a)| a.owner.as_deref() == Some("dave")));

    // Each same-named principal has its OWN durable outbox row, and the ACK is
    // gated by the authenticated principal — never the shared display name.
    let pids: Vec<String> = roster.iter().map(|(pid, _)| pid.clone()).collect();
    let care_a = hub
        .pending_care_scoped("proj", "dave", Some(&pids[0]))
        .pop()
        .expect("principal a has its own row");
    let care_b = hub
        .pending_care_scoped("proj", "dave", Some(&pids[1]))
        .pop()
        .expect("principal b has its own row");
    let sig_a = care_a.id.clone();
    let sig_b = care_b.id.clone();
    assert_ne!(sig_a, sig_b, "each principal's delivery is distinct");
    assert!(
        matches!(
            hub.resolve_attention("proj", &care_a.attention_id, "dave", Some(&pids[1]), false),
            Err(AttentionError::Forbidden)
        ),
        "a same-named principal cannot resolve another principal's attention"
    );
    assert!(
        matches!(
            hub.resolve_attention("proj", &care_a.attention_id, "dave", None, false),
            Err(AttentionError::Forbidden)
        ),
        "a principal-bound attention cannot be resolved by name alone"
    );
    // The OTHER same-named principal cannot ACK this row.
    assert!(
        !hub.ack_care(&sig_a, "dave", Some(&pids[1]))
            .expect("cross-principal ack"),
        "a same-named principal cannot ACK another's delivery"
    );
    // A legacy/name-only ACK (no authenticated principal) cannot ACK an
    // Everyone per-member row either — the reminder demands the principal.
    assert!(
        !hub.ack_care(&sig_a, "dave", None).expect("name-only ack"),
        "a principal-bound row is never ACK'd by display name alone"
    );
    // Its own principal ACKs it; the other principal's delivery stays pending.
    assert!(
        hub.ack_care(&sig_a, "dave", Some(&pids[0]))
            .expect("own ack a"),
        "the owning principal ACKs its own delivery"
    );
    assert!(
        hub.pending_care_scoped("proj", "dave", Some(&pids[0]))
            .is_empty(),
        "principal a's row is now acknowledged"
    );
    assert_eq!(
        hub.pending_care_scoped("proj", "dave", Some(&pids[1]))
            .len(),
        1,
        "principal b's delivery is untouched by a's ACK"
    );
    assert!(
        hub.ack_care(&sig_b, "dave", Some(&pids[1]))
            .expect("own ack b"),
        "principal b independently ACKs its own delivery"
    );
    assert_eq!(
        hub.resolve_attention("proj", &care_a.attention_id, "dave", Some(&pids[0]), false)
            .expect("owner resolves its own attention")
            .status,
        AttentionStatus::Resolved
    );

    // Revoking one same-named member drops only its own delivery; the other stays.
    hub.revoke_member(&dave.id).expect("revoke dave");
    let after = hub
        .everyone_recipients("proj")
        .expect("roster after revoke");
    assert_eq!(
        after.len(),
        1,
        "revoking one same-named member leaves the other"
    );
}

#[test]
fn a_legacy_invite_binds_to_its_principal_and_enters_the_roster_once() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let dir = tempfile::tempdir().expect("temp store");
    let db = dir.path().join("legacy.sqlite");
    let store = Arc::new(Store::open(db.to_str()).expect("store"));
    // First boot: admit erin (membership + principal), then seed a LEGACY invite
    // that predates the member link — empty `member`, carrying only her name.
    let erin = {
        let hub = everyone_hub_from_store(store.clone());
        let erin = hub.admit_member("erin", "agent", "operator").expect("erin");
        let legacy = InviteRecord::issued(
            "dv_legacy_erin",
            "proj",
            "",
            "erin",
            "agent",
            1_000,
            "operator",
        );
        hub.store
            .issue_invite(&legacy, "dv_legacy_erin")
            .expect("seed legacy invite");
        erin
    };
    // Second boot: the load-time migration binds the legacy invite to erin's
    // membership by name, so she resolves to her real canonical principal —
    // exactly once, never a fabricated or duplicate identity.
    let hub = everyone_hub_from_store(store);
    let roster = hub.everyone_recipients("proj").expect("roster");
    assert_eq!(
        roster.len(),
        1,
        "the migrated legacy invite enters the roster once"
    );
    assert_eq!(roster[0].1, "erin");
    assert_eq!(
        roster[0].0,
        erin.principal_id.clone().expect("erin principal"),
        "bound to erin's real canonical principal"
    );
}

#[test]
fn replacing_wait_resolves_old_attention_and_starts_a_new_generation() {
    let _clock = TEST_CLOCK_LOCK.lock().expect("clock test lock");
    let dir = tempfile::tempdir().expect("temp store");
    let db = dir.path().join("wait-attention.sqlite");
    let mut hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        Arc::new(Store::open(db.to_str()).expect("sqlite store")),
        RoomSettings {
            care_wait_secs: 1,
            care_cooldown_secs: 1,
            care_max_attempts: 2,
            ..RoomSettings::default()
        },
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);
    hub.set_lead("proj", Some("lead".into()), "operator")
        .expect("lead");
    let (mut events, _) = hub.subscribe("proj");
    assert!(hub.join("proj", "member:lead", "lead", SenderType::Agent, 1));
    let heartbeat = || RuntimeHealthUpdate {
        wake: "IDLE".into(),
        ack: "OK".into(),
        delivery_id: None,
        attention_id: None,
        stored: false,
        accepted: false,
        first_response: false,
        final_response: false,
        turn_completed: false,
    };
    hub.report_runtime_health("lead", heartbeat())
        .expect("runtime health");
    hub.set_wait(
        "proj",
        SetWait {
            by: "worker".into(),
            waiting_for: "reviewer".into(),
            reason: "needs review".into(),
        },
    )
    .expect("first wait");
    while events.try_recv().is_ok() {}

    TEST_NOW.store(2_001, Ordering::Relaxed);
    hub.tick_care();
    let first = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("wait attention missing: {error}"),
        }
    };
    assert_eq!(hub.pending_care("proj", "lead").len(), 1);

    TEST_NOW.store(3_000, Ordering::Relaxed);
    hub.set_wait(
        "proj",
        SetWait {
            by: "worker".into(),
            waiting_for: "security".into(),
            reason: "needs security review".into(),
        },
    )
    .expect("replacement wait");
    let old = hub
        .attentions("proj")
        .into_iter()
        .find(|attention| attention.id == first.attention_id)
        .expect("old attention retained as history");
    assert_eq!(old.status, AttentionStatus::Resolved);
    assert!(hub.pending_care("proj", "lead").is_empty());

    while events.try_recv().is_ok() {}
    TEST_NOW.store(4_001, Ordering::Relaxed);
    hub.report_runtime_health("lead", heartbeat())
        .expect("runtime health");
    hub.tick_care();
    let next = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("replacement wait attention missing: {error}"),
        }
    };
    assert_ne!(next.attention_id, first.attention_id);
}

#[test]
fn attention_defaults_to_lead_and_delivery_ack_does_not_resolve_it() {
    let _clock = TEST_CLOCK_LOCK.lock().expect("clock test lock");
    let mut hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        Arc::new(Store::open(None).expect("memory store")),
        RoomSettings::default(),
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);
    hub.set_lead("proj", Some("lead".into()), "operator")
        .expect("lead");
    let (mut events, _) = hub.subscribe("proj");

    let attention = hub
        .create_attention(
            "proj",
            CreateAttention {
                subject: "review the release gate".into(),
                audience: AttentionAudience::Lead,
                by: "operator".into(),
            },
        )
        .expect("attention");
    assert_eq!(attention.owner.as_deref(), Some("lead"));
    assert_eq!(attention.status, AttentionStatus::Open);
    let delivery_id = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal.id,
            Ok(_) => continue,
            Err(error) => panic!("attention delivery missing: {error}"),
        }
    };
    assert!(!hub
        .ack_care("unknown-delivery", "lead", None)
        .expect("unknown ack"));
    assert!(!hub
        .ack_care(&delivery_id, "intruder", None)
        .expect("wrong-owner ack"));

    TEST_NOW.store(2_000, Ordering::Relaxed);
    let claimed = hub
        .claim_attention("proj", &attention.id, "lead")
        .expect("claim");
    assert_eq!(claimed.status, AttentionStatus::Claimed);
    assert_eq!(claimed.claimed_by.as_deref(), Some("lead"));

    TEST_NOW.store(3_000, Ordering::Relaxed);
    assert!(hub
        .ack_care(&delivery_id, "lead", None)
        .expect("delivery ack"));
    assert!(!hub
        .ack_care(&delivery_id, "lead", None)
        .expect("duplicate delivery ack"));
    let after_ack = hub.attentions("proj").pop().expect("attention remains");
    assert_eq!(after_ack.delivered_at, Some(3_000));
    assert_eq!(
        after_ack.status,
        AttentionStatus::Claimed,
        "runtime delivery must not manufacture work completion"
    );

    TEST_NOW.store(4_000, Ordering::Relaxed);
    let resolved = hub
        .resolve_attention("proj", &attention.id, "lead", None, false)
        .expect("resolve");
    assert_eq!(resolved.status, AttentionStatus::Resolved);
    assert_eq!(resolved.resolved_at, Some(4_000));
}

#[test]
fn claimed_attention_keeps_one_owner_when_retry_fallback_changes() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let dir = tempfile::tempdir().expect("temp store");
    let db = dir.path().join("claimed-owner.sqlite");
    let mut hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::from(["loca-care".to_string()]),
        },
        Arc::new(Store::open(db.to_str()).expect("sqlite store")),
        RoomSettings {
            care_cooldown_secs: 1,
            care_max_attempts: 2,
            care_goal_secs: 1,
            ..RoomSettings::default()
        },
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);
    hub.set_lead("proj", Some("lead".into()), "operator")
        .expect("lead");
    let (mut project_events, _) = hub.subscribe("proj");
    let (mut home_events, _) = hub.subscribe("iye");
    assert!(hub.join("proj", "member:lead", "lead", SenderType::Agent, 1));
    assert!(hub.join("iye", "member:loca-care", "loca-care", SenderType::Agent, 1));
    let health = |wake: &str| RuntimeHealthUpdate {
        wake: wake.into(),
        ack: "OK".into(),
        delivery_id: None,
        attention_id: None,
        stored: false,
        accepted: false,
        first_response: false,
        final_response: false,
        turn_completed: false,
    };
    hub.report_runtime_health("lead", health("IDLE"))
        .expect("lead health");
    hub.report_runtime_health("loca-care", health("IDLE"))
        .expect("caretaker health");
    let goal = hub
        .create_goal(
            "proj",
            CreateGoal {
                outcome: "one owner".into(),
                checkpoint: None,
                stale_after_secs: None,
                completion: GoalCompletion::Manual,
                task_ids: Vec::new(),
                by: "operator".into(),
            },
        )
        .expect("goal");
    while project_events.try_recv().is_ok() {}
    while home_events.try_recv().is_ok() {}

    TEST_NOW.store(goal.progress_at + 1_001, Ordering::Relaxed);
    hub.tick_care();
    let first = loop {
        match project_events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("missing lead delivery: {error}"),
        }
    };
    assert_eq!(first.owner.as_deref(), Some("lead"));
    hub.claim_attention("proj", &first.attention_id, "lead")
        .expect("lead claim");
    hub.report_runtime_health("lead", health("FAILED"))
        .expect("lead failure health");
    while project_events.try_recv().is_ok() {}
    while home_events.try_recv().is_ok() {}

    TEST_NOW.store(goal.progress_at + 3_000, Ordering::Relaxed);
    hub.report_runtime_health("loca-care", health("IDLE"))
        .expect("fresh caretaker health");
    hub.tick_care();
    assert!(
        !matches!(home_events.try_recv(), Ok(ServerFrame::Care { .. })),
        "claimed work must not be reassigned to the fallback caretaker"
    );
    assert!(hub.pending_care("iye", "loca-care").is_empty());
    let claimed = hub
        .attentions("proj")
        .into_iter()
        .find(|attention| attention.id == first.attention_id)
        .expect("claimed attention");
    assert_eq!(claimed.status, AttentionStatus::Claimed);
    assert_eq!(claimed.owner.as_deref(), Some("lead"));
    assert_eq!(claimed.claimed_by.as_deref(), Some("lead"));
}

#[test]
fn group_attention_selects_exactly_one_healthy_claimant() {
    let _clock = TEST_CLOCK_LOCK.lock().expect("clock test lock");
    let hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        Arc::new(Store::open(None).expect("memory store")),
        RoomSettings::default(),
        1,
    );
    let _ = hub.subscribe("proj");
    assert!(hub.join("proj", "member:healthy", "healthy", SenderType::Agent, 1));
    hub.report_runtime_health(
        "healthy",
        RuntimeHealthUpdate {
            wake: "IDLE".into(),
            ack: "OK".into(),
            delivery_id: None,
            attention_id: None,
            stored: false,
            accepted: false,
            first_response: false,
            final_response: false,
            turn_completed: false,
        },
    )
    .expect("runtime health");
    hub.report_runtime_health(
        "awake-but-not-seated",
        RuntimeHealthUpdate {
            wake: "IDLE".into(),
            ack: "OK".into(),
            delivery_id: None,
            attention_id: None,
            stored: false,
            accepted: false,
            first_response: false,
            final_response: false,
            turn_completed: false,
        },
    )
    .expect("unseated runtime health");
    let attention = hub
        .create_attention(
            "proj",
            CreateAttention {
                subject: "one reviewer must inspect this".into(),
                audience: AttentionAudience::Group {
                    names: vec![
                        "offline".into(),
                        "healthy".into(),
                        "healthy".into(),
                        "awake-but-not-seated".into(),
                    ],
                },
                by: "operator".into(),
            },
        )
        .expect("group attention");
    assert_eq!(attention.owner.as_deref(), Some("healthy"));
    assert_eq!(
        attention.participants,
        vec!["awake-but-not-seated", "healthy", "offline"]
    );
}

#[test]
fn same_millisecond_care_conditions_keep_distinct_delivery_receipts() {
    let dir = tempfile::tempdir().expect("temp store");
    let db = dir.path().join("care-delivery.sqlite");
    let store = Arc::new(Store::open(db.to_str()).expect("sqlite store"));
    let hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        store.clone(),
        RoomSettings::default(),
        1,
    );
    let _ = hub.subscribe("proj");
    let rooms = hub.rooms.lock().expect("rooms");
    let room = rooms.get("proj").expect("room");
    let signal = |attention_key: &str| {
        hub.make_care_signal(
            "proj",
            room,
            CareDraft {
                attention_key: attention_key.into(),
                owner: Some("lead".into()),
                owner_principal_id: None,
                group: None,
                reason: CareReason::WaitOverdue,
                target: None,
                participants: Vec::new(),
                subject: attention_key.into(),
                attempt: 1,
                at: 1_000,
                escalated: false,
            },
        )
    };
    let first = signal("wait:a:1");
    let second = signal("wait:b:1");
    drop(rooms);

    assert_ne!(first.id, second.id);
    store.enqueue_care("proj", &first).expect("first enqueue");
    store.enqueue_care("proj", &second).expect("second enqueue");
    assert_eq!(
        store.pending_care("proj", "lead").len(),
        2,
        "neither delivery may disappear behind an outbox primary-key collision"
    );
}

#[test]
fn wait_cycle_survives_missing_owner_and_uses_bounded_scheduler() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let dir = tempfile::tempdir().expect("temp store");
    let db = dir.path().join("cycle-scheduler.sqlite");
    let mut hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        Arc::new(Store::open(db.to_str()).expect("sqlite store")),
        RoomSettings {
            care_wait_secs: 600,
            care_cooldown_secs: 10,
            care_max_attempts: 1,
            ..RoomSettings::default()
        },
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);
    for (waiter, waiting_for) in [("a", "b"), ("b", "a")] {
        hub.set_wait(
            "proj",
            SetWait {
                by: waiter.into(),
                waiting_for: waiting_for.into(),
                reason: "cycle".into(),
            },
        )
        .expect("wait");
    }
    hub.tick_care();
    assert!(hub.attentions("proj").is_empty());

    hub.set_lead("proj", Some("lead".into()), "operator")
        .expect("lead");
    let (mut events, _) = hub.subscribe("proj");
    assert!(hub.join("proj", "member:lead", "lead", SenderType::Agent, 1));
    hub.report_runtime_health(
        "lead",
        RuntimeHealthUpdate {
            wake: "IDLE".into(),
            ack: "OK".into(),
            delivery_id: None,
            attention_id: None,
            stored: false,
            accepted: false,
            first_response: false,
            final_response: false,
            turn_completed: false,
        },
    )
    .expect("health");
    hub.tick_care();
    let first = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("missing cycle signal: {error}"),
        }
    };
    assert_eq!(first.reason, CareReason::WaitCycle);
    assert_eq!(first.attempt, 1);

    // Reposting the exact edge is a no-op and cannot manufacture a wake.
    hub.set_wait(
        "proj",
        SetWait {
            by: "b".into(),
            waiting_for: "a".into(),
            reason: "cycle".into(),
        },
    )
    .expect("idempotent wait");
    TEST_NOW.store(5_000, Ordering::Relaxed);
    hub.tick_care();
    assert!(!matches!(events.try_recv(), Ok(ServerFrame::Care { .. })));

    TEST_NOW.store(12_000, Ordering::Relaxed);
    hub.tick_care();
    let escalation = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("missing bounded escalation: {error}"),
        }
    };
    assert_eq!(escalation.attempt, 2);
    assert!(escalation.escalated);
    TEST_NOW.store(13_000, Ordering::Relaxed);
    hub.tick_care();
    assert!(!matches!(events.try_recv(), Ok(ServerFrame::Care { .. })));
}

#[test]
fn reminder_is_recorded_and_published_before_a_healthy_owner_exists() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let dir = tempfile::tempdir().expect("temp store");
    let mut hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        Arc::new(Store::open(dir.path().join("ownerless.sqlite").to_str()).expect("store")),
        RoomSettings {
            care_cooldown_secs: 0,
            care_max_attempts: 3,
            care_silence_secs: 1,
            care_recipient: ReminderRecipient::Lead,
            ..RoomSettings::default()
        },
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);
    hub.set_lead("proj", Some("lead".into()), "operator")
        .expect("lead");
    assert!(hub.join("proj", "member:lead", "lead", SenderType::Agent, 1));
    let (mut events, _) = hub.subscribe("proj");
    let first_message = hub
        .post(
            "proj",
            PostMessage {
                kind: Default::default(),
                sender: "operator".into(),
                sender_type: SenderType::User,
                target: None,
                text: "start silence clock".into(),
                reply_to: None,
                op_id: None,
                attachments: Vec::new(),
            },
            true,
            &"operator".into(),
            None,
        )
        .expect("seed message");
    while events.try_recv().is_ok() {}

    TEST_NOW.store(first_message.ts + 1_001, Ordering::Relaxed);
    hub.tick_care();

    let attention = hub
        .attentions("proj")
        .into_iter()
        .find(|attention| attention.reason == CareReason::RoomSilence)
        .expect("room-visible reminder exists without a healthy owner");
    assert_eq!(attention.owner, None);
    assert_eq!(attention.delivered_at, None);
    assert!(hub.pending_care("proj", "lead").is_empty());
    let mut published = false;
    while let Ok(frame) = events.try_recv() {
        if matches!(
            frame,
            ServerFrame::Attention { attention }
                if attention.reason == CareReason::RoomSilence && attention.owner.is_none()
        ) {
            published = true;
        }
    }
    assert!(published, "the room receives the owner-less reminder state");

    hub.report_runtime_health(
        "lead",
        RuntimeHealthUpdate {
            wake: "IDLE".into(),
            ack: "OK".into(),
            delivery_id: None,
            attention_id: None,
            stored: false,
            accepted: false,
            first_response: false,
            final_response: false,
            turn_completed: false,
        },
    )
    .expect("runtime health");
    TEST_NOW.store(first_message.ts + 2_001, Ordering::Relaxed);
    hub.tick_care();
    let recovered = hub
        .attentions("proj")
        .into_iter()
        .find(|candidate| candidate.id == attention.id)
        .expect("same reminder generation survives owner recovery");
    assert_eq!(recovered.owner.as_deref(), Some("lead"));
    assert_eq!(recovered.attempt, 1);
    assert_eq!(hub.pending_care("proj", "lead").len(), 1);
}

#[test]
fn accepted_chat_retires_old_silence_attention_and_starts_a_new_generation() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let dir = tempfile::tempdir().expect("temp store");
    let db = dir.path().join("silence-generation.sqlite");
    let mut hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        Arc::new(Store::open(db.to_str()).expect("sqlite store")),
        RoomSettings {
            care_cooldown_secs: 0,
            care_max_attempts: 1,
            care_silence_secs: 1,
            ..RoomSettings::default()
        },
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);
    hub.set_lead("proj", Some("lead".into()), "operator")
        .expect("lead");
    let (mut events, _) = hub.subscribe("proj");
    assert!(hub.join("proj", "member:lead", "lead", SenderType::Agent, 1));
    let heartbeat = || RuntimeHealthUpdate {
        wake: "IDLE".into(),
        ack: "OK".into(),
        delivery_id: None,
        attention_id: None,
        stored: false,
        accepted: false,
        first_response: false,
        final_response: false,
        turn_completed: false,
    };
    hub.report_runtime_health("lead", heartbeat())
        .expect("runtime health");
    let first_message = hub
        .post(
            "proj",
            PostMessage {
                kind: Default::default(),
                sender: "operator".into(),
                sender_type: SenderType::User,
                target: None,
                text: "first".into(),
                reply_to: None,
                op_id: None,
                attachments: Vec::new(),
            },
            true,
            &"operator".into(),
            None,
        )
        .expect("first message");
    while events.try_recv().is_ok() {}

    TEST_NOW.store(first_message.ts + 1_001, Ordering::Relaxed);
    hub.tick_care();
    let first_signal = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("missing first silence signal: {error}"),
        }
    };
    assert_eq!(first_signal.reason, CareReason::RoomSilence);
    assert_eq!(hub.pending_care("proj", "lead").len(), 1);

    TEST_NOW.store(first_message.ts + 2_000, Ordering::Relaxed);
    let second_message = hub
        .post(
            "proj",
            PostMessage {
                kind: Default::default(),
                sender: "operator".into(),
                sender_type: SenderType::User,
                target: None,
                text: "progress".into(),
                reply_to: None,
                op_id: None,
                attachments: Vec::new(),
            },
            true,
            &"operator".into(),
            None,
        )
        .expect("progress message");
    let retired = hub
        .attentions("proj")
        .into_iter()
        .find(|attention| attention.id == first_signal.attention_id)
        .expect("old silence attention retained as history");
    assert_eq!(retired.status, AttentionStatus::Resolved);
    assert!(hub.pending_care("proj", "lead").is_empty());

    while events.try_recv().is_ok() {}
    TEST_NOW.store(second_message.ts + 1_001, Ordering::Relaxed);
    hub.report_runtime_health("lead", heartbeat())
        .expect("refreshed runtime health");
    hub.tick_care();
    let next_signal = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("missing next silence generation: {error}"),
        }
    };
    assert_eq!(next_signal.reason, CareReason::RoomSilence);
    assert_ne!(next_signal.attention_id, first_signal.attention_id);
}

#[test]
fn archived_room_pauses_care_and_unarchive_resumes_same_open_attention() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let dir = tempfile::tempdir().expect("temp store");
    let db = dir.path().join("archive-resume.sqlite");
    let mut hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        Arc::new(Store::open(db.to_str()).expect("sqlite store")),
        RoomSettings {
            care_cooldown_secs: 0,
            care_max_attempts: 1,
            care_goal_secs: 1,
            ..RoomSettings::default()
        },
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);
    hub.set_lead("proj", Some("lead".into()), "operator")
        .expect("lead");
    let (mut events, _) = hub.subscribe("proj");
    assert!(hub.join("proj", "member:lead", "lead", SenderType::Agent, 1));
    let heartbeat = RuntimeHealthUpdate {
        wake: "IDLE".into(),
        ack: "OK".into(),
        delivery_id: None,
        attention_id: None,
        stored: false,
        accepted: false,
        first_response: false,
        final_response: false,
        turn_completed: false,
    };
    hub.report_runtime_health("lead", heartbeat.clone())
        .expect("runtime health");
    let goal = hub
        .create_goal(
            "proj",
            CreateGoal {
                outcome: "ship safely".into(),
                checkpoint: None,
                stale_after_secs: None,
                completion: GoalCompletion::Manual,
                task_ids: Vec::new(),
                by: "operator".into(),
            },
        )
        .expect("goal");
    while events.try_recv().is_ok() {}

    TEST_NOW.store(goal.progress_at + 1_001, Ordering::Relaxed);
    hub.tick_care();
    let first_signal = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("missing goal reminder: {error}"),
        }
    };
    assert_eq!(hub.pending_care("proj", "lead").len(), 1);

    hub.set_settings(
        "proj",
        None,
        None,
        None,
        Some(true),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .expect("archive");
    let paused = hub
        .attentions("proj")
        .into_iter()
        .find(|attention| attention.id == first_signal.attention_id)
        .expect("open attention remains visible while archived");
    assert_eq!(paused.status, AttentionStatus::Open);
    assert!(hub.pending_care("proj", "lead").is_empty());
    while events.try_recv().is_ok() {}
    TEST_NOW.store(goal.progress_at + 2_000, Ordering::Relaxed);
    hub.tick_care();
    assert!(!matches!(events.try_recv(), Ok(ServerFrame::Care { .. })));

    hub.set_settings(
        "proj",
        None,
        None,
        None,
        Some(false),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .expect("unarchive");
    while events.try_recv().is_ok() {}
    hub.report_runtime_health("lead", heartbeat)
        .expect("refreshed runtime health");
    hub.tick_care();
    let resumed = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("missing resumed goal reminder: {error}"),
        }
    };
    assert_eq!(resumed.attention_id, first_signal.attention_id);
    assert_ne!(resumed.id, first_signal.id);
}

#[test]
fn runtime_readiness_expires_without_a_renewed_native_lease() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let mut hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        Arc::new(Store::open(None).expect("memory store")),
        RoomSettings::default(),
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);
    hub.report_runtime_health(
        "native-lead",
        RuntimeHealthUpdate {
            wake: "IDLE".into(),
            ack: "IDLE".into(),
            delivery_id: None,
            attention_id: None,
            stored: false,
            accepted: false,
            first_response: false,
            final_response: false,
            turn_completed: false,
        },
    )
    .expect("initial native lease");
    assert!(
        hub.runtime_health_for("native-lead")
            .expect("reported runtime")
            .ready
    );

    TEST_NOW.store(21_001, Ordering::Relaxed);
    assert!(
        !hub.runtime_health_for("native-lead")
            .expect("expired runtime remains observable")
            .ready,
        "a stopped Monitor must become ineligible without a shutdown report"
    );
}

#[test]
fn disjoint_wait_cycle_is_not_starved_by_resolved_first_cycle() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let mut hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        Arc::new(Store::open(None).expect("memory store")),
        RoomSettings {
            care_wait_secs: 600,
            care_cooldown_secs: 0,
            care_max_attempts: 1,
            ..RoomSettings::default()
        },
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);
    hub.set_lead("proj", Some("lead".into()), "operator")
        .expect("lead");
    let (mut events, _) = hub.subscribe("proj");
    assert!(hub.join("proj", "member:lead", "lead", SenderType::Agent, 1));
    hub.report_runtime_health(
        "lead",
        RuntimeHealthUpdate {
            wake: "IDLE".into(),
            ack: "OK".into(),
            delivery_id: None,
            attention_id: None,
            stored: false,
            accepted: false,
            first_response: false,
            final_response: false,
            turn_completed: false,
        },
    )
    .expect("health");
    for (waiter, waiting_for) in [("a", "b"), ("b", "a"), ("c", "d"), ("d", "c")] {
        hub.set_wait(
            "proj",
            SetWait {
                by: waiter.into(),
                waiting_for: waiting_for.into(),
                reason: "cycle".into(),
            },
        )
        .expect("wait");
    }
    while events.try_recv().is_ok() {}
    hub.tick_care();
    let first = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("first cycle missing: {error}"),
        }
    };
    assert_eq!(first.participants, vec!["a", "b"]);
    hub.resolve_attention("proj", &first.attention_id, "lead", None, false)
        .expect("resolve first cycle");
    while events.try_recv().is_ok() {}
    TEST_NOW.store(2_000, Ordering::Relaxed);
    hub.tick_care();
    let second = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("second cycle starved: {error}"),
        }
    };
    assert_eq!(second.participants, vec!["c", "d"]);
}

#[test]
fn clearing_and_recreating_wait_in_same_millisecond_uses_new_generation() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let dir = tempfile::tempdir().expect("temp store");
    let db = dir.path().join("wait-generation.sqlite");
    let mut hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        Arc::new(Store::open(db.to_str()).expect("sqlite store")),
        RoomSettings {
            care_wait_secs: 0,
            care_cooldown_secs: 0,
            care_max_attempts: 1,
            ..RoomSettings::default()
        },
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);
    hub.set_lead("proj", Some("lead".into()), "operator")
        .expect("lead");
    let (mut events, _) = hub.subscribe("proj");
    assert!(hub.join("proj", "member:lead", "lead", SenderType::Agent, 1));
    hub.report_runtime_health(
        "lead",
        RuntimeHealthUpdate {
            wake: "IDLE".into(),
            ack: "OK".into(),
            delivery_id: None,
            attention_id: None,
            stored: false,
            accepted: false,
            first_response: false,
            final_response: false,
            turn_completed: false,
        },
    )
    .expect("health");
    let first_wait = hub
        .set_wait(
            "proj",
            SetWait {
                by: "worker".into(),
                waiting_for: "reviewer".into(),
                reason: "review".into(),
            },
        )
        .expect("wait");
    while events.try_recv().is_ok() {}
    hub.tick_care();
    let first = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("first wait attention missing: {error}"),
        }
    };
    hub.clear_wait("proj", "worker").expect("clear");
    drop(events);
    drop(hub);
    let mut hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        Arc::new(Store::open(db.to_str()).expect("restart store")),
        RoomSettings {
            care_wait_secs: 0,
            care_cooldown_secs: 0,
            care_max_attempts: 1,
            ..RoomSettings::default()
        },
        2,
    );
    hub.now_ms = test_now_ms;
    let (mut events, _) = hub.subscribe("proj");
    assert!(hub.join("proj", "member:lead", "lead", SenderType::Agent, 2));
    hub.report_runtime_health(
        "lead",
        RuntimeHealthUpdate {
            wake: "IDLE".into(),
            ack: "OK".into(),
            delivery_id: None,
            attention_id: None,
            stored: false,
            accepted: false,
            first_response: false,
            final_response: false,
            turn_completed: false,
        },
    )
    .expect("restart health");
    let recreated = hub
        .set_wait(
            "proj",
            SetWait {
                by: "worker".into(),
                waiting_for: "reviewer".into(),
                reason: "review".into(),
            },
        )
        .expect("recreate");
    assert!(recreated.since > first_wait.since);
    while events.try_recv().is_ok() {}
    hub.tick_care();
    let second = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("recreated wait was suppressed: {error}"),
        }
    };
    assert_ne!(second.attention_id, first.attention_id);
    assert_eq!(
        hub.attentions("proj")
            .iter()
            .filter(|attention| attention.status == AttentionStatus::Open)
            .count(),
        1
    );
}

#[test]
fn care_attempt_is_not_burned_when_attention_storage_fails() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let dir = tempfile::tempdir().expect("temp store");
    let db = dir.path().join("care-atomic.sqlite");
    let store = Arc::new(Store::open(db.to_str()).expect("sqlite store"));
    let mut hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        store,
        RoomSettings {
            care_goal_secs: 1,
            care_cooldown_secs: 0,
            care_max_attempts: 1,
            ..RoomSettings::default()
        },
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);
    hub.set_lead("proj", Some("lead".into()), "operator")
        .expect("lead");
    let (mut events, _) = hub.subscribe("proj");
    assert!(hub.join("proj", "member:lead", "lead", SenderType::Agent, 1));
    hub.report_runtime_health(
        "lead",
        RuntimeHealthUpdate {
            wake: "IDLE".into(),
            ack: "OK".into(),
            delivery_id: None,
            attention_id: None,
            stored: false,
            accepted: false,
            first_response: false,
            final_response: false,
            turn_completed: false,
        },
    )
    .expect("health");
    hub.create_goal(
        "proj",
        CreateGoal {
            outcome: "ship".into(),
            checkpoint: None,
            stale_after_secs: None,
            completion: GoalCompletion::Manual,
            task_ids: Vec::new(),
            by: "operator".into(),
        },
    )
    .expect("goal");
    while events.try_recv().is_ok() {}

    let sql = rusqlite::Connection::open(&db).expect("probe db");
    sql.execute_batch(
        "CREATE TRIGGER reject_care_attention
         BEFORE INSERT ON attentions
         BEGIN SELECT RAISE(FAIL, 'injected care failure'); END;",
    )
    .expect("trigger");
    TEST_NOW.store(2_001, Ordering::Relaxed);
    hub.tick_care();
    let counts: (u64, u64, u64) = sql
        .query_row(
            "SELECT
                (SELECT count(*) FROM care_marks),
                (SELECT count(*) FROM attentions),
                (SELECT count(*) FROM care_outbox)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("counts");
    assert_eq!(counts, (0, 0, 0));
    assert!(!matches!(events.try_recv(), Ok(ServerFrame::Care { .. })));

    sql.execute_batch("DROP TRIGGER reject_care_attention")
        .expect("drop trigger");
    TEST_NOW.store(2_002, Ordering::Relaxed);
    hub.tick_care();
    let signal = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("care did not recover: {error}"),
        }
    };
    assert_eq!(signal.attempt, 1);
    let counts: (u64, u64, u64) = sql
        .query_row(
            "SELECT
                (SELECT count(*) FROM care_marks),
                (SELECT count(*) FROM attentions),
                (SELECT count(*) FROM care_outbox)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("counts");
    assert_eq!(counts, (1, 1, 1));
}

#[test]
fn join_request_rate_limit_is_per_source_not_global() {
    use super::JoinRequestCreate;
    let _clock = TEST_CLOCK_LOCK.lock().expect("clock lock");
    let mut hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        Arc::new(Store::open(None).expect("memory store")),
        RoomSettings::default(),
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);

    let a: std::net::IpAddr = "10.0.0.1".parse().unwrap();
    let b: std::net::IpAddr = "10.0.0.2".parse().unwrap();
    let cap = Hub::JOIN_CREATE_MAX_IN_WINDOW;

    // Source A exhausts its own window with distinct names (no NameTaken noise).
    for i in 0..cap {
        assert!(
            matches!(
                hub.create_join_request(&format!("a{i}"), "agent", a),
                JoinRequestCreate::Created { .. }
            ),
            "A request {i} should be admitted within the window"
        );
    }
    // One more from A is refused — the per-source window is full.
    assert!(matches!(
        hub.create_join_request("a-over", "agent", a),
        JoinRequestCreate::BacklogFull
    ));
    // A GLOBAL limiter would also refuse B here (the counter is already at cap).
    // Per-source isolation means B's own window is untouched -> Created.
    assert!(matches!(
        hub.create_join_request("b0", "agent", b),
        JoinRequestCreate::Created { .. }
    ));

    // Sliding window: once A's timestamps age past the window, A is admitted again.
    TEST_NOW.store(1_000 + Hub::JOIN_CREATE_WINDOW_MS + 1, Ordering::Relaxed);
    assert!(matches!(
        hub.create_join_request("a-later", "agent", a),
        JoinRequestCreate::Created { .. }
    ));
}

#[test]
fn approve_issued_membership_authenticates_on_a_persistent_store() {
    use super::{Approve, JoinRequestCreate};
    let _clock = TEST_CLOCK_LOCK.lock().expect("clock lock");
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("lobby-auth.db");
    // A PERSISTENT (file-backed) store: the Lobby authenticates through the
    // credentials table here, NOT the in-memory cache (which only backs a
    // non-persistent store), so this is the configuration that caught the bug.
    let store = Arc::new(Store::open(Some(path.to_str().expect("db path"))).expect("store"));
    store
        .ensure_master_principal("MASTER", "operator", 1)
        .expect("master");
    let mut hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        store,
        RoomSettings::default(),
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);

    // Master mints one right; an outside agent requests to join.
    hub.mint_admission_stock(1, 3_600_000, None);
    let ip: std::net::IpAddr = "10.0.0.5".parse().unwrap();
    let JoinRequestCreate::Created {
        request_id,
        request_secret,
    } = hub.create_join_request("newbie", "agent", ip)
    else {
        panic!("expected Created");
    };

    // Master approves; the agent bootstraps its mb_ exactly once.
    assert!(matches!(
        hub.approve_join_request(&request_id, "operator"),
        Approve::Approved
    ));
    let mb = hub
        .claim_join_request_bootstrap(&request_id, &request_secret)
        .expect("bootstrap reads the store");
    let crate::store::BootstrapClaim::Delivered(mb) = mb else {
        panic!("bootstrap yields the mb_, got {mb:?}");
    };
    assert!(mb.starts_with("mb_"));

    // REGRESSION GUARD (review blocker): the approve-issued mb_ must AUTHENTICATE
    // immediately at the Lobby entry point on a persistent store. `member_for_credential`
    // resolves through the credentials table here — RED before the fix (approve wrote
    // only the `members` row, so this returned None -> the joining agent got 401
    // until an unrelated server restart backfilled the credential).
    let member = hub
        .member_for_credential(Some(&mb))
        .expect("approve-issued mb_ must authenticate immediately on a persistent store");
    assert_eq!(member.name, "newbie");
    assert_eq!(member.kind, "agent");
}

#[test]
fn join_request_is_announced_visibly_in_the_home_loca_without_leaking_the_secret() {
    use super::JoinRequestCreate;
    let hub = Hub::build(
        HubConfig {
            admin_token: "M".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        Arc::new(Store::open(None).expect("memory store")),
        RoomSettings::default(),
        1,
    );
    // Deliberately do NOT join the home loca first: this covers the review
    // blocker where `iye` is not yet in memory. The announce must STILL surface
    // (create must never silently drop it), so create_join_request materialises
    // the home loca and posts regardless.
    let ip: std::net::IpAddr = "10.0.0.1".parse().unwrap();
    let out = hub.create_join_request("visitor", "agent", ip);
    let JoinRequestCreate::Created { request_secret, .. } = out else {
        panic!("expected Created");
    };

    // A visible "<name> wants to join" announce must land in the home loca so the
    // Master sees it in the main app — the whole point of this fix. RED before it
    // (create only wrote the hidden request row). And it must carry NO secret.
    let (_rx, history) = hub.subscribe("iye");
    let announce = history
        .iter()
        .find(|m| m.sender == "loca" && m.text.contains("wants to join"))
        .unwrap_or_else(|| {
            panic!(
                "no visible join-request announce in the home loca; history: {:?}",
                history.iter().map(|m| &m.text).collect::<Vec<_>>()
            )
        });
    assert!(announce.text.contains("visitor"));
    assert!(
        !announce.text.contains(&request_secret) && !announce.text.contains("jrs_"),
        "the announce must never carry the request secret"
    );
}

/// The mention grammar is pinned by `docs/mention-cases.json`, which the
/// agent kit is tested against too.
#[test]
fn mentions_follow_the_shared_cases() {
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("../../../../docs/mention-cases.json")).unwrap();
    for case in cases["cases"].as_array().unwrap() {
        let text = case["text"].as_str().unwrap();
        let expected: Vec<String> = case["names"]
            .as_array()
            .unwrap()
            .iter()
            .map(|name| name.as_str().unwrap().to_string())
            .collect();
        assert_eq!(super::mentioned_names(text), expected, "{text:?}");
    }
}

#[test]
fn an_agent_naming_people_does_not_wake_every_agent() {
    use super::names_people_but_not_all as named;
    assert!(named("@Nwakaeme @smaster #18871'e ek"));
    assert!(named("ok @mithrandir."));
    assert!(!named("@all please look"));
    assert!(!named("@Nwakaeme and @all of you"));
    assert!(!named("no mention here"));
    assert!(!named("mail me at omer@example.com"));
    assert!(!named("@ alone"));
}

#[test]
fn an_agents_all_target_narrows_to_its_mentions_but_a_human_and_at_all_stay() {
    let temp = tempfile::tempdir().unwrap();
    let store =
        Arc::new(Store::open(Some(temp.path().join("target.db").to_str().unwrap())).unwrap());
    let hub = critical_restart_hub(store, "root");
    let post = |sender: &str, kind: SenderType, text: &str| {
        hub.post(
            "team",
            PostMessage {
                kind: Default::default(),
                sender: sender.into(),
                sender_type: kind,
                target: Some("all".into()),
                text: text.into(),
                reply_to: None,
                op_id: None,
                attachments: Vec::new(),
            },
            true,
            &sender.into(),
            None,
        )
        .unwrap()
        .target
    };
    assert_eq!(
        post(
            "curunir",
            SenderType::Agent,
            "@Nwakaeme @smaster see #18871"
        ),
        None
    );
    assert_eq!(
        post("curunir", SenderType::Agent, "@all the build is green").as_deref(),
        Some("all")
    );
    assert_eq!(
        post("curunir", SenderType::Agent, "nobody named").as_deref(),
        Some("all")
    );
    assert_eq!(
        post("omrylcn", SenderType::User, "@Nwakaeme look").as_deref(),
        Some("all"),
        "a person's chosen @all target is theirs to keep"
    );
}

#[test]
fn a_group_loca_keeps_its_in_room_care_but_none_reaches_care_loca() {
    let _clock = TEST_CLOCK_LOCK.lock().expect("clock test lock");
    let mut hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::from(["loca-care".into()]),
        },
        Arc::new(Store::open(None).expect("memory store")),
        RoomSettings {
            care_goal_secs: 1,
            care_cooldown_secs: 1,
            ..RoomSettings::default()
        },
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);
    let master = hub.resolve_authority(Some("MASTER"), None);
    let group = hub.create_group("quiet", &master).expect("group");
    hub.create_loca_in("proj", &master, None, Some(&group.id))
        .expect("group loca");
    assert!(hub.is_group_loca("proj"));
    assert!(hub.join("proj", "member:lead", "lead", SenderType::Agent, 1));
    assert!(hub.join("iye", "member:loca-care", "loca-care", SenderType::Agent, 1));
    hub.set_lead("proj", Some("lead".into()), "operator")
        .expect("lead");
    hub.report_runtime_health(
        "loca-care",
        RuntimeHealthUpdate {
            wake: "IDLE".into(),
            ack: "OK".into(),
            delivery_id: None,
            attention_id: None,
            stored: false,
            accepted: false,
            first_response: false,
            final_response: false,
            turn_completed: false,
        },
    )
    .expect("caretaker health");
    // The group's own lead is healthy, so the reminder is theirs, in the loca.
    hub.report_runtime_health(
        "lead",
        RuntimeHealthUpdate {
            wake: "IDLE".into(),
            ack: "OK".into(),
            delivery_id: None,
            attention_id: None,
            stored: false,
            accepted: false,
            first_response: false,
            final_response: false,
            turn_completed: false,
        },
    )
    .expect("lead health");
    let (mut proj, _) = hub.subscribe("proj");
    let (mut care_loca, _) = hub.subscribe("iye");
    hub.create_goal(
        "proj",
        CreateGoal {
            outcome: "nothing leaves".into(),
            checkpoint: None,
            stale_after_secs: None,
            completion: GoalCompletion::Manual,
            task_ids: Vec::new(),
            by: "operator".into(),
        },
    )
    .expect("goal");
    while proj.try_recv().is_ok() {}
    while care_loca.try_recv().is_ok() {}
    TEST_NOW.store(5_000, Ordering::Relaxed);
    hub.tick_care();
    let mut in_room = 0;
    while let Ok(frame) = proj.try_recv() {
        if matches!(frame, ServerFrame::Care { .. }) {
            in_room += 1;
        }
    }
    assert!(
        in_room > 0,
        "the group's own lead is reminded inside the loca"
    );
    while let Ok(frame) = care_loca.try_recv() {
        assert!(
            !matches!(frame, ServerFrame::Care { .. }),
            "a group loca's care never reaches Care, where every rank reads"
        );
    }
}

type TurnJob = Box<dyn FnOnce(&Hub, &super::RequestAuthority) + Send>;

/// A loca move holds the invite lock from its check of the target group to
/// its mirror. Everything that changes what that check saw — deleting or
/// archiving the group, founding a loca in it, adding or removing a member —
/// waits for the same lock, so none of it can land between a move's check and
/// its commit. The test stands where a move stands: it holds the lock, starts
/// each rival and a move, and nothing finishes or changes until it lets go.
#[test]
fn group_changes_and_a_loca_move_take_turns_on_the_invite_lock() {
    let directory = tempfile::tempdir().unwrap();
    let hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        Arc::new(Store::open(directory.path().join("turns.db").to_str()).unwrap()),
        RoomSettings::default(),
        1,
    );
    let master = hub.resolve_authority(Some("MASTER"), None);
    let group = |name: &str| hub.create_group(name, &master).expect("group").id;
    let (doomed, shelved, target, nursery, roster) = (
        group("doomed"),
        group("shelved"),
        group("target"),
        group("nursery"),
        group("roster"),
    );
    hub.create_loca_in("mover", &master, None, None)
        .expect("default loca");
    let member = hub.admit_member("turn-member", "user", "master").unwrap();
    let principal = hub.member_principal(&member.id).expect("principal");

    let held = hub.member_lifecycle.lock().expect("invite lock");
    let (done, finished) = std::sync::mpsc::channel::<&'static str>();
    let mut threads = Vec::new();
    let mut start = |label: &'static str, job: TurnJob| {
        let (hub, master, done) = (hub.clone(), master.clone(), done.clone());
        threads.push(std::thread::spawn(move || {
            job(&hub, &master);
            done.send(label).expect("report");
        }));
    };
    {
        let id = doomed.clone();
        start(
            "delete",
            Box::new(move |hub, master| {
                hub.delete_group(&id, master).expect("delete");
            }),
        );
        let id = shelved.clone();
        start(
            "archive",
            Box::new(move |hub, master| {
                hub.set_group_archived(&id, true, master).expect("archive");
            }),
        );
        let id = nursery.clone();
        start(
            "found",
            Box::new(move |hub, master| {
                hub.create_loca_in("born", master, None, Some(&id))
                    .expect("found a group loca");
            }),
        );
        let (id, who) = (roster.clone(), principal.clone());
        start(
            "add member",
            Box::new(move |hub, master| {
                hub.add_group_member(&id, &who, master).expect("add");
            }),
        );
        let id = target.clone();
        start(
            "move",
            Box::new(move |hub, master| {
                hub.set_loca_group("mover", Some(&id), master)
                    .expect("move");
            }),
        );
    }

    assert!(
        finished
            .recv_timeout(std::time::Duration::from_millis(500))
            .is_err(),
        "nothing finishes while the invite lock is held"
    );
    assert!(hub.group_view(&doomed, &master).is_ok(), "not deleted yet");
    assert!(
        !hub.group_view(&shelved, &master).unwrap().archived,
        "not archived yet"
    );
    assert!(!hub.room_exists("born"), "not founded yet");
    assert!(
        hub.group_view(&roster, &master).unwrap().members.is_empty(),
        "no member added yet"
    );
    assert!(!hub.is_group_loca("mover"), "not moved yet");

    drop(held);
    let mut labels: Vec<&str> = (0..5)
        .map(|_| {
            finished
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("every change finishes once the lock is free")
        })
        .collect();
    labels.sort_unstable();
    assert_eq!(labels, ["add member", "archive", "delete", "found", "move"]);
    for thread in threads {
        thread.join().expect("no change panicked");
    }
    assert!(hub.group_view(&doomed, &master).is_err(), "deleted");
    assert!(hub.group_view(&shelved, &master).unwrap().archived);
    assert_eq!(hub.group_of("born").as_deref(), Some(nursery.as_str()));
    assert_eq!(hub.group_view(&roster, &master).unwrap().members.len(), 1);
    assert_eq!(hub.group_of("mover").as_deref(), Some(target.as_str()));
}

/// Adding or removing ONE extension is worked out from the set as it stands
/// under the rooms lock. Two single changes that wait on that lock together
/// both land: neither is computed from a copy read before the other wrote.
#[test]
fn two_single_extension_changes_at_once_both_land() {
    use super::ExtensionEdit;
    let directory = tempfile::tempdir().unwrap();
    let hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        Arc::new(Store::open(directory.path().join("ext-turns.db").to_str()).unwrap()),
        RoomSettings::default(),
        1,
    );
    let master = hub.resolve_authority(Some("MASTER"), None);
    hub.create_loca_in("desk", &master, Some(vec!["notes".into()]), None)
        .expect("loca with notes only");
    assert_eq!(hub.enabled_extensions("desk"), ["notes"]);

    let (done, finished) = std::sync::mpsc::channel::<&'static str>();
    let held = hub.rooms.lock().expect("rooms lock");
    let mut threads = Vec::new();
    for (label, edit) in [
        ("add focus", ("focus", true)),
        ("add wiki", ("wiki", true)),
        ("remove notes", ("notes", false)),
    ] {
        let (hub, done) = (hub.clone(), done.clone());
        threads.push(std::thread::spawn(move || {
            let (id, add) = edit;
            let edit = if add {
                ExtensionEdit::Add(id)
            } else {
                ExtensionEdit::Remove(id)
            };
            hub.edit_extensions("desk", edit, "operator", SenderType::User)
                .expect("single change");
            done.send(label).expect("report");
        }));
    }
    assert!(
        finished
            .recv_timeout(std::time::Duration::from_millis(400))
            .is_err(),
        "each change waits for the lock before it reads the current set"
    );
    drop(held);
    for thread in threads {
        thread.join().expect("no change panicked");
    }
    assert_eq!(
        hub.enabled_extensions("desk"),
        ["focus", "wiki"],
        "all three single changes are kept, in catalog order"
    );
    let journal = hub.journal("desk");
    assert_eq!(
        journal.len(),
        4,
        "the opening line plus one line per change: {journal:?}"
    );
}

/// A direct message between a Smaster and a member, on a fixed clock, with a helper
/// that lets either side speak. Returns (hub, room, smaster principal, member
/// principal).
fn witness_fixture(db: &std::path::Path) -> (Hub, String, String, String) {
    let store = Arc::new(Store::open(db.to_str()).unwrap());
    store.add_smaster("sm_witness", "deputy", 1).unwrap();
    let mut hub = critical_restart_hub(store, "root");
    hub.now_ms = test_now_ms;
    TEST_NOW.store(10_000, Ordering::Relaxed);
    let token = critical_admin_session(&hub, "sm_witness");
    let deputy = hub.session_identity(Some(&token)).expect("deputy session");
    hub.admit_member("ayse", "user", "master").expect("member");
    let (dm, _) = hub.open_dm(&deputy, "ayse").expect("dm");
    (hub, dm.room, dm.a, dm.b)
}

fn say_in(hub: &Hub, room: &str, principal: &str, text: &str) {
    hub.post(
        room,
        PostMessage {
            kind: Default::default(),
            sender: "speaker".into(),
            sender_type: SenderType::User,
            target: None,
            text: text.into(),
            reply_to: None,
            op_id: None,
            attachments: Vec::new(),
        },
        true,
        &principal.into(),
        None,
    )
    .expect("post");
}

fn witness_texts(hub: &Hub, room: &str) -> Option<Vec<String>> {
    hub.dm_witness_messages_after(room, 0, 1_000)
        .map(|page| page.expect("archive").into_iter().map(|m| m.text).collect())
}

/// The call boundary is ordered with message writes, not with the wall clock.
/// Message times run ahead of a clock that stands still, so a boundary taken
/// from the clock would hand the Master words said before the call.
#[test]
fn a_called_in_master_never_reads_what_was_said_before_the_call() {
    let _clock = TEST_CLOCK_LOCK.lock().expect("clock test lock");
    let directory = tempfile::tempdir().unwrap();
    let (hub, room, a, b) = witness_fixture(&directory.path().join("witness-clock.db"));

    // Three messages while the clock stands still: their times are 10_000,
    // 10_001 and 10_002, ahead of the clock.
    for text in ["before 1", "before 2", "before 3"] {
        say_in(&hub, &room, &a, text);
    }
    assert!(witness_texts(&hub, &room).is_none(), "nobody called yet");

    hub.dm_call_master(&room, &a).expect("call");
    assert_eq!(
        witness_texts(&hub, &room).expect("called in"),
        Vec::<String>::new(),
        "nothing said before the call is on the Master's side"
    );
    say_in(&hub, &room, &b, "after 1");
    say_in(&hub, &room, &a, "after 2");
    assert_eq!(
        witness_texts(&hub, &room).expect("called in"),
        ["after 1", "after 2"]
    );

    // One side allowing history opens nothing; both open all of it.
    hub.dm_allow_history(&room, &a).expect("allow a");
    assert_eq!(
        witness_texts(&hub, &room).expect("called in"),
        ["after 1", "after 2"]
    );
    hub.dm_allow_history(&room, &b).expect("allow b");
    assert_eq!(witness_texts(&hub, &room).expect("called in").len(), 5);

    // Sending the Master out ends the visit and its history permission: a
    // second call starts from its own moment and opens nothing earlier.
    hub.dm_dismiss_master(&room, &a).expect("dismiss");
    assert!(witness_texts(&hub, &room).is_none(), "the Master is out");
    say_in(&hub, &room, &b, "between visits");
    hub.dm_call_master(&room, &b).expect("second call");
    assert_eq!(
        witness_texts(&hub, &room).expect("called in again"),
        Vec::<String>::new(),
        "the earlier permission does not carry into the new visit"
    );
    say_in(&hub, &room, &a, "second visit");
    assert_eq!(
        witness_texts(&hub, &room).expect("called in again"),
        ["second visit"]
    );
}

/// The permission and the messages it covers are fixed together. A read that
/// was granted while the Master was in, and finishes after the Master is sent
/// out, does not return what the two say afterwards.
#[test]
fn a_witness_read_granted_before_dismissal_returns_nothing_said_after_it() {
    let _clock = TEST_CLOCK_LOCK.lock().expect("clock test lock");
    let directory = tempfile::tempdir().unwrap();
    let (hub, room, a, b) = witness_fixture(&directory.path().join("witness-window.db"));
    hub.dm_call_master(&room, &a).expect("call");
    say_in(&hub, &room, &b, "during the visit");

    // The read is granted here ...
    let window = hub.dm_witness_window(&room).expect("the Master is in");
    // ... then the Master is sent out and the two go on in private ...
    hub.dm_dismiss_master(&room, &a).expect("dismiss");
    say_in(&hub, &room, &a, "after the Master left");
    say_in(&hub, &room, &b, "still private");
    // ... and only now does the granted read reach the archive.
    let texts: Vec<String> = hub
        .dm_witness_page(&room, &window, 0, 1_000)
        .expect("archive")
        .into_iter()
        .map(|m| m.text)
        .collect();
    assert_eq!(
        texts,
        ["during the visit"],
        "the old visit's permission does not open the later conversation"
    );
    assert!(
        hub.dm_witness_window(&room).is_none(),
        "and no new read is granted"
    );

    // Called back in, a read granted before the new call still returns
    // nothing from it: the new visit's words lie above the old ceiling and
    // the old visit's words below the new floor.
    hub.dm_call_master(&room, &b).expect("second call");
    say_in(&hub, &room, &a, "second visit");
    let again: Vec<String> = hub
        .dm_witness_page(&room, &window, 0, 1_000)
        .expect("archive")
        .into_iter()
        .map(|m| m.text)
        .collect();
    assert_eq!(again, ["during the visit"]);
}

/// A visit recorded by a version that stamped the call with the wall clock
/// cannot be trusted: message times ran ahead of the clock, so words said
/// before the call sit on the Master's side of that boundary. Opening the
/// database ends such a visit once. A visit opened under the ordered rule
/// survives a restart with the same boundary.
#[test]
fn a_visit_recorded_before_the_ordered_boundary_ends_when_the_store_opens() {
    let _clock = TEST_CLOCK_LOCK.lock().expect("clock test lock");
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("witness-upgrade.db");
    let (hub, room, a, _b) = witness_fixture(&path);
    for text in ["before 1", "before 2", "before 3"] {
        say_in(&hub, &room, &a, text);
    }
    drop(hub);

    // The row as the earlier version left it: the Master called in at the
    // wall-clock time 10_000, which the three messages above (10_000, 10_001,
    // 10_002) are not before.
    {
        let conn = rusqlite::Connection::open(&path).expect("database");
        let changed = conn
            .execute(
                "UPDATE birebirs SET master_called_by = ?1, master_since = 10000 WHERE room = ?2",
                rusqlite::params![a, room],
            )
            .expect("legacy visit");
        assert_eq!(changed, 1);
    }

    let reopen = |path: &std::path::Path| -> Hub {
        let store = Arc::new(Store::open(path.to_str()).expect("reopen"));
        let mut hub = critical_restart_hub(store, "root");
        hub.now_ms = test_now_ms;
        hub
    };
    let hub = reopen(&path);
    assert!(
        witness_texts(&hub, &room).is_none(),
        "the legacy visit ended; nothing said before the call is shown"
    );
    {
        let conn = rusqlite::Connection::open(&path).expect("database");
        let open: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM birebirs WHERE master_since IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(open, 0, "and it is ended on disk");
    }

    // The two call again under the ordered rule: still nothing earlier.
    hub.dm_call_master(&room, &a).expect("new call");
    assert_eq!(
        witness_texts(&hub, &room).expect("called in"),
        Vec::<String>::new()
    );
    say_in(&hub, &room, &a, "after the new call");
    assert_eq!(
        witness_texts(&hub, &room).expect("called in"),
        ["after the new call"]
    );
    drop(hub);

    // A restart keeps a visit opened under the ordered rule, and its boundary.
    let hub = reopen(&path);
    assert_eq!(
        witness_texts(&hub, &room).expect("the visit survives a restart"),
        ["after the new call"]
    );
    say_in(&hub, &room, &a, "after the restart");
    assert_eq!(
        witness_texts(&hub, &room).expect("called in"),
        ["after the new call", "after the restart"]
    );
    drop(hub);
    // Opening again changes nothing: the one-time step leaves it alone.
    let hub = reopen(&path);
    assert_eq!(
        witness_texts(&hub, &room).expect("still in"),
        ["after the new call", "after the restart"]
    );
}

/// The seven-day sweep takes back a Smaster code nobody signed in with and
/// leaves one that was used, across a restart, and the ledger says which.
#[test]
fn the_unused_smaster_sweep_spares_a_used_code() {
    let _clock = TEST_CLOCK_LOCK.lock().expect("clock test lock");
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(temp.path().join("sweep.db").to_str()).unwrap());
    TEST_NOW.store(1_000, Ordering::Relaxed);
    let mut hub = critical_restart_hub(store.clone(), "root");
    hub.now_ms = test_now_ms;
    let used = hub.add_smaster("used").unwrap();
    let idle = hub.add_smaster("idle").unwrap();
    assert!(hub.resolve_authority(Some(&used), None).is_building_admin());
    hub.expire_unused_smasters();
    assert!(hub.smaster_name(Some(&idle)).is_some(), "not yet due");
    TEST_NOW.store(1_000 + Hub::SMASTER_USE_MS + 1, Ordering::Relaxed);
    hub.expire_unused_smasters();
    assert_eq!(hub.smaster_name(Some(&used)).as_deref(), Some("used"));
    assert!(hub.smaster_name(Some(&idle)).is_none());
    assert!(!hub.resolve_authority(Some(&idle), None).is_building_admin());
    let states: Vec<(String, &str)> = hub
        .key_ledger()
        .unwrap()
        .into_iter()
        .filter(|k| k.kind == "smaster")
        .map(|k| (k.for_name.unwrap_or_default(), k.state))
        .collect();
    assert!(states.contains(&("used".into(), "active")), "{states:?}");
    assert!(states.contains(&("idle".into(), "expired")), "{states:?}");
    drop(hub);
    let hub = critical_restart_hub(store, "root");
    assert!(hub.smaster_name(Some(&used)).is_some());
    assert!(hub.smaster_name(Some(&idle)).is_none());
}

/// Post into "ops" as `sender`, with the operation scope (`principal`) and the
/// verified writer (`writer`) the REST or WS door would pass.
#[allow(clippy::too_many_arguments)]
fn rename_case_say(
    hub: &Hub,
    sender: &str,
    sender_type: SenderType,
    text: &str,
    reply_to: Option<u64>,
    op: Option<&str>,
    principal: &str,
    writer: Option<&str>,
) -> protocol::Message {
    hub.post(
        "ops",
        PostMessage {
            kind: Default::default(),
            sender: sender.into(),
            sender_type,
            target: None,
            text: text.into(),
            reply_to,
            op_id: op.map(Into::into),
            attachments: Vec::new(),
        },
        true,
        &principal.into(),
        writer,
    )
    .unwrap()
}

/// A rename frame as a client receives it: `Relabelled` (to, ids,
/// reply_ids) or the older `Renamed` (from, to).
#[derive(Debug, PartialEq)]
enum Told {
    ById(String, Vec<u64>, Vec<u64>),
    ByName(String, String),
}

/// Every rename frame waiting on `rx`, in the order it was sent.
fn rename_frames(rx: &mut tokio::sync::broadcast::Receiver<ServerFrame>) -> Vec<Told> {
    let mut frames = Vec::new();
    while let Ok(frame) = rx.try_recv() {
        match frame {
            ServerFrame::Relabelled { to, ids, reply_ids } => {
                frames.push(Told::ById(to, ids, reply_ids))
            }
            ServerFrame::Renamed { from, to } => frames.push(Told::ByName(from, to)),
            _ => {}
        }
    }
    frames
}

fn sender_of(history: &[protocol::Message], id: u64) -> String {
    history.iter().find(|m| m.id == id).unwrap().sender.clone()
}

fn reply_label_of(history: &[protocol::Message], id: u64) -> Option<String> {
    history
        .iter()
        .find(|m| m.id == id)
        .unwrap()
        .reply_to_sender
        .clone()
}

#[test]
fn a_rename_leaves_a_former_wearer_s_and_an_unknown_author_s_messages_alone() {
    let temp = tempfile::tempdir().unwrap();
    let store =
        Arc::new(Store::open(Some(temp.path().join("wearers.db").to_str().unwrap())).unwrap());
    store.add_smaster("sm_deputy", "deputy-start", 1).unwrap();
    let hub = critical_restart_hub(store.clone(), "root");
    // The first wearer of "shared" is a member; it speaks, then leaves.
    let member = hub.admit_member("shared", "human", "master").unwrap();
    let member_principal = hub.member_principal(&member.id).expect("member principal");
    let member_words = rename_case_say(
        &hub,
        "shared",
        SenderType::User,
        "member words",
        None,
        Some("op-member"),
        &format!("mb:{}", member.id.clone()),
        Some(&member_principal),
    );
    // A line under the same label whose writer nobody recorded.
    let unknown_words = rename_case_say(
        &hub,
        "shared",
        SenderType::User,
        "unknown words",
        None,
        None,
        "user:shared",
        None,
    );
    assert!(hub.revoke_member(&member.id).unwrap());

    // The Smaster takes the freed label, speaks under it, then renames.
    let session = critical_admin_session(&hub, "sm_deputy");
    let actor = hub.resolve_authority(None, Some(&session));
    let deputy = actor.principal_id.clone().expect("deputy principal");
    hub.rename_profile(&actor, "shared")
        .expect("take the freed name");
    let actor = hub.resolve_authority(None, Some(&session));
    let smaster_words = rename_case_say(
        &hub,
        "shared",
        SenderType::User,
        "smaster words",
        None,
        Some("op-smaster"),
        "user:shared",
        Some(&deputy),
    );
    let reply_to_member = rename_case_say(
        &hub,
        "helper",
        SenderType::Agent,
        "answering the member",
        Some(member_words.id),
        Some("op-r1"),
        "agent:helper",
        None,
    );
    let reply_to_unknown = rename_case_say(
        &hub,
        "helper",
        SenderType::Agent,
        "answering the unknown",
        Some(unknown_words.id),
        Some("op-r3"),
        "agent:helper",
        None,
    );
    let reply_to_smaster = rename_case_say(
        &hub,
        "helper",
        SenderType::Agent,
        "answering the smaster",
        Some(smaster_words.id),
        Some("op-r2"),
        "agent:helper",
        None,
    );
    assert_eq!(reply_to_member.reply_to_sender.as_deref(), Some("shared"));
    assert_eq!(reply_to_smaster.reply_to_sender.as_deref(), Some("shared"));
    let mut rx = hub.subscribe("ops").0;

    hub.rename_profile(&actor, "renamed").expect("rename");

    let live = hub.subscribe("ops").1;
    assert_eq!(sender_of(&live, smaster_words.id), "renamed");
    assert_eq!(
        sender_of(&live, member_words.id),
        "shared",
        "memory: member"
    );
    assert_eq!(
        sender_of(&live, unknown_words.id),
        "shared",
        "memory: unknown"
    );
    assert_eq!(
        reply_label_of(&live, reply_to_member.id).as_deref(),
        Some("shared"),
        "memory: reply label pointing at the member"
    );
    assert_eq!(
        reply_label_of(&live, reply_to_unknown.id).as_deref(),
        Some("shared"),
        "memory: reply label pointing at the unknown writer"
    );
    assert_eq!(
        reply_label_of(&live, reply_to_smaster.id).as_deref(),
        Some("renamed")
    );
    assert_eq!(
        rename_frames(&mut rx),
        vec![
            Told::ById(
                "renamed".into(),
                vec![smaster_words.id],
                vec![reply_to_smaster.id]
            ),
            // People's lines under "shared" are in the tail, so older pages
            // are told by name exactly as before.
            Told::ByName("shared".into(), "renamed".into()),
        ],
        "connected clients are told exactly which messages changed"
    );

    drop(hub);
    let restarted = critical_restart_hub(store.clone(), "root");
    let after = restarted.subscribe("ops").1;
    assert_eq!(sender_of(&after, smaster_words.id), "renamed");
    assert_eq!(
        sender_of(&after, member_words.id),
        "shared",
        "restart: member"
    );
    assert_eq!(
        sender_of(&after, unknown_words.id),
        "shared",
        "restart: unknown"
    );
    // Reply labels are not stored: after a restart no reply carries one.
    for reply in [&reply_to_member, &reply_to_unknown, &reply_to_smaster] {
        assert_eq!(reply_label_of(&after, reply.id), None);
    }
}

#[test]
fn a_rank_s_socket_sent_words_read_the_same_after_a_rename_and_a_restart() {
    let temp = tempfile::tempdir().unwrap();
    let store =
        Arc::new(Store::open(Some(temp.path().join("ws-sent.db").to_str().unwrap())).unwrap());
    store.add_smaster("sm_deputy", "before", 1).unwrap();
    let hub = critical_restart_hub(store.clone(), "root");
    let session = critical_admin_session(&hub, "sm_deputy");
    let actor = hub.resolve_authority(None, Some(&session));
    let deputy = actor.principal_id.clone().unwrap();
    // The WS door passes the seat identity as the operation principal.
    let said = rename_case_say(
        &hub,
        "before",
        SenderType::User,
        "over the socket",
        None,
        Some("op-ws"),
        &format!("sm:pr:{deputy}"),
        Some(&deputy),
    );
    hub.rename_profile(&actor, "after").expect("rename");
    let live_sender = sender_of(&hub.subscribe("ops").1, said.id);
    drop(hub);
    let restarted = critical_restart_hub(store.clone(), "root");
    let stored_sender = sender_of(&restarted.subscribe("ops").1, said.id);
    assert_eq!(live_sender, "after");
    assert_eq!(stored_sender, live_sender, "a restart shows what was live");
}

#[test]
fn a_second_rename_still_carries_words_from_before_the_first() {
    let temp = tempfile::tempdir().unwrap();
    let store =
        Arc::new(Store::open(Some(temp.path().join("twice.db").to_str().unwrap())).unwrap());
    store.add_smaster("sm_deputy", "first", 1).unwrap();
    let hub = critical_restart_hub(store.clone(), "root");
    let session = critical_admin_session(&hub, "sm_deputy");
    let actor = hub.resolve_authority(None, Some(&session));
    let deputy = actor.principal_id.clone().unwrap();
    let said = rename_case_say(
        &hub,
        "first",
        SenderType::User,
        "said as first",
        None,
        Some("op-1"),
        "user:first",
        Some(&deputy),
    );
    let reply = rename_case_say(
        &hub,
        "helper",
        SenderType::Agent,
        "answering first",
        Some(said.id),
        None,
        "agent:helper",
        None,
    );
    hub.rename_profile(&actor, "second").expect("first rename");
    let actor = hub.resolve_authority(None, Some(&session));
    hub.rename_profile(&actor, "third").expect("second rename");
    let live = hub.subscribe("ops").1;
    let live_sender = sender_of(&live, said.id);
    assert_eq!(reply_label_of(&live, reply.id).as_deref(), Some("third"));
    drop(hub);
    let restarted = critical_restart_hub(store.clone(), "root");
    let stored_sender = sender_of(&restarted.subscribe("ops").1, said.id);
    assert_eq!(live_sender, "third");
    assert_eq!(stored_sender, "third", "a restart shows what was live");
}

#[test]
fn a_reply_label_follows_ownership_after_its_target_left_the_memory_tail() {
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(Some(temp.path().join("tail.db").to_str().unwrap())).unwrap());
    store.add_smaster("sm_deputy", "deputy-start", 1).unwrap();
    let hub = critical_restart_hub(store.clone(), "root");
    let member = hub.admit_member("shared", "human", "master").unwrap();
    let member_principal = hub.member_principal(&member.id).unwrap();
    let member_words = rename_case_say(
        &hub,
        "shared",
        SenderType::User,
        "member words",
        None,
        None,
        &format!("mb:{}", member.id.clone()),
        Some(&member_principal),
    );
    assert!(hub.revoke_member(&member.id).unwrap());
    let session = critical_admin_session(&hub, "sm_deputy");
    let actor = hub.resolve_authority(None, Some(&session));
    let deputy = actor.principal_id.clone().unwrap();
    hub.rename_profile(&actor, "shared").unwrap();
    let smaster_words = rename_case_say(
        &hub,
        "shared",
        SenderType::User,
        "smaster words",
        None,
        None,
        "user:shared",
        Some(&deputy),
    );
    // Push both out of the in-memory tail, then answer each of them.
    for n in 0..super::HISTORY_LIMIT {
        rename_case_say(
            &hub,
            "filler",
            SenderType::Agent,
            &format!("filler {n}"),
            None,
            None,
            "agent:filler",
            None,
        );
    }
    let to_member = rename_case_say(
        &hub,
        "helper",
        SenderType::Agent,
        "to the member",
        Some(member_words.id),
        None,
        "agent:helper",
        None,
    );
    let to_smaster = rename_case_say(
        &hub,
        "helper",
        SenderType::Agent,
        "to the smaster",
        Some(smaster_words.id),
        None,
        "agent:helper",
        None,
    );
    let live = hub.subscribe("ops").1;
    assert!(live
        .iter()
        .all(|m| m.id != member_words.id && m.id != smaster_words.id));
    assert_eq!(
        reply_label_of(&live, to_member.id).as_deref(),
        Some("shared")
    );
    assert_eq!(
        reply_label_of(&live, to_smaster.id).as_deref(),
        Some("shared")
    );

    let mut rx = hub.subscribe("ops").0;
    let actor = hub.resolve_authority(None, Some(&session));
    hub.rename_profile(&actor, "renamed").unwrap();
    let live = hub.subscribe("ops").1;
    assert_eq!(
        reply_label_of(&live, to_member.id).as_deref(),
        Some("shared")
    );
    assert_eq!(
        reply_label_of(&live, to_smaster.id).as_deref(),
        Some("renamed")
    );
    assert_eq!(
        rename_frames(&mut rx),
        vec![Told::ById("renamed".into(), vec![], vec![to_smaster.id])],
        "no person's line under the old name is in the tail: no by-name frame"
    );

    // A second rename finds the same owner in storage again.
    let actor = hub.resolve_authority(None, Some(&session));
    hub.rename_profile(&actor, "again").unwrap();
    let live = hub.subscribe("ops").1;
    assert_eq!(
        reply_label_of(&live, to_member.id).as_deref(),
        Some("shared")
    );
    assert_eq!(
        reply_label_of(&live, to_smaster.id).as_deref(),
        Some("again")
    );
    assert_eq!(
        rename_frames(&mut rx),
        vec![Told::ById("again".into(), vec![], vec![to_smaster.id])]
    );

    // After a restart no reply carries a label, and a rename then gives none.
    drop(hub);
    let restarted = critical_restart_hub(store.clone(), "root");
    let mut rx = restarted.subscribe("ops").0;
    let actor =
        restarted.resolve_authority(None, Some(&critical_admin_session(&restarted, "sm_deputy")));
    restarted.rename_profile(&actor, "final").unwrap();
    let after = restarted.subscribe("ops").1;
    assert_eq!(reply_label_of(&after, to_member.id), None);
    assert_eq!(reply_label_of(&after, to_smaster.id), None);
    assert!(
        rename_frames(&mut rx).is_empty(),
        "nothing in the tail changed"
    );
    let stored = store
        .messages_after("ops", 0, 0, u64::MAX, 10)
        .unwrap()
        .unwrap();
    assert_eq!(sender_of(&stored, smaster_words.id), "final");
    assert_eq!(sender_of(&stored, member_words.id), "shared");
}

/// One operation, one key: the sender, whichever credential carried it. An
/// ordinary member over its own-key session, an invite session, the bare invite
/// or the bare membership key; the Master over its session or the root key;
/// a Smaster over its session or its code. Different senders never share a
/// key, and a sender nobody verified is keyed apart from every verified one.
#[test]
fn an_operation_is_keyed_by_its_sender_whichever_credential_carried_it() {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(directory.path().join("op-scope.db").to_str()).unwrap());
    let hub = critical_restart_hub(store, "op-root");
    let code = hub.add_smaster("deputy").unwrap();
    let (ayse, ayse_key) = admit_with_key(&hub, "ayse", "user", "master");
    let invite = hub.new_invite_token();
    add_test_invite(&hub, &invite, "ops", &ayse, 1_767_225_600_000);
    let own = critical_admin_session(&hub, &ayse_key);
    let by_invite = hub
        .create_session_scoped(
            session_request("ayse"),
            Some(&hub.invite_by_token(&invite).expect("a live invite")),
            None,
            false,
            None,
        )
        .unwrap()
        .expect("session recorded")
        .session_token;
    let master = critical_admin_session(&hub, "op-root");
    let deputy = critical_admin_session(&hub, &code);
    let scope = |admin: Option<&str>, invite: Option<&str>, session: Option<&str>, name: &str| {
        let session = session.map(|token| hub.session_identity(Some(token)).expect("live"));
        hub.operation_scope(
            "ops",
            admin,
            invite,
            session.as_ref(),
            SenderType::User,
            name,
        )
    };

    let ayse_principal = hub.member_principal(&ayse.id).expect("ayse's principal");
    for (how, got) in [
        ("own-key session", scope(None, None, Some(&own), "ayse")),
        (
            "invite session",
            scope(None, None, Some(&by_invite), "ayse"),
        ),
        ("invite", scope(None, Some(&invite), None, "ayse")),
        ("membership key", scope(None, Some(&ayse_key), None, "ayse")),
    ] {
        assert_eq!(got.key, format!("pr:{ayse_principal}"), "ayse, {how}");
        assert_eq!(got.principal.as_deref(), Some(ayse_principal.as_str()));
    }
    // What the REST route stored for the same request before this version.
    let own_legacy = scope(None, None, Some(&own), "ayse").legacy;
    assert_eq!(own_legacy.as_deref(), Some("user:ayse"));
    let invite_legacy = scope(None, None, Some(&by_invite), "ayse").legacy;
    assert_eq!(invite_legacy, Some(format!("mb:{}", ayse.id.clone())));

    let master_principal = hub.master_principal().expect("the Master").id;
    let master_key = format!("pr:{master_principal}");
    assert_eq!(scope(None, None, Some(&master), "Master").key, master_key);
    assert_eq!(
        scope(Some("op-root"), None, None, "any label").key,
        master_key
    );

    let deputy_principal = hub
        .session_identity(Some(&deputy))
        .and_then(|session| session.principal_id)
        .expect("the Smaster's principal");
    let deputy_key = format!("pr:{deputy_principal}");
    assert_eq!(scope(None, None, Some(&deputy), "deputy").key, deputy_key);
    assert_eq!(scope(Some(&code), None, None, "deputy").key, deputy_key);

    // Nobody verified: the label alone, under a prefix no verified sender
    // and no earlier version uses.
    let unverified = scope(None, None, None, "ayse");
    assert_eq!(unverified.key, "label:user:ayse");
    assert_eq!(unverified.principal, None);
    assert_eq!(unverified.legacy.as_deref(), Some("user:ayse"));
    let keys: HashSet<String> = [
        format!("pr:{ayse_principal}"),
        master_key,
        deputy_key,
        unverified.key,
    ]
    .into_iter()
    .collect();
    assert_eq!(keys.len(), 4, "two senders share a key: {keys:?}");
}

/// After the upgrade an operation the previous version stored under its REST
/// key is answered with the original message, but only to the sender that
/// key meant: a row that names a different writer, or that a verified sender
/// wrote under its own key now, is not handed to anyone else.
#[test]
fn an_operation_stored_under_the_previous_rest_key_is_answered_once_and_only_to_its_sender() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("op-legacy.db");
    let store = Arc::new(Store::open(path.to_str()).unwrap());
    let hub = critical_restart_hub(store.clone(), "op-root");
    let (ayse, ayse_key) = admit_with_key(&hub, "ayse", "user", "master");
    let ayse_id = hub.member_principal(&ayse.id).expect("principal");
    drop(hub);
    // What earlier versions stored under the REST key: a row with no writer
    // recorded, one whose writer is recorded as somebody else, and one whose
    // writer is recorded as ayse.
    for (id, text, op, writer) in [
        (40, "said before the upgrade", "old-1", None),
        (41, "someone else's", "old-2", Some("pr_someone_else")),
        (
            42,
            "ayse's, and it says so",
            "old-3",
            Some(ayse_id.as_str()),
        ),
    ] {
        store
            .insert_message(
                &protocol::Message {
                    id,
                    room: "ops".into(),
                    sender: "ayse".into(),
                    sender_type: SenderType::User,
                    target: None,
                    text: text.into(),
                    kind: protocol::MessageKind::Say,
                    reply_to: None,
                    reply_to_sender: None,
                    sender_principal: writer.map(Into::into),
                    ts: 1_767_225_600_000 + id,
                    attachments: vec![],
                },
                Some(("user:ayse", op)),
            )
            .unwrap();
    }
    let hub = critical_restart_hub(store, "op-root");
    let own = critical_admin_session(&hub, &ayse_key);
    let ayse_principal = hub.member_principal(&ayse.id);
    let post = |scope: &super::OperationScope, text: &str, op: &str| {
        hub.post(
            "ops",
            PostMessage {
                kind: Default::default(),
                sender: "ayse".into(),
                sender_type: SenderType::User,
                target: None,
                text: text.into(),
                reply_to: None,
                op_id: Some(op.into()),
                attachments: Vec::new(),
            },
            false,
            scope,
            scope.principal.as_deref(),
        )
        .expect("post")
    };
    let identity = hub.session_identity(Some(&own)).expect("live session");
    let ayse_scope =
        hub.operation_scope("ops", None, None, Some(&identity), SenderType::User, "ayse");
    assert_eq!(ayse_scope.principal, ayse_principal);

    // A row that names ayse as its writer answers her re-send.
    let again = post(&ayse_scope, "ayse's, and it says so", "old-3");
    assert_eq!(again.id, 42, "the original answers the re-send");
    // A row another writer wrote is not hers: her own is posted.
    let mine = post(&ayse_scope, "my own old-2", "old-2");
    assert_ne!(mine.id, 41, "a row another writer wrote was handed over");
    // A row that names no writer, under a display name: nothing proves it is
    // hers (a namesake, a re-admitted name or a renamed rank could have
    // written it), so it is neither handed over nor posted again.
    let unclear = hub.post(
        "ops",
        PostMessage {
            kind: Default::default(),
            sender: "ayse".into(),
            sender_type: SenderType::User,
            target: None,
            text: "said before the upgrade".into(),
            reply_to: None,
            op_id: Some("old-1".into()),
            attachments: Vec::new(),
        },
        false,
        &ayse_scope,
        ayse_scope.principal.as_deref(),
    );
    assert!(
        matches!(unclear, Err(super::PostReject::OperationUnclear)),
        "a row with no writer under a name was treated as proven or posted again"
    );
    // A sender nobody verified is its label, before and after: the row
    // answers it.
    let labelled = hub.operation_scope("ops", None, None, None, SenderType::User, "ayse");
    assert_eq!(
        post(&labelled, "said before the upgrade", "old-1").id,
        40,
        "an unverified sender's re-send under its label"
    );

    // A sender nobody verified, wearing the same label, does not meet what
    // the verified one stores from now on.
    let fresh = post(&ayse_scope, "after the upgrade", "new-1");
    let unverified = hub.operation_scope("ops", None, None, None, SenderType::User, "ayse");
    let other = post(&unverified, "an unverified ayse", "new-1");
    assert_ne!(other.id, fresh.id);
    assert_eq!(post(&ayse_scope, "after the upgrade", "new-1").id, fresh.id);
    let stored = hub
        .store
        .messages_after("ops", 0, 0, u64::MAX, 50)
        .unwrap()
        .unwrap();
    let count = |text: &str| stored.iter().filter(|m| m.text == text).count();
    assert_eq!(count("said before the upgrade"), 1);
    assert_eq!(count("after the upgrade"), 1);
}

/// A record an earlier version stored under a name alone, with no writer,
/// while two senders bear that name (a member and a Smaster, both kept from
/// before names were compared without regard to letter case): the server
/// cannot prove whose it is. Neither is answered with it as if their re-send
/// had succeeded, and neither has a second message posted under that
/// operation; each is told, and a new operation of either is posted.
#[test]
fn a_record_with_no_writer_under_a_shared_name_answers_neither_namesake() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("op-shared-name.db");
    let store = Arc::new(Store::open(path.to_str()).unwrap());
    let hub = critical_restart_hub(store.clone(), "op-root");
    let (_, member_key) = admit_with_key(&hub, "shared", "user", "master");
    // No door lets a Smaster take a member's name now, in any letter case.
    // An earlier version did, so the pair is written the way it stored it.
    assert!(hub.add_smaster("shared").is_err(), "the door refuses it");
    drop(hub);
    let smaster_code = "sm_stored-by-an-earlier-version".to_string();
    store
        .add_smaster(&smaster_code, "shared", 1_767_225_600_000)
        .unwrap();
    store
        .insert_message(
            &protocol::Message {
                id: 60,
                room: "ops".into(),
                sender: "shared".into(),
                sender_type: SenderType::User,
                target: None,
                text: "one of them said this before the upgrade".into(),
                kind: protocol::MessageKind::Say,
                reply_to: None,
                reply_to_sender: None,
                sender_principal: None,
                ts: 1_767_225_660_000,
                attachments: vec![],
            },
            Some(("user:shared", "op-X")),
        )
        .unwrap();
    let hub = critical_restart_hub(store, "op-root");
    let post = |scope: &super::OperationScope, text: &str, op: &str| {
        hub.post(
            "ops",
            PostMessage {
                kind: Default::default(),
                sender: "shared".into(),
                sender_type: SenderType::User,
                target: None,
                text: text.into(),
                reply_to: None,
                op_id: Some(op.into()),
                attachments: Vec::new(),
            },
            false,
            scope,
            scope.principal.as_deref(),
        )
    };
    let session = critical_admin_session(&hub, &member_key);
    let identity = hub.session_identity(Some(&session)).expect("live session");
    let as_member = hub.operation_scope(
        "ops",
        None,
        None,
        Some(&identity),
        SenderType::User,
        "shared",
    );
    let as_smaster = hub.operation_scope(
        "ops",
        Some(&smaster_code),
        None,
        None,
        SenderType::User,
        "shared",
    );
    assert!(as_member.principal.is_some() && as_smaster.principal.is_some());
    assert_ne!(
        as_member.principal, as_smaster.principal,
        "two senders, one name"
    );

    for (who, scope) in [("the member", &as_member), ("the Smaster", &as_smaster)] {
        match post(scope, "my own word", "op-X") {
            Err(super::PostReject::OperationUnclear) => {}
            Ok(message) => panic!(
                "{who} was answered as if the re-send had succeeded, with message {}: {:?}",
                message.id, message.text
            ),
            Err(other) => panic!("{who}: {}", other.message()),
        }
    }
    let stored = || {
        hub.store
            .messages_after("ops", 0, 0, u64::MAX, 50)
            .unwrap()
            .unwrap()
    };
    assert_eq!(
        stored().iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![60],
        "nothing was posted under the unclear operation"
    );

    // A new operation of each is its own message, and repeats as itself.
    let first = post(&as_member, "the member, afresh", "op-Y").expect("post");
    let second = post(&as_smaster, "the Smaster, afresh", "op-Y").expect("post");
    assert_ne!(first.id, second.id, "two senders, one op_id, two messages");
    assert_eq!(
        post(&as_member, "the member, afresh", "op-Y")
            .expect("post")
            .id,
        first.id
    );
    assert_eq!(stored().len(), 3);
}
