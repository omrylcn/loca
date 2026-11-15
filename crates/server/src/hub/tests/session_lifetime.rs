//! Every session has a lifetime, expired and ended sessions are swept from
//! memory and storage, and one credential holds at most
//! `SESSIONS_PER_CREDENTIAL` live sessions.

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use protocol::{RoomSettings, SenderType, SessionInfo};

use super::super::{AdminSessionGrant, Hub, HubConfig};
use super::{test_now_ms, TEST_CLOCK_LOCK, TEST_NOW};
use crate::store::{secret_hash, InviteRecord, Store, SESSIONS_PER_CREDENTIAL};

/// A realistic time, in milliseconds since the epoch, later than any real
/// clock these tests run under (a restart restores admin sessions by the real
/// clock before the test clock is installed).
const T0: u64 = 1_800_000_000_000;
const TTL: u64 = Hub::ADMIN_SESSION_TTL_MS;
const HOUR: u64 = 60 * 60 * 1000;

fn at(time: u64) {
    TEST_NOW.store(time, Ordering::Relaxed);
}

/// A hub on `store`. With a `room_token` it is the sandbox (building key);
/// without one, invite-only.
fn hub_on(store: Arc<Store>, room_token: &str) -> Hub {
    let mut hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: room_token.into(),
            require_sessions: true,
            require_invite: room_token.is_empty(),
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        store,
        RoomSettings::default(),
        1,
    );
    hub.now_ms = test_now_ms;
    hub
}

fn persistent(path: &Path, room_token: &str) -> (Hub, Arc<Store>) {
    let store = Arc::new(Store::open_at(path.to_str(), T0).unwrap());
    (hub_on(store.clone(), room_token), store)
}

fn take(hub: &Hub, invite: Option<&InviteRecord>, grant: Option<AdminSessionGrant>) -> SessionInfo {
    // With neither an invite nor a grant, the only key these tests take a
    // session with is the sandbox's building key.
    let building_key = invite.is_none() && grant.is_none();
    hub.create_session_scoped(
        protocol::CreateSession {
            name: "body-name".into(),
            kind: Some(SenderType::Agent),
            runtime: None,
            capabilities: vec![],
        },
        invite,
        grant,
        building_key,
        None,
    )
    .unwrap()
    .expect("the session is recorded")
}

/// A member and an invite for it into `room`, both persisted: the member's
/// key, handed over once at its admission, and the invite's record.
fn member_with_invite(hub: &Hub, name: &str, room: &str) -> (String, InviteRecord) {
    let (member, key) = super::admit_with_key(hub, name, "agent", "master");
    let invite_key = hub.new_invite_token();
    let invite = InviteRecord::issued(
        &invite_key,
        room,
        &member.id,
        &member.name,
        &member.kind,
        T0,
        "master",
    );
    hub.add_invite(invite.clone(), &invite_key).unwrap();
    (key, invite)
}

fn invite_session(hub: &Hub, invite: &InviteRecord) -> String {
    take(hub, Some(invite), None).session_token
}

/// A session taken with the member's own key (`mb_`, no invite).
fn member_session(hub: &Hub, key: &str) -> String {
    take(hub, None, hub.admin_session_grant(Some(key), TTL)).session_token
}

fn live(hub: &Hub, token: &str) -> bool {
    hub.session_identity(Some(token)).is_some()
}

fn in_memory(hub: &Hub, token: &str) -> bool {
    hub.sessions
        .lock()
        .unwrap()
        .contains_key(&secret_hash(token))
}

/// The stored row of a session: `Some(revoked)` when present.
fn row(path: &Path, token: &str) -> Option<bool> {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT revoked_at IS NOT NULL FROM principal_sessions WHERE id = ?1",
            [secret_hash(token)],
            |r| r.get(0),
        )
        .ok()
}

fn live_rows_of_credential(path: &Path, credential_id: &str) -> i64 {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM principal_sessions
             WHERE credential_id = ?1 AND revoked_at IS NULL",
            [credential_id],
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn an_invite_session_and_a_sandbox_session_end_exactly_twelve_hours_after_they_were_taken() {
    let _clock = TEST_CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let directory = tempfile::tempdir().unwrap();
    let (hub, _store) = persistent(&directory.path().join("lifetime.db"), "sandbox-key");
    at(T0);
    let (_, invite) = member_with_invite(&hub, "lifetime-worker", "work");
    let by_invite = take(&hub, Some(&invite), None);
    // The sandbox's building key carries no member: the route passes no
    // invite and no grant, exactly as here.
    let by_building_key = take(&hub, None, None);
    assert_eq!(by_invite.expires_at, Some(T0 + TTL));
    assert_eq!(by_building_key.expires_at, Some(T0 + TTL));

    at(T0 + TTL - 1);
    assert!(live(&hub, &by_invite.session_token));
    assert!(live(&hub, &by_building_key.session_token));

    at(T0 + TTL);
    assert!(!live(&hub, &by_invite.session_token));
    assert!(!live(&hub, &by_building_key.session_token));
}

#[test]
fn the_sweep_removes_ended_sessions_from_memory_and_storage_and_a_restart_keeps_the_live_ones() {
    let _clock = TEST_CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sweep.db");
    let (hub, store) = persistent(&path, "");
    at(T0);
    let (key, invite) = member_with_invite(&hub, "sweep-worker", "work");
    let expiring = member_session(&hub, &key);
    let signed_out = member_session(&hub, &key);
    assert!(hub.revoke_session(&signed_out).unwrap());
    // An invite session is filed under its invite and lives in memory only.
    let seated = invite_session(&hub, &invite);
    assert!(in_memory(&hub, &seated));
    assert_eq!(
        row(&path, &seated),
        None,
        "an invite session has no stored row"
    );
    let master = take(&hub, None, Some(hub.master_pairing_grant(24 * HOUR))).session_token;
    at(T0 + HOUR);
    let later = member_session(&hub, &key);
    assert_eq!(
        row(&path, &signed_out),
        Some(true),
        "sign-out marks the row"
    );

    // Nobody presents anything; the sweep alone ends what has expired.
    at(T0 + TTL);
    assert_eq!(hub.sweep_sessions().unwrap(), 2);
    assert!(!in_memory(&hub, &expiring));
    assert!(!in_memory(&hub, &seated), "the invite session expired too");
    assert!(!live(&hub, &seated));
    assert_eq!(row(&path, &expiring), None);
    assert_eq!(row(&path, &signed_out), None);
    for token in [&later, &master] {
        assert!(in_memory(&hub, token));
        assert_eq!(row(&path, token), Some(false));
        assert!(live(&hub, token));
    }
    // Nothing more to do: the sweep says so.
    assert_eq!(hub.sweep_sessions().unwrap(), 0);
    // `/profile` still names the credential a live session was taken with.
    let credential = store.credential_id_for_secret(&key).unwrap();
    assert_eq!(
        hub.credential_id_for_request(None, Some(&later)).as_deref(),
        Some(credential.as_str())
    );

    // A restart while `later` is live keeps it, and the Master's session too.
    drop(hub);
    drop(store);
    let store = Arc::new(Store::open_at(path.to_str(), T0 + TTL + 1).unwrap());
    let hub = hub_on(store.clone(), "");
    at(T0 + TTL + 1);
    assert_eq!(row(&path, &later), Some(false));
    assert!(live(&hub, &master), "a live Master session is restored");

    // A restart after both have ended removes them at the open.
    drop(hub);
    drop(store);
    let store = Arc::new(Store::open_at(path.to_str(), T0 + 24 * HOUR).unwrap());
    let hub = hub_on(store, "");
    at(T0 + 24 * HOUR);
    assert_eq!(row(&path, &later), None);
    assert_eq!(row(&path, &master), None);
    assert!(!live(&hub, &master));
}

#[test]
fn rows_of_never_expiring_sessions_from_before_the_upgrade_are_deleted_at_the_first_open() {
    let _clock = TEST_CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("upgrade.db");
    let (hub, store) = persistent(&path, "");
    at(T0);
    let (key, invite) = member_with_invite(&hub, "upgrade-worker", "work");
    let kept = member_session(&hub, &key);
    let old = invite_session(&hub, &invite);
    drop(hub);
    drop(store);
    // What the previous version wrote for an invite session: no expiry.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE principal_sessions SET expires_at = 0 WHERE id = ?1",
            [secret_hash(&old)],
        )
        .unwrap();

    let store = Arc::new(Store::open_at(path.to_str(), T0 + 1).unwrap());
    assert_eq!(row(&path, &old), None, "the unbounded row goes at the open");
    assert_eq!(row(&path, &kept), Some(false), "a bounded live row stays");
    let hub = hub_on(store, "");
    at(T0 + 1);
    assert!(!live(&hub, &old));
}

#[test]
fn a_seventeenth_session_ends_the_oldest_and_only_the_oldest() {
    let _clock = TEST_CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cap.db");
    let (hub, store) = persistent(&path, "");
    at(T0);
    let (key, invite) = member_with_invite(&hub, "cap-worker", "work");
    let principal = hub
        .member_principal(&hub.member_by_name("cap-worker").unwrap().id)
        .unwrap();
    let (_, laptop) = hub.create_profile_credential(&principal, "laptop").unwrap();
    let credential = store.credential_id_for_secret(&key).unwrap();

    // Sessions taken with the member's own key are counted under that key.
    let mut taken = Vec::new();
    for i in 0..SESSIONS_PER_CREDENTIAL as u64 {
        at(T0 + i);
        taken.push(member_session(&hub, &key));
    }
    // Another credential of the same principal has its own count, and so
    // does the member's invite (see the next test).
    let other: Vec<String> = (0..5).map(|_| member_session(&hub, &laptop)).collect();
    let seated: Vec<String> = (0..5).map(|_| invite_session(&hub, &invite)).collect();
    for token in taken.iter().chain(&other).chain(&seated) {
        assert!(live(&hub, token));
    }

    at(T0 + 100);
    let newest = member_session(&hub, &key);
    assert!(live(&hub, &newest));
    assert!(!live(&hub, &taken[0]), "the oldest is ended");
    assert!(!in_memory(&hub, &taken[0]));
    assert_eq!(row(&path, &taken[0]), Some(true));
    for token in taken[1..].iter().chain(&other).chain(&seated) {
        assert!(live(&hub, token), "only the oldest is ended");
    }
    assert_eq!(
        live_rows_of_credential(&path, &credential),
        SESSIONS_PER_CREDENTIAL as i64
    );
}

/// An invite session is filed under its invite, not under a credential, so the
/// credential's cap never counts it. The invite has the same cap of its own:
/// its seventeenth session ends its oldest, and touches neither the member's
/// key sessions nor another invite's.
#[test]
fn an_invite_holds_at_most_the_cap_and_its_seventeenth_session_ends_its_oldest() {
    let _clock = TEST_CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cap-invite.db");
    let (hub, store) = persistent(&path, "");
    at(T0);
    let (key, invite) = member_with_invite(&hub, "seat-worker", "work");
    let (_, other_invite) = member_with_invite(&hub, "seat-neighbour", "work");
    let credential = store.credential_id_for_secret(&key).unwrap();

    let own: Vec<String> = (0..3).map(|_| member_session(&hub, &key)).collect();
    let neighbour: Vec<String> = (0..3)
        .map(|_| invite_session(&hub, &other_invite))
        .collect();
    let mut taken = Vec::new();
    for i in 0..SESSIONS_PER_CREDENTIAL as u64 {
        at(T0 + 10 + i);
        taken.push(invite_session(&hub, &invite));
    }
    for token in taken.iter().chain(&own).chain(&neighbour) {
        assert!(live(&hub, token));
        assert!(in_memory(&hub, token));
    }
    assert_eq!(
        row(&path, &taken[0]),
        None,
        "an invite session has no stored row"
    );

    at(T0 + 100);
    let newest = invite_session(&hub, &invite);
    assert!(live(&hub, &newest));
    assert!(!live(&hub, &taken[0]), "the invite's oldest is ended");
    assert!(!in_memory(&hub, &taken[0]));
    for token in taken[1..].iter().chain(&own).chain(&neighbour) {
        assert!(live(&hub, token), "only the invite's oldest is ended");
    }
    let held = taken
        .iter()
        .chain(std::iter::once(&newest))
        .filter(|token| in_memory(&hub, token))
        .count();
    assert_eq!(held, SESSIONS_PER_CREDENTIAL);
    assert_eq!(live_rows_of_credential(&path, &credential), 3);
}

#[test]
fn master_pairing_sessions_count_under_the_root_recovery_credential() {
    let _clock = TEST_CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cap-master.db");
    let (hub, store) = persistent(&path, "");
    let root = store.credential_id_for_secret("MASTER").unwrap();
    let mut taken = Vec::new();
    for i in 0..=SESSIONS_PER_CREDENTIAL as u64 {
        at(T0 + i);
        taken.push(take(&hub, None, Some(hub.master_pairing_grant(TTL))).session_token);
    }
    assert!(!live(&hub, &taken[0]));
    assert!(taken[1..].iter().all(|token| live(&hub, token)));
    assert_eq!(
        live_rows_of_credential(&path, &root),
        SESSIONS_PER_CREDENTIAL as i64
    );
    // The ended browser session is not restored after a restart.
    drop(hub);
    drop(store);
    let hub = hub_on(
        Arc::new(Store::open_at(path.to_str(), T0 + 100).unwrap()),
        "",
    );
    at(T0 + 100);
    assert!(!live(&hub, &taken[0]));
    assert!(live(&hub, &taken[1]));
}

#[test]
fn concurrent_takes_never_leave_more_than_the_cap_and_end_the_oldest() {
    let _clock = TEST_CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cap-race.db");
    let (hub, store) = persistent(&path, "");
    at(T0);
    let (_key, invite) = member_with_invite(&hub, "race-worker", "work");
    let _ = &store;
    let mut original = Vec::new();
    for i in 0..SESSIONS_PER_CREDENTIAL as u64 {
        at(T0 + i);
        original.push(invite_session(&hub, &invite));
    }
    at(T0 + 1_000);
    let racers = 8;
    let barrier = Arc::new(std::sync::Barrier::new(racers));
    let threads: Vec<_> = (0..racers)
        .map(|_| {
            let hub = hub.clone();
            let invite = invite.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                invite_session(&hub, &invite)
            })
        })
        .collect();
    let raced: Vec<String> = threads.into_iter().map(|t| t.join().unwrap()).collect();

    assert!(raced.iter().all(|token| live(&hub, token)));
    assert!(original[..racers].iter().all(|token| !live(&hub, token)));
    assert!(original[racers..].iter().all(|token| live(&hub, token)));
    let in_memory_count = original
        .iter()
        .chain(&raced)
        .filter(|token| in_memory(&hub, token))
        .count();
    assert_eq!(in_memory_count, SESSIONS_PER_CREDENTIAL);
}

#[test]
fn sessions_without_a_credential_row_are_not_capped() {
    let _clock = TEST_CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let directory = tempfile::tempdir().unwrap();
    let (hub, _store) = persistent(&directory.path().join("sandbox.db"), "sandbox-key");
    at(T0);
    let taken: Vec<String> = (0..SESSIONS_PER_CREDENTIAL + 4)
        .map(|_| take(&hub, None, None).session_token)
        .collect();
    assert!(taken.iter().all(|token| live(&hub, token)));
}

#[test]
fn a_storage_error_in_the_sweep_is_returned_and_memory_is_still_swept() {
    let _clock = TEST_CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sweep-error.db");
    let (hub, _store) = persistent(&path, "");
    at(T0);
    let (key, _invite) = member_with_invite(&hub, "error-worker", "work");
    let expiring = member_session(&hub, &key);
    let guard = rusqlite::Connection::open(&path).unwrap();
    guard
        .execute_batch(
            "CREATE TRIGGER refuse_session_delete BEFORE DELETE ON principal_sessions
             BEGIN SELECT RAISE(ABORT, 'disk says no'); END;",
        )
        .unwrap();

    at(T0 + TTL);
    assert!(
        hub.sweep_sessions().is_err(),
        "a failed delete is an error, not \"nothing to sweep\""
    );
    assert!(!in_memory(&hub, &expiring), "memory is swept regardless");
    assert_eq!(row(&path, &expiring), Some(false), "the row is still there");

    guard
        .execute_batch("DROP TRIGGER refuse_session_delete;")
        .unwrap();
    assert_eq!(hub.sweep_sessions().unwrap(), 1);
    assert_eq!(row(&path, &expiring), None);
}

/// Guard: ending a seat, a membership's access or a sign-out works as it did;
/// the sweep only deletes what those already ended, and resuming a member
/// revives nothing.
#[test]
fn revoke_suspend_and_sign_out_still_end_sessions_and_the_sweep_does_not_revive_them() {
    let _clock = TEST_CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("guards.db");
    let (hub, _store) = persistent(&path, "");
    at(T0);
    let (key, invite) = member_with_invite(&hub, "guard-worker", "work");
    let member_id = hub.member_by_name("guard-worker").unwrap().id;
    let elsewhere_invite = hub.new_invite_token();
    let other_room = InviteRecord::issued(
        &elsewhere_invite,
        "elsewhere",
        &member_id,
        "guard-worker",
        "agent",
        T0,
        "master",
    );
    hub.add_invite(other_room.clone(), &elsewhere_invite)
        .unwrap();
    let seat = invite_session(&hub, &invite);
    let elsewhere = invite_session(&hub, &other_room);
    let own = member_session(&hub, &key);
    let signed_out = member_session(&hub, &key);

    assert!(hub.revoke_session(&signed_out).unwrap());
    assert!(!hub.revoke_session(&signed_out).unwrap());
    assert!(hub.revoke_invite_by_hash(&invite.secret_hash).unwrap());
    assert!(!live(&hub, &seat));
    assert!(live(&hub, &elsewhere), "another loca's seat survives");
    assert!(live(&hub, &own));

    let reference = member_id;
    assert!(hub.set_member_suspended(&reference, true, "test").unwrap());
    assert!(!live(&hub, &elsewhere));
    assert!(!live(&hub, &own));
    hub.sweep_sessions().unwrap();
    assert!(hub.set_member_suspended(&reference, false, "").unwrap());
    for token in [&seat, &elsewhere, &own, &signed_out] {
        assert!(!live(&hub, token), "nothing ended comes back");
        assert_eq!(row(&path, token), None, "and the sweep deleted it");
    }
    // The member signs in again as before.
    assert!(live(&hub, &member_session(&hub, &key)));
}
