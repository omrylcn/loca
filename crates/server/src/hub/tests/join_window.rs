//! The delivery window of a join request's key, through the hub: a late
//! collection, the background sweep, and everything inside the window.

use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use protocol::RoomSettings;

use super::super::{Approve, Hub, HubConfig, JoinRequestCreate};
use super::{test_now_ms, TEST_CLOCK_LOCK, TEST_NOW};
use crate::store::{BootstrapClaim, Store, JOIN_BOOTSTRAP_WINDOW_MS};

/// A realistic approval time, in milliseconds since the epoch.
const APPROVED_AT: u64 = 1_800_000_000_000;

fn hub_on(store: Arc<Store>) -> Hub {
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
    hub
}

/// A persistent hub with one request named `name`, approved at `APPROVED_AT`.
fn approved(path: &std::path::Path, name: &str) -> (Hub, Arc<Store>, String, String) {
    let store = Arc::new(Store::open_at(path.to_str(), APPROVED_AT).unwrap());
    store
        .ensure_master_principal("MASTER", "operator", 1)
        .unwrap();
    let hub = hub_on(store.clone());
    TEST_NOW.store(APPROVED_AT, Ordering::Relaxed);
    hub.mint_admission_stock(1, 3_600_000);
    let JoinRequestCreate::Created {
        request_id,
        request_secret,
    } = hub.create_join_request(name, "agent", "10.0.0.7".parse().unwrap())
    else {
        panic!("created");
    };
    assert!(matches!(
        hub.approve_join_request(&request_id, "operator"),
        Approve::Approved
    ));
    (hub, store, request_id, request_secret)
}

fn at(time: u64) {
    TEST_NOW.store(time, Ordering::Relaxed);
}

fn collect(hub: &Hub, id: &str, secret: &str) -> BootstrapClaim {
    hub.claim_join_request_bootstrap(id, secret)
        .expect("the store answers")
}

fn shown(hub: &Hub, id: &str, secret: &str) -> (String, bool, bool) {
    let view = hub.join_request_view(id, secret).unwrap().unwrap();
    (view.status, view.bootstrap_ready, view.bootstrap_expired)
}

#[test]
fn a_late_collection_gets_no_key_and_the_membership_it_made_still_signs_in() {
    let _clock = TEST_CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("late.db");
    let (hub, store, id, secret) = approved(&path, "latecomer");

    // The requester collected its key in time; its ACK never arrived.
    at(APPROVED_AT + 60_000);
    let BootstrapClaim::Delivered(key) = collect(&hub, &id, &secret) else {
        panic!("collected inside the window");
    };

    // At the deadline the copy is no longer handed out, and the answer says
    // why. Asking cleared it.
    at(APPROVED_AT + JOIN_BOOTSTRAP_WINDOW_MS);
    assert_eq!(collect(&hub, &id, &secret), BootstrapClaim::Expired);
    assert_eq!(store.join_request_mb_token_raw(&id), Some(None));
    assert_eq!(collect(&hub, &id, &secret), BootstrapClaim::Expired);
    assert_eq!(shown(&hub, &id, &secret), ("approved".into(), false, true));

    // The membership was not revoked: the key collected earlier signs in,
    // now and after a restart.
    assert_eq!(
        hub.member_for_credential(Some(&key))
            .expect("still a member")
            .name,
        "latecomer"
    );
    drop(hub);
    drop(store);
    let store = Arc::new(
        Store::open_at(path.to_str(), APPROVED_AT + JOIN_BOOTSTRAP_WINDOW_MS + 1).unwrap(),
    );
    let hub = hub_on(store);
    assert_eq!(
        hub.member_for_credential(Some(&key))
            .expect("still a member after a restart")
            .name,
        "latecomer"
    );
    assert_eq!(collect(&hub, &id, &secret), BootstrapClaim::Expired);
}

#[test]
fn a_late_ack_succeeds_and_neither_reopens_the_window_nor_shows_expired_again() {
    let _clock = TEST_CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let directory = tempfile::tempdir().unwrap();
    let (hub, store, id, secret) = approved(&directory.path().join("lateack.db"), "slowack");
    at(APPROVED_AT + 1_000);
    assert!(matches!(
        collect(&hub, &id, &secret),
        BootstrapClaim::Delivered(_)
    ));
    at(APPROVED_AT + JOIN_BOOTSTRAP_WINDOW_MS + 5_000);
    hub.expire_join_request_bootstraps();
    assert_eq!(store.join_request_mb_token_raw(&id), Some(None));

    // The ACK that was lost arrives late: it is a success, not a failure the
    // requester would meet on every retry...
    assert_eq!(
        hub.ack_join_request_bootstrap(&id, &secret).unwrap(),
        Some(true)
    );
    assert_eq!(
        hub.ack_join_request_bootstrap(&id, &secret).unwrap(),
        Some(false)
    );
    // ...and it restores nothing: no copy, no collection, no readiness.
    assert_eq!(store.join_request_mb_token_raw(&id), Some(None));
    assert_eq!(collect(&hub, &id, &secret), BootstrapClaim::Unavailable);
    // Once acknowledged, the request ended normally; it never reads as
    // expired again, whatever the clock does.
    for later in [1, JOIN_BOOTSTRAP_WINDOW_MS, 10 * JOIN_BOOTSTRAP_WINDOW_MS] {
        at(APPROVED_AT + JOIN_BOOTSTRAP_WINDOW_MS + later);
        hub.expire_join_request_bootstraps();
        assert_eq!(shown(&hub, &id, &secret), ("approved".into(), false, false));
    }
}

#[test]
fn an_ack_inside_the_window_never_reads_as_expired_later() {
    let _clock = TEST_CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ontime.db");
    let (hub, _store, id, secret) = approved(&path, "ontime");
    at(APPROVED_AT + 1_000);
    assert!(matches!(
        collect(&hub, &id, &secret),
        BootstrapClaim::Delivered(_)
    ));
    assert_eq!(
        hub.ack_join_request_bootstrap(&id, &secret).unwrap(),
        Some(true)
    );
    at(APPROVED_AT + 2 * JOIN_BOOTSTRAP_WINDOW_MS);
    hub.expire_join_request_bootstraps();
    assert_eq!(shown(&hub, &id, &secret), ("approved".into(), false, false));
    assert_eq!(collect(&hub, &id, &secret), BootstrapClaim::Unavailable);
    let expired_at: Option<u64> = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT bootstrap_expired_at FROM join_requests WHERE id = ?1",
            [&id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(expired_at, None);
}

#[test]
fn the_sweep_clears_an_expired_copy_without_any_request() {
    let _clock = TEST_CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let directory = tempfile::tempdir().unwrap();
    let (hub, store, id, _secret) = approved(&directory.path().join("sweep.db"), "absent");

    // Nobody ever asks. One second before the deadline the sweep leaves it...
    at(APPROVED_AT + JOIN_BOOTSTRAP_WINDOW_MS - 1_000);
    hub.expire_join_request_bootstraps();
    assert!(matches!(
        store.join_request_mb_token_raw(&id),
        Some(Some(_))
    ));
    // ...and at the deadline it clears it.
    at(APPROVED_AT + JOIN_BOOTSTRAP_WINDOW_MS);
    hub.expire_join_request_bootstraps();
    assert_eq!(store.join_request_mb_token_raw(&id), Some(None));
    assert!(hub.member_by_name("absent").is_some());
}

#[test]
fn inside_the_window_everything_is_as_before() {
    let _clock = TEST_CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let directory = tempfile::tempdir().unwrap();
    let (hub, store, id, secret) = approved(&directory.path().join("inside.db"), "punctual");

    assert_eq!(shown(&hub, &id, &secret), ("approved".into(), true, false));
    at(APPROVED_AT + 1_000);
    let first = collect(&hub, &id, &secret);
    at(APPROVED_AT + 2_000);
    assert_eq!(
        collect(&hub, &id, &secret),
        first,
        "a repeated collection gets the same key"
    );
    // One second before the deadline the key is still handed out.
    at(APPROVED_AT + JOIN_BOOTSTRAP_WINDOW_MS - 1_000);
    hub.expire_join_request_bootstraps();
    assert_eq!(collect(&hub, &id, &secret), first);
    assert_eq!(shown(&hub, &id, &secret), ("approved".into(), true, false));
    assert!(matches!(first, BootstrapClaim::Delivered(_)));
    // ACK, and ACK again.
    assert_eq!(
        hub.ack_join_request_bootstrap(&id, &secret).unwrap(),
        Some(true)
    );
    assert_eq!(
        hub.ack_join_request_bootstrap(&id, &secret).unwrap(),
        Some(false)
    );
    assert_eq!(collect(&hub, &id, &secret), BootstrapClaim::Unavailable);
    assert_eq!(store.join_request_mb_token_raw(&id), Some(None));
    // A wrong secret learns nothing, as before.
    assert!(hub.join_request_view(&id, "wrong").unwrap().is_none());
    assert_eq!(collect(&hub, &id, "wrong"), BootstrapClaim::Unavailable);
    assert_eq!(hub.ack_join_request_bootstrap(&id, "wrong").unwrap(), None);

    // A request that is never approved is untouched by any amount of time.
    let JoinRequestCreate::Created {
        request_id: waiting,
        request_secret: waiting_secret,
    } = hub.create_join_request("patient", "agent", "10.0.0.8".parse().unwrap())
    else {
        panic!("created");
    };
    at(APPROVED_AT + 10 * JOIN_BOOTSTRAP_WINDOW_MS);
    hub.expire_join_request_bootstraps();
    assert_eq!(
        shown(&hub, &waiting, &waiting_secret),
        ("pending".into(), false, false)
    );
    assert_eq!(
        collect(&hub, &waiting, &waiting_secret),
        BootstrapClaim::Unavailable
    );
    assert_eq!(hub.list_pending_join_requests().len(), 1);
}
