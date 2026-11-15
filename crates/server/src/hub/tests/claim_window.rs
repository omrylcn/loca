//! Claiming a membership key with an invite, through the hub: one key per
//! set-up, the delivery copy's deadline (a late request, the sweep, the
//! open), the cap, door A, and the same rules in a memory-only Building.

use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use protocol::RoomSettings;

use super::super::{ClaimRefusal, Hub, HubConfig};
use super::{test_now_ms, TEST_CLOCK_LOCK, TEST_NOW};
use crate::store::{secret_hash, MemberClaim, Store, CLAIM_DELIVERY_WINDOW_MS};

/// A realistic claim time, in milliseconds since the epoch.
const CLAIMED_AT: u64 = 1_800_000_000_000;

/// Claim ids shaped like the kit's (`cl_` and 43 URL-safe characters).
const SETUP_A: &str = "cl_SetupAaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SETUP_B: &str = "cl_SetupBbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const SETUP_C: &str = "cl_SetupCccccccccccccccccccccccccccccccccccccc";

fn at(time: u64) {
    TEST_NOW.store(time, Ordering::Relaxed);
}

fn hub_on(store: Arc<Store>) -> Hub {
    let mut hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: true,
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

/// A hub with member `worker` seated in `parlor`; returns it with the member's
/// original key and its invite.
fn seated(store: Arc<Store>) -> (Hub, String, String) {
    let hub = hub_on(store);
    at(CLAIMED_AT - 60_000);
    hub.create_loca("parlor", &hub.resolve_authority(Some("MASTER"), None), None)
        .expect("explicit fixture loca");
    let (_, original) = super::admit_with_key(&hub, "worker", "agent", "master");
    let member = hub.member_by_name("worker").expect("admitted");
    let (_, invite) = hub
        .invite_member_to_room(&member.id, "parlor", "master")
        .unwrap();
    (hub, original, invite)
}

fn claim(hub: &Hub, invite: &str, setup: Option<&str>) -> MemberClaim {
    hub.claim_membership_key(invite, setup)
        .expect("a live invite")
        .0
}

fn key_of(outcome: MemberClaim) -> (String, bool) {
    match outcome {
        MemberClaim::Delivered { key, fresh, .. } => (key, fresh),
        other => panic!("expected a key, got {other:?}"),
    }
}

fn invite_id(hub: &Hub, invite: &str) -> String {
    hub.invite_management_id(invite)
}

#[test]
fn a_late_repeat_is_told_the_delivery_ended_and_mints_nothing() {
    let _clock = TEST_CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("late-claim.db");
    let store = Arc::new(Store::open_at(path.to_str(), CLAIMED_AT).unwrap());
    let (hub, original, invite) = seated(store.clone());
    let id = invite_id(&hub, &invite);

    at(CLAIMED_AT);
    let (key, fresh) = key_of(claim(&hub, &invite, Some(SETUP_A)));
    assert!(fresh && key.starts_with("mb_") && key != original);
    at(CLAIMED_AT + 60_000);
    assert_eq!(
        key_of(claim(&hub, &invite, Some(SETUP_A))),
        (key.clone(), false),
        "inside the window a repeat gets the same key"
    );

    // At the deadline the same set-up is told the delivery ended; asking
    // cleared the copy, and nothing was minted or counted.
    at(CLAIMED_AT + CLAIM_DELIVERY_WINDOW_MS);
    assert_eq!(
        claim(&hub, &invite, Some(SETUP_A)),
        MemberClaim::DeliveryEnded
    );
    assert_eq!(
        store.member_claim_copy_raw(&id, &secret_hash(SETUP_A)),
        Some(None)
    );
    assert_eq!(
        claim(&hub, &invite, Some(SETUP_A)),
        MemberClaim::DeliveryEnded
    );
    assert_eq!(store.member_claim_count(&id), 1);
    // Only the copy ended: the credential it minted still signs in.
    assert_eq!(
        hub.member_for_credential(Some(&key)).unwrap().name,
        "worker"
    );

    // A lost ACK that arrives late is a success, and from then on the
    // set-up hears "already delivered", across a restart too.
    assert_eq!(
        hub.ack_membership_claim(&invite, SETUP_A).unwrap(),
        Some(true)
    );
    assert_eq!(
        hub.ack_membership_claim(&invite, SETUP_A).unwrap(),
        Some(false)
    );
    assert_eq!(
        claim(&hub, &invite, Some(SETUP_A)),
        MemberClaim::AlreadyDelivered
    );
    drop(hub);
    drop(store);
    let store =
        Arc::new(Store::open_at(path.to_str(), CLAIMED_AT + 2 * CLAIM_DELIVERY_WINDOW_MS).unwrap());
    let hub = hub_on(store.clone());
    assert_eq!(
        claim(&hub, &invite, Some(SETUP_A)),
        MemberClaim::AlreadyDelivered
    );
    assert_eq!(store.member_claim_count(&id), 1);
    assert_eq!(
        hub.member_for_credential(Some(&key)).unwrap().name,
        "worker"
    );
}

#[test]
fn the_sweep_and_the_open_clear_an_uncollected_copy() {
    let _clock = TEST_CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sweep-claim.db");
    let store = Arc::new(Store::open_at(path.to_str(), CLAIMED_AT).unwrap());
    let (hub, _original, invite) = seated(store.clone());
    let id = invite_id(&hub, &invite);
    at(CLAIMED_AT);
    key_of(claim(&hub, &invite, Some(SETUP_A)));
    at(CLAIMED_AT + 5_000);
    key_of(claim(&hub, &invite, Some(SETUP_B)));

    // Nobody asks again. A second before A's deadline the sweep keeps it...
    at(CLAIMED_AT + CLAIM_DELIVERY_WINDOW_MS - 1_000);
    hub.expire_join_request_bootstraps();
    assert!(matches!(
        store.member_claim_copy_raw(&id, &secret_hash(SETUP_A)),
        Some(Some(_))
    ));
    // ...at the deadline it clears A, and B (claimed later) stays.
    at(CLAIMED_AT + CLAIM_DELIVERY_WINDOW_MS);
    hub.expire_join_request_bootstraps();
    assert_eq!(
        store.member_claim_copy_raw(&id, &secret_hash(SETUP_A)),
        Some(None)
    );
    assert!(matches!(
        store.member_claim_copy_raw(&id, &secret_hash(SETUP_B)),
        Some(Some(_))
    ));

    // The server stops across B's deadline: the open clears it before
    // anything is asked.
    drop(hub);
    drop(store);
    let store = Arc::new(
        Store::open_at(path.to_str(), CLAIMED_AT + CLAIM_DELIVERY_WINDOW_MS + 5_000).unwrap(),
    );
    assert_eq!(
        store.member_claim_copy_raw(&id, &secret_hash(SETUP_B)),
        Some(None)
    );
    let hub = hub_on(store.clone());
    at(CLAIMED_AT + CLAIM_DELIVERY_WINDOW_MS + 6_000);
    assert_eq!(
        claim(&hub, &invite, Some(SETUP_B)),
        MemberClaim::DeliveryEnded
    );
    assert_eq!(store.member_claim_count(&id), 2);
}

#[test]
fn door_a_revokes_every_credential_of_the_member_including_claimed_ones() {
    let _clock = TEST_CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("door-a.db");
    let store = Arc::new(Store::open_at(path.to_str(), CLAIMED_AT).unwrap());
    let (hub, original, invite) = seated(store.clone());
    at(CLAIMED_AT);
    let (claimed, _) = key_of(claim(&hub, &invite, Some(SETUP_A)));
    let member_id = hub.member_by_name("worker").expect("member").id;
    let principal = hub.member_principal(&member_id).unwrap();
    hub.create_profile_credential(&principal, "Worker laptop")
        .unwrap();
    assert_eq!(hub.credentials_for(&principal).len(), 3);

    assert!(hub.revoke_member_ref(&member_id).unwrap());
    for credential in hub.credentials_for(&principal) {
        assert!(
            credential.revoked_at.is_some(),
            "{} is still live after the membership was revoked",
            credential.label
        );
    }
    assert!(hub.member_for_credential(Some(&claimed)).is_none());
    assert!(hub.member_for_credential(Some(&original)).is_none());
}

#[test]
fn a_memory_only_building_keeps_the_same_rules() {
    let _clock = TEST_CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (hub, original, invite) = seated(Arc::new(Store::open(None).unwrap()));
    at(CLAIMED_AT);
    let (a, fresh) = key_of(claim(&hub, &invite, Some(SETUP_A)));
    assert!(fresh && a != original);
    assert_eq!(
        key_of(claim(&hub, &invite, Some(SETUP_A))),
        (a.clone(), false)
    );
    assert_eq!(hub.member_for_credential(Some(&a)).unwrap().name, "worker");
    let (b, _) = key_of(claim(&hub, &invite, Some(SETUP_B)));
    assert_ne!(a, b, "another set-up gets a key of its own");

    assert_eq!(
        hub.ack_membership_claim(&invite, SETUP_A).unwrap(),
        Some(true)
    );
    assert_eq!(
        hub.ack_membership_claim(&invite, SETUP_A).unwrap(),
        Some(false)
    );
    assert_eq!(hub.ack_membership_claim(&invite, SETUP_C).unwrap(), None);
    assert_eq!(
        claim(&hub, &invite, Some(SETUP_A)),
        MemberClaim::AlreadyDelivered
    );
    at(CLAIMED_AT + CLAIM_DELIVERY_WINDOW_MS);
    assert_eq!(
        claim(&hub, &invite, Some(SETUP_B)),
        MemberClaim::DeliveryEnded
    );
    // Calls without a claim id are set-ups of their own: three more fill
    // the invite's five, and the sixth set-up mints nothing.
    for _ in 0..3 {
        key_of(claim(&hub, &invite, None));
    }
    assert_eq!(claim(&hub, &invite, None), MemberClaim::CapReached);
    assert_eq!(claim(&hub, &invite, Some(SETUP_C)), MemberClaim::CapReached);
    // A membership key is not an invite.
    assert_eq!(
        hub.claim_membership_key(&a, Some(SETUP_C)).unwrap_err(),
        ClaimRefusal::MembershipKey
    );
    // Door A ends the claimed keys with the membership.
    let member_id = hub.member_by_name("worker").expect("member").id;
    assert!(hub.revoke_member_ref(&member_id).unwrap());
    assert!(hub.member_for_credential(Some(&a)).is_none());
    assert!(hub.member_for_credential(Some(&b)).is_none());
}
