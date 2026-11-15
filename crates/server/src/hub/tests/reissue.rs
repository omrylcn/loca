//! Invite re-issue through the hub: one transaction in the store, one swap in
//! memory, the old seat's sessions and socket ended, one Lobby `called`, and
//! the seat count untouched.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use protocol::{RoomSettings, SenderType, ServerFrame};
use tokio::sync::broadcast::error::TryRecvError;

use super::super::{
    BuildingRole, Hub, HubConfig, InviteError, LobbyEvent, ReissueError, RequestAuthority,
};
use super::{revoke_invite, Minted};
use crate::store::{secret_hash, InviteRecord, Store};

const ROOT: &str = "reissue-fixture-root";

fn hub_on(store: Arc<Store>) -> Hub {
    Hub::build(
        HubConfig {
            admin_token: ROOT.into(),
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

fn master(hub: &Hub) -> RequestAuthority {
    hub.resolve_authority(Some(ROOT), None)
}

fn smaster(name: &str) -> RequestAuthority {
    RequestAuthority {
        principal_id: None,
        display_name: Some(name.into()),
        kind: Some(SenderType::User),
        building_role: Some(BuildingRole::Smaster),
    }
}

fn open(hub: &Hub, room: &str) {
    hub.create_loca(room, &master(hub), None).expect("loca");
}

fn invite_session(hub: &Hub, token: &str) -> String {
    let invite = hub.invite_by_token(token).expect("live invite");
    hub.create_session_scoped(
        protocol::CreateSession {
            name: "body-name".into(),
            kind: Some(SenderType::Agent),
            runtime: None,
            capabilities: vec![],
        },
        Some(&invite),
        None,
        false,
        None,
    )
    .expect("session")
    .expect("the invite still stands")
    .session_token
}

/// Every `called` waiting on the Lobby bus, as (member, room, token).
fn drain_calls(
    lobby: &mut tokio::sync::broadcast::Receiver<LobbyEvent>,
) -> Vec<(String, String, String)> {
    let mut calls = Vec::new();
    loop {
        match lobby.try_recv() {
            Ok(LobbyEvent::Called {
                member,
                room,
                token,
            }) => calls.push((member, room, token)),
            Ok(_) => {}
            Err(TryRecvError::Empty) | Err(TryRecvError::Closed) => return calls,
            Err(TryRecvError::Lagged(_)) => panic!("lobby bus lagged"),
        }
    }
}

/// The live invites of `name` as stored: (the invite's hash, its issuer).
fn live_rows(db: &std::path::Path, name: &str) -> Vec<(String, String)> {
    let sql = rusqlite::Connection::open(db).unwrap();
    let mut st = sql
        .prepare(
            "SELECT secret_hash, issued_by FROM invites
             WHERE name = ?1 AND revoked_at IS NULL ORDER BY issued_at",
        )
        .unwrap();
    st.query_map([name], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn reissue_swaps_one_invite_for_another_everywhere() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("swap.db");
    let hub = hub_on(Arc::new(Store::open(db.to_str()).unwrap()));
    open(&hub, "parlor");
    let member = hub.admit_member("worker", "agent", "master").unwrap();
    let old = Minted::from(
        hub.invite_member_to_room(&member.id, "parlor", "smaster:deputy")
            .unwrap(),
    );
    let session = invite_session(&hub, &old.token);
    let mut lobby = hub.subscribe_lobby();
    let (mut room_bus, _) = hub.subscribe("parlor");

    let fresh = Minted::from(
        hub.reissue_invite(&member.id, "parlor", &master(&hub))
            .expect("re-issue"),
    );

    assert_ne!(fresh.token, old.token);
    assert_eq!(
        (&fresh.room, &fresh.member, &fresh.name, &fresh.kind),
        (&old.room, &old.member, &old.name, &old.kind)
    );
    assert_eq!(fresh.issued_by, "smaster:deputy", "the issuer is kept");
    assert!(hub.invite_for("parlor", Some(&old.token)).is_none());
    assert!(hub.invite_for("parlor", Some(&fresh.token)).is_some());
    assert!(
        hub.session_identity(Some(&session)).is_none(),
        "the old invite's session ended"
    );
    assert_eq!(hub.invites_of("parlor").len(), 1);
    assert_eq!(
        live_rows(&db, "worker"),
        vec![(secret_hash(&fresh.token), "smaster:deputy".to_string())]
    );
    assert_eq!(
        drain_calls(&mut lobby),
        vec![(member.id.clone(), "parlor".to_string(), fresh.token.clone())],
        "exactly one called, carrying the new invite"
    );
    // The loca's sockets are woken to re-check their credentials (the one
    // seated with the old invite then closes). Nobody is told `kicked`: the
    // seat goes on under the fresh invite.
    let frames: Vec<ServerFrame> = std::iter::from_fn(|| room_bus.try_recv().ok()).collect();
    assert!(
        frames
            .iter()
            .any(|f| matches!(f, ServerFrame::Members { .. })),
        "the loca's sockets are woken"
    );
    assert!(
        !frames
            .iter()
            .any(|f| matches!(f, ServerFrame::Kicked { .. })),
        "a re-issue is not a kick"
    );

    // A restart reads the store: the old invite stays dead, the new one works.
    drop(hub);
    let hub = hub_on(Arc::new(Store::open(db.to_str()).unwrap()));
    assert!(hub.invite_for("parlor", Some(&old.token)).is_none());
    assert!(hub.invite_for("parlor", Some(&fresh.token)).is_some());
}

#[test]
fn a_full_loca_reissues_and_a_racing_call_never_takes_the_seat() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub_on(Arc::new(
        Store::open(dir.path().join("full.db").to_str()).unwrap(),
    ));
    hub.set_loca_capacity(2);
    open(&hub, "packed");
    let a = hub.admit_member("sat-a", "agent", "master").unwrap();
    let b = hub.admit_member("sat-b", "agent", "master").unwrap();
    let waiting = hub.admit_member("waiting", "agent", "master").unwrap();
    hub.invite_member_to_room(&a.id, "packed", "master")
        .unwrap();
    hub.invite_member_to_room(&b.id, "packed", "master")
        .unwrap();
    for _ in 0..50 {
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let caller = hub.clone();
        let ready = barrier.clone();
        let waiting_token = waiting.id.clone();
        let racer = std::thread::spawn(move || {
            ready.wait();
            caller.invite_member_to_room(&waiting_token, "packed", "master")
        });
        barrier.wait();
        hub.reissue_invite(&a.id, "packed", &master(&hub))
            .expect("a full loca re-issues");
        assert_eq!(racer.join().unwrap().unwrap_err(), InviteError::Full);
        let mut names: Vec<String> = hub
            .invites_of("packed")
            .into_iter()
            .map(|i| i.name)
            .collect();
        names.sort();
        assert_eq!(names, ["sat-a", "sat-b"]);
    }
}

#[test]
fn a_failed_write_changes_nothing_anywhere() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("fail.db");
    let hub = hub_on(Arc::new(Store::open(db.to_str()).unwrap()));
    open(&hub, "parlor");
    let member = hub.admit_member("worker", "agent", "master").unwrap();
    let old = Minted::from(
        hub.invite_member_to_room(&member.id, "parlor", "master")
            .unwrap(),
    );
    let session = invite_session(&hub, &old.token);
    let mut lobby = hub.subscribe_lobby();
    let (mut room_bus, _) = hub.subscribe("parlor");
    let sql = rusqlite::Connection::open(&db).unwrap();
    for trigger in [
        "CREATE TRIGGER fail_reissue BEFORE INSERT ON invites
         BEGIN SELECT RAISE(ABORT, 'injected'); END;",
        "CREATE TRIGGER fail_reissue BEFORE UPDATE ON invites
         BEGIN SELECT RAISE(ABORT, 'injected'); END;",
    ] {
        sql.execute_batch(trigger).unwrap();
        assert_eq!(
            hub.reissue_invite(&member.id, "parlor", &master(&hub))
                .err(),
            Some(ReissueError::Invite(InviteError::Storage))
        );
        sql.execute_batch("DROP TRIGGER fail_reissue").unwrap();
        assert!(hub.invite_for("parlor", Some(&old.token)).is_some());
        assert!(hub.session_identity(Some(&session)).is_some());
        assert_eq!(hub.invites_of("parlor").len(), 1);
        assert_eq!(
            live_rows(&db, "worker"),
            vec![(secret_hash(&old.token), "master".to_string())]
        );
        assert!(drain_calls(&mut lobby).is_empty(), "no frame is sent");
        assert!(
            std::iter::from_fn(|| room_bus.try_recv().ok())
                .next()
                .is_none(),
            "the loca's sockets are not disturbed"
        );
    }
}

/// A revoke that commits first wins: the re-issue finds the old row already
/// ended and mints nothing, so a re-issue never undoes a revoke.
#[test]
fn a_reissue_never_revives_an_invite_ended_underneath_it() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("ended.db");
    let store = Arc::new(Store::open(db.to_str()).unwrap());
    let hub = hub_on(store.clone());
    open(&hub, "parlor");
    let member = hub.admit_member("worker", "agent", "master").unwrap();
    let old = Minted::from(
        hub.invite_member_to_room(&member.id, "parlor", "master")
            .unwrap(),
    );
    // Ended in the store only, as a concurrent revoke is between its store
    // write and its cache update.
    store
        .revoke_invite_by_hash(&secret_hash(&old.token), 1)
        .unwrap();
    assert_eq!(
        hub.reissue_invite(&member.id, "parlor", &master(&hub))
            .err(),
        Some(ReissueError::Invite(InviteError::Storage))
    );
    assert!(live_rows(&db, "worker").is_empty(), "nothing was minted");
}

#[test]
fn reissue_takes_the_authority_ending_the_invite_takes() {
    let hub = hub_on(Arc::new(Store::open(None).unwrap()));
    open(&hub, "parlor");
    let mut lobby = hub.subscribe_lobby();
    let cases = [
        ("by-master", "master", false, true),
        ("by-deputy", "smaster:deputy", true, true),
        ("by-other-deputy", "smaster:other", true, true),
        ("by-move", "group-move", false, true),
        ("by-legacy", "legacy-unknown", false, true),
    ];
    for (name, issuer, smaster_may, master_may) in cases {
        let member = hub.admit_member(name, "agent", "master").unwrap();
        let fresh_invite = Minted::from(
            hub.invite_member_to_room(&member.id, "parlor", issuer)
                .unwrap(),
        );
        drain_calls(&mut lobby);
        let by_smaster = hub.reissue_invite(&member.id, "parlor", &smaster("deputy"));
        assert_eq!(by_smaster.is_ok(), smaster_may, "{issuer}: {by_smaster:?}");
        if !smaster_may {
            assert_eq!(by_smaster.err(), Some(ReissueError::Forbidden));
            assert!(hub
                .invite_for("parlor", Some(&fresh_invite.token))
                .is_some());
            assert!(drain_calls(&mut lobby).is_empty());
        }
        let by_master = hub.reissue_invite(&member.id, "parlor", &master(&hub));
        assert_eq!(by_master.is_ok(), master_may, "{issuer}: {by_master:?}");
        assert_eq!(by_master.unwrap().0.issued_by, issuer);
    }
    // The same rule the revoke route asks.
    for issuer in ["master", "group-move", "birebir"] {
        let invite = InviteRecord::issued("dv_probe", "parlor", "", "probe", "agent", 0, issuer);
        assert!(!hub.may_end_invite(&smaster("deputy"), &invite));
        assert!(hub.may_end_invite(&master(&hub), &invite));
    }
}

#[test]
fn reissue_is_refused_without_an_invite_and_for_a_suspended_member() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("refused.db");
    let hub = hub_on(Arc::new(Store::open(db.to_str()).unwrap()));
    open(&hub, "parlor");
    let mut lobby = hub.subscribe_lobby();
    let idle = hub.admit_member("idle", "agent", "master").unwrap();
    assert_eq!(
        hub.reissue_invite(&idle.id, "parlor", &master(&hub)).err(),
        Some(ReissueError::NotHeld)
    );
    assert!(
        hub.invites_of("parlor").is_empty(),
        "never a silent first call"
    );
    assert!(drain_calls(&mut lobby).is_empty());

    let held = hub.admit_member("held", "agent", "master").unwrap();
    hub.invite_member_to_room(&held.id, "parlor", "master")
        .unwrap();
    drain_calls(&mut lobby);
    assert!(hub
        .set_member_suspended(&held.id.clone(), true, "test")
        .unwrap());
    let call = hub
        .invite_member_to_room(&held.id, "parlor", "master")
        .unwrap_err();
    assert_eq!(
        hub.reissue_invite(&held.id, "parlor", &master(&hub)).err(),
        Some(ReissueError::Invite(call)),
        "refused the way a call is"
    );
    assert!(
        live_rows(&db, "held").is_empty(),
        "nothing minted for a suspended member"
    );
    assert!(drain_calls(&mut lobby).is_empty());

    assert_eq!(
        hub.reissue_invite(&idle.id, "nowhere", &master(&hub)).err(),
        Some(ReissueError::Invite(InviteError::MissingRoom))
    );
}

/// Rows from before the one-invite-per-loca rule may hold two invites for one
/// member and loca. A re-issue ends both and keeps the higher issuer, so it
/// never leaves a second live invite or lowers who may end it.
#[test]
fn a_reissue_ends_every_invite_the_member_holds_there() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("legacy.db");
    let hub = hub_on(Arc::new(Store::open(db.to_str()).unwrap()));
    open(&hub, "parlor");
    let member = hub.admit_member("worker", "agent", "master").unwrap();
    let mut tokens = Vec::new();
    for (at, issuer) in [(1, "smaster:deputy"), (2, "master")] {
        let token = hub.new_invite_token();
        let invite = InviteRecord::issued(
            &token,
            "parlor",
            &member.id,
            &member.name,
            &member.kind,
            at,
            issuer,
        );
        hub.add_invite(invite, &token).unwrap();
        tokens.push(token);
    }
    assert_eq!(
        hub.reissue_invite(&member.id, "parlor", &smaster("deputy"))
            .err(),
        Some(ReissueError::Forbidden),
        "one of them is the Master's"
    );
    let fresh = Minted::from(
        hub.reissue_invite(&member.id, "parlor", &master(&hub))
            .unwrap(),
    );
    assert_eq!(fresh.issued_by, "master");
    for token in &tokens {
        assert!(hub.invite_for("parlor", Some(token)).is_none());
    }
    assert_eq!(
        live_rows(&db, "worker"),
        vec![(secret_hash(&fresh.token), "master".to_string())]
    );
}

/// Memory-only Buildings re-issue too: the swap is the whole record there.
#[test]
fn memory_only_reissue_swaps_the_invite() {
    let hub = hub_on(Arc::new(Store::open(None).unwrap()));
    open(&hub, "parlor");
    let member = hub.admit_member("worker", "agent", "master").unwrap();
    let old = Minted::from(
        hub.invite_member_to_room(&member.id, "parlor", "master")
            .unwrap(),
    );
    let fresh = Minted::from(
        hub.reissue_invite(&member.id, "parlor", &master(&hub))
            .unwrap(),
    );
    assert!(hub.invite_for("parlor", Some(&old.token)).is_none());
    assert!(hub.invite_for("parlor", Some(&fresh.token)).is_some());
    assert_eq!(hub.invites_of("parlor").len(), 1);
}

/// Two re-issues of one seat: the first is held right after its swap, before
/// its clean-up and delivery, while the second is started. The invite lock is
/// still held at that point, so the second waits; the Lobby bus then carries
/// the two fresh invites in the order they became live, the last one is the
/// invite the server holds, and a session taken with it survives.
#[test]
fn a_held_reissue_is_delivered_before_the_next_one_of_the_seat() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub_on(Arc::new(
        Store::open(dir.path().join("order.db").to_str()).unwrap(),
    ));
    open(&hub, "parlor");
    let member = hub.admit_member("worker", "agent", "master").unwrap();
    hub.invite_member_to_room(&member.id, "parlor", "master")
        .unwrap();
    let mut lobby = hub.subscribe_lobby();

    // The first re-issue stops in the hook once; later ones pass through.
    let (paused_tx, paused) = std::sync::mpsc::channel::<()>();
    let (go, go_rx) = std::sync::mpsc::channel::<()>();
    let once = Arc::new(Mutex::new(Some((paused_tx, go_rx))));
    hub.set_reissue_swapped_hook(Some(Arc::new(move || {
        // Take it out first: the guard must not be held while waiting, or a
        // second re-issue reaching the hook would wait on it.
        let held = once.lock().unwrap().take();
        if let Some((paused_tx, go_rx)) = held {
            paused_tx.send(()).unwrap();
            go_rx.recv().unwrap();
        }
    })));

    let (h1, m1, a1) = (hub.clone(), member.id.clone(), master(&hub));
    let first = std::thread::spawn(move || h1.reissue_invite(&m1, "parlor", &a1));
    paused.recv().unwrap();
    assert!(
        hub.member_lifecycle.try_lock().is_err(),
        "the invite lock is held from the swap through the delivery"
    );

    let (started_tx, started) = std::sync::mpsc::channel::<()>();
    let (h2, m2, a2) = (hub.clone(), member.id.clone(), master(&hub));
    let second = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        h2.reissue_invite(&m2, "parlor", &a2)
    });
    started.recv().unwrap();
    go.send(()).unwrap();
    let a = Minted::from(first.join().unwrap().expect("first re-issue"));
    let b = Minted::from(second.join().unwrap().expect("second re-issue"));
    hub.set_reissue_swapped_hook(None);

    let delivered: Vec<String> = drain_calls(&mut lobby)
        .into_iter()
        .map(|(_, _, token)| token)
        .collect();
    assert_eq!(
        delivered,
        vec![a.token.clone(), b.token.clone()],
        "delivered in the order the invites became live"
    );
    assert!(hub.invite_for("parlor", Some(&a.token)).is_none());
    assert!(hub.call_is_current(&member.id, "parlor", &b.token));
    let session = invite_session(&hub, &b.token);
    assert!(hub.session_identity(Some(&session)).is_some());
}

/// The Lobby forwards a `called` only while its invite is the seat's live one:
/// a call followed by a revoke, or an invite a re-issue replaced, is stale.
#[test]
fn only_a_live_invite_is_a_current_call() {
    let hub = hub_on(Arc::new(Store::open(None).unwrap()));
    open(&hub, "parlor");
    open(&hub, "other");
    let member = hub.admit_member("worker", "agent", "master").unwrap();
    let stranger = hub.admit_member("stranger", "agent", "master").unwrap();
    let first = Minted::from(
        hub.invite_member_to_room(&member.id, "parlor", "master")
            .unwrap(),
    );
    assert!(hub.call_is_current(&member.id, "parlor", &first.token));
    assert!(!hub.call_is_current(&stranger.id, "parlor", &first.token));
    assert!(!hub.call_is_current(&member.id, "other", &first.token));

    let fresh = Minted::from(
        hub.reissue_invite(&member.id, "parlor", &master(&hub))
            .unwrap(),
    );
    assert!(!hub.call_is_current(&member.id, "parlor", &first.token));
    assert!(hub.call_is_current(&member.id, "parlor", &fresh.token));

    assert!(revoke_invite(&hub, &fresh.token));
    assert!(!hub.call_is_current(&member.id, "parlor", &fresh.token));
}
