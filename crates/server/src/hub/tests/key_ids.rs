//! What the hub names a membership or an invite by when it uses one as an
//! identifier rather than checking it as a credential: the seat key (which
//! travels in `Evicted`), a session's membership, the `op_id` scope and
//! Lobby presence carry the management ids (`mbid_`, `ivid_`), never the
//! membership key or the invite.

use std::sync::Arc;

use protocol::SenderType;

use super::super::Hub;
use super::{admit_with_key, critical_restart_hub, revoke_invite, session_request};
use crate::store::{InviteRecord, MemberRecord, Store};

/// An issue time shaped like a real one (milliseconds since the epoch).
const ISSUED_AT: u64 = 1_800_000_000_000;

fn invite(hub: &Hub, room: &str, member: &MemberRecord, name: &str) -> String {
    let token = hub.new_invite_token();
    hub.add_invite(
        InviteRecord::issued(
            &token,
            room,
            &member.id,
            name,
            &member.kind,
            ISSUED_AT,
            "master",
        ),
        &token,
    )
    .unwrap();
    token
}

fn invite_session(hub: &Hub, invite_key: &str, name: &str) -> String {
    hub.create_session_scoped(
        session_request(name),
        Some(&hub.invite_by_token(invite_key).expect("a live invite")),
        None,
        false,
        None,
    )
    .unwrap()
    .expect("session recorded")
    .session_token
}

/// A memory-only Building: no principal binds a membership, so the
/// membership itself is what the seat, the session and the operation are
/// keyed by. Each of them names it `mb:<mbid_>`; an unbound legacy invite is
/// `dv:<ivid_>`. The sessions still end with their invite and membership.
#[test]
fn seats_sessions_and_operations_name_a_membership_by_its_id() {
    let hub = critical_restart_hub(Arc::new(Store::open(None).unwrap()), "ids-root");
    let (member, key) = admit_with_key(&hub, "worker", "agent", "master");
    let member_id = member.id.clone();
    assert!(member_id.starts_with("mbid_"), "{member_id}");
    let invite_key = invite(&hub, "ops", &member, "worker");
    let by_invite = invite_session(&hub, &invite_key, "worker");
    let own = hub
        .create_session_scoped(
            session_request("worker"),
            None,
            None,
            false,
            hub.member_for_credential(Some(&key)),
        )
        .unwrap()
        .expect("session recorded")
        .session_token;
    let unbound = hub.new_invite_token();
    hub.add_invite(
        InviteRecord::issued(&unbound, "ops", "", "legacy", "agent", ISSUED_AT, "master"),
        &unbound,
    )
    .unwrap();

    let invite_idy = hub.session_identity(Some(&by_invite)).expect("live");
    let own_idy = hub.session_identity(Some(&own)).expect("live");
    assert_eq!(invite_idy.member.as_deref(), Some(member_id.as_str()));
    assert_eq!(own_idy.member.as_deref(), Some(member_id.as_str()));

    let seat = format!("mb:{member_id}");
    for (how, got) in [
        (
            "invite",
            hub.seat_identity(None, Some(&invite_key), None, None, "a"),
        ),
        (
            "membership key",
            hub.seat_identity(None, Some(&key), None, None, "b"),
        ),
        (
            "invite session",
            hub.seat_identity(None, None, Some(&invite_idy), Some(&by_invite), "c"),
        ),
        (
            "own-key session",
            hub.seat_identity(None, None, Some(&own_idy), Some(&own), "d"),
        ),
    ] {
        assert_eq!(got, seat, "{how}");
    }
    assert_eq!(
        hub.seat_identity(None, Some(&unbound), None, None, "legacy"),
        format!("dv:{}", hub.invite_management_id(&unbound))
    );

    let scope = |invite_key: Option<&str>, session: Option<&super::super::SessionIdentity>| {
        hub.operation_scope(
            "ops",
            None,
            invite_key,
            session,
            SenderType::Agent,
            "worker",
        )
    };
    for (how, got) in [
        ("invite", scope(Some(&invite_key), None)),
        ("membership key", scope(Some(&key), None)),
        ("invite session", scope(None, Some(&invite_idy))),
        ("own-key session", scope(None, Some(&own_idy))),
    ] {
        assert_eq!(got.key, seat, "{how}");
        assert_eq!(got.principal, None, "{how}");
    }
    // The REST route's earlier key for a session's membership, read only.
    assert_eq!(scope(None, Some(&invite_idy)).legacy, None);
    assert_eq!(
        scope(Some(&invite_key), None).legacy.as_deref(),
        Some("agent:worker")
    );

    // Nothing above carries the key or the invite.
    let carried = [
        hub.seat_identity(None, Some(&invite_key), None, None, "a"),
        hub.seat_identity(None, Some(&unbound), None, None, "legacy"),
        scope(Some(&invite_key), None).key,
        invite_idy.member.clone().unwrap_or_default(),
    ];
    for value in &carried {
        for secret in [&key, &invite_key, &unbound] {
            assert!(!value.contains(secret.as_str()), "{value} carries a key");
        }
        assert!(!value.contains("mb_") && !value.contains("dv_"), "{value}");
    }

    // A session found by its membership id: release-self's lookup.
    let held = hub.invites_for_member(&member_id);
    assert_eq!(
        held.iter()
            .map(|i| i.secret_hash.as_str())
            .collect::<Vec<_>>(),
        [crate::store::secret_hash(&invite_key).as_str()]
    );

    // The sweeps compare by id too: the invite ends its session, the
    // membership ends the rest.
    assert!(revoke_invite(&hub, &invite_key));
    assert!(hub.session_identity(Some(&by_invite)).is_none());
    assert!(hub.session_identity(Some(&own)).is_some());
    assert!(hub.revoke_member(&member.id).unwrap());
    assert!(hub.session_identity(Some(&own)).is_none());
}

/// A persistent Building: an invite session names its membership by id, and
/// the REST route's earlier key for it (`legacy`) is `mb:<mbid_>`, the form
/// `Store::open` rewrites stored keys to. An invite session taken from a
/// store's own invite still ends with it.
#[test]
fn an_invite_session_in_a_persistent_building_names_its_membership_by_id() {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(directory.path().join("ids.db").to_str()).unwrap());
    let hub = critical_restart_hub(store, "ids-root");
    let member = hub.admit_member("ada", "user", "master").unwrap();
    let member_id = member.id.clone();
    let invite_key = invite(&hub, "ops", &member, "ada");
    let by_invite = invite_session(&hub, &invite_key, "ada");
    let idy = hub.session_identity(Some(&by_invite)).expect("live");
    assert_eq!(idy.member.as_deref(), Some(member_id.as_str()));
    let principal = hub.member_principal(&member.id).expect("bound");
    let scope = hub.operation_scope("ops", None, None, Some(&idy), SenderType::User, "ada");
    assert_eq!(scope.key, format!("pr:{principal}"));
    assert_eq!(scope.legacy, Some(format!("mb:{member_id}")));
    assert!(revoke_invite(&hub, &invite_key));
    assert!(hub.session_identity(Some(&by_invite)).is_none());
}

/// Lobby presence is counted by the membership's id, and the residents
/// view reads it by the same id.
#[test]
fn lobby_presence_is_counted_by_the_membership_id() {
    let hub = critical_restart_hub(Arc::new(Store::open(None).unwrap()), "ids-root");
    let member = hub.admit_member("bea", "user", "master").unwrap();
    let member_id = member.id.clone();
    let online = |hub: &Hub| {
        hub.residents()
            .into_iter()
            .find(|r| r.name == "bea")
            .expect("a resident")
            .online
    };
    assert!(!online(&hub));
    hub.lobby_join(&member_id);
    hub.lobby_join(&member_id);
    assert!(online(&hub));
    hub.lobby_leave(&member_id);
    assert!(online(&hub), "one socket is still open");
    hub.lobby_leave(&member_id);
    assert!(!online(&hub));
}
