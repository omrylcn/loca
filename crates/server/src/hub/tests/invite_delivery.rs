//! An invite's delivery through the hub: one stable job, resumed by every
//! Lobby connect and never restarted by one. Whatever the order of Lobby
//! connections, lost frames, late loca sockets and restarts, the member ends
//! with the same single working invite per seat, and the copy ends at first
//! use or 24 hours after issue.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use protocol::{RoomSettings, SenderType};

use super::super::{Hub, HubConfig, LobbyEvent, RequestAuthority};
use crate::store::{secret_hash, Store, INVITE_DELIVERY_WINDOW_MS};

const ROOT: &str = "delivery-fixture-root";
/// An issue time shaped like a real one (milliseconds since the epoch).
const ISSUED_AT: u64 = 1_800_000_000_000;

static NOW: AtomicU64 = AtomicU64::new(ISSUED_AT);
/// The clock is one static: the tests of this module take turns with it.
static CLOCK: Mutex<()> = Mutex::new(());

fn clock() -> u64 {
    NOW.load(Ordering::Relaxed)
}

fn set_now(at: u64) {
    NOW.store(at, Ordering::Relaxed);
}

fn hub_on(store: Arc<Store>) -> Hub {
    let mut hub = Hub::build(
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
    );
    hub.now_ms = clock;
    hub
}

fn master(hub: &Hub) -> RequestAuthority {
    hub.resolve_authority(Some(ROOT), None)
}

fn session_with(hub: &Hub, token: &str) {
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
    .expect("invite live");
}

/// What a Lobby connect sends after `lobby_ready`: (loca, invite).
fn connect(hub: &Hub, member: &str) -> Vec<(String, String)> {
    let mut sent: Vec<(String, String)> = hub
        .pending_deliveries(member)
        .into_iter()
        .map(|(invite, invite_key)| (invite.room, invite_key))
        .collect();
    sent.sort();
    sent
}

/// The member's live invites: (loca, the invite's hash, which is all the hub
/// keeps of it).
fn held(hub: &Hub, member: &str) -> Vec<(String, String)> {
    let mut held: Vec<(String, String)> = hub
        .invites_for_member(member)
        .into_iter()
        .map(|invite| (invite.room, invite.secret_hash))
        .collect();
    held.sort();
    held
}

/// `seat` as `held` shows it: the invite by its hash.
fn hashed(seat: &[(String, String)]) -> Vec<(String, String)> {
    seat.iter()
        .map(|(room, invite)| (room.clone(), secret_hash(invite)))
        .collect()
}

/// The invite's delivery copy and first use, as stored.
fn copy(store: &Store, invite: &str) -> (Option<String>, Option<u64>) {
    store
        .invite_delivery_raw_by_hash(&secret_hash(invite))
        .expect("row")
}

/// A loca socket taken with `invite`: its first use.
fn use_invite(hub: &Hub, invite: &str) {
    hub.note_invite_used_by_hash(&secret_hash(invite));
}

#[test]
fn lobby_connections_lost_frames_and_a_late_socket_end_with_one_working_invite() {
    let _clock = CLOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_now(ISSUED_AT);
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open_at(dir.path().join("order.db").to_str(), ISSUED_AT).unwrap());
    let hub = hub_on(store.clone());
    hub.create_loca("parlor", &master(&hub), None)
        .expect("loca");
    let member = hub.admit_member("worker", "agent", "master").unwrap();
    // Called while no Lobby socket is open.
    let (_, invite) = hub
        .invite_member_to_room(&member.id, "parlor", "master")
        .unwrap();
    let seat = vec![("parlor".to_string(), invite.clone())];

    // Listener A connects; listener B connects before A has opened the loca
    // socket; A's frame is lost with its socket and A reconnects. Every one
    // is sent the same invite, and none of them changes what is held.
    for _ in ["A", "B", "A again"] {
        set_now(clock() + 1_000);
        assert_eq!(connect(&hub, &member.id), seat);
        assert_eq!(held(&hub, &member.id), hashed(&seat));
        assert!(hub.invite_for("parlor", Some(&invite)).is_some());
    }
    assert_eq!(copy(&store, &invite).0, Some(invite.clone()));

    // The loca socket comes late, after the second connection: its use ends
    // the delivery, and the invite the listeners hold keeps working.
    set_now(clock() + 1_000);
    let used_at = clock();
    use_invite(&hub, &invite);
    assert_eq!(copy(&store, &invite), (None, Some(used_at)));
    assert!(connect(&hub, &member.id).is_empty());
    assert_eq!(held(&hub, &member.id), hashed(&seat));
    assert!(hub.invite_for("parlor", Some(&invite)).is_some());
    // A second socket with it is no second first use.
    set_now(clock() + 1_000);
    use_invite(&hub, &invite);
    assert_eq!(copy(&store, &invite).1, Some(used_at));

    // A session taken with an invite is a use too.
    hub.create_loca("runnerup", &master(&hub), None)
        .expect("loca");
    let (_, second) = hub
        .invite_member_to_room(&member.id, "runnerup", "master")
        .unwrap();
    assert_eq!(
        connect(&hub, &member.id),
        vec![("runnerup".to_string(), second.clone())]
    );
    session_with(&hub, &second);
    assert!(connect(&hub, &member.id).is_empty());
    assert_eq!(copy(&store, &second).0, None);
}

/// A Lobby pass sends one frame, waits on the socket, then decides the next.
/// Whatever happens to the next invite while the first frame is waiting, the
/// pass never sends an invite that is no longer owed: not a revoked one, not
/// one a re-issue replaced (the fresh one goes instead), not one whose first
/// use has ended its delivery.
#[test]
fn an_invite_that_ends_while_an_earlier_frame_waits_is_not_sent() {
    let _clock = CLOCK.lock().unwrap_or_else(|e| e.into_inner());
    for change in ["revoke", "reissue", "first use"] {
        set_now(ISSUED_AT);
        let dir = tempfile::tempdir().unwrap();
        let store =
            Arc::new(Store::open_at(dir.path().join("pass.db").to_str(), ISSUED_AT).unwrap());
        let hub = hub_on(store);
        hub.create_loca("north", &master(&hub), None).expect("loca");
        hub.create_loca("south", &master(&hub), None).expect("loca");
        let member = hub.admit_member("worker", "agent", "master").unwrap();
        for room in ["north", "south"] {
            hub.invite_member_to_room(&member.id, room, "master")
                .unwrap();
        }
        assert_eq!(connect(&hub, &member.id).len(), 2, "two are owed");

        // The pass sends its first frame and waits on the socket.
        let mut sent = HashSet::new();
        let (first, first_invite) = hub
            .next_owed_delivery(&member.id, &sent)
            .expect("the first owed invite");
        sent.insert(first.secret_hash.clone());
        let (_, waiting) = connect(&hub, &member.id)
            .into_iter()
            .find(|(_, token)| *token != first_invite)
            .expect("the invite that is next");
        let room = if first.room == "north" {
            "south"
        } else {
            "north"
        };

        // Meanwhile the next invite ends, or its delivery does.
        set_now(clock() + 1_000);
        let replaced_by = match change {
            "revoke" => {
                assert!(super::revoke_invite(&hub, &waiting));
                None
            }
            "reissue" => Some(
                hub.reissue_invite(&member.id, room, &master(&hub))
                    .expect("re-issue")
                    .1,
            ),
            _ => {
                use_invite(&hub, &waiting);
                None
            }
        };

        // The pass goes on. It is never handed the invite it would have sent.
        let mut rest = Vec::new();
        while let Some((next, invite)) = hub.next_owed_delivery(&member.id, &sent) {
            sent.insert(next.secret_hash.clone());
            rest.push(invite);
        }
        assert!(
            !rest.contains(&waiting),
            "{change}: the pass sent an invite that was no longer owed"
        );
        assert_eq!(
            rest,
            replaced_by.into_iter().collect::<Vec<_>>(),
            "{change}: only an invite that replaced it may follow"
        );
    }
}

/// The start binds an invite of the old model (no membership on its row) to a
/// membership. That is not a first use: a young invite the upgrade gave a
/// delivery copy to is still delivered after the next restart.
#[test]
fn binding_an_old_invite_to_its_membership_does_not_end_its_delivery() {
    let _clock = CLOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_now(ISSUED_AT);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.db");
    let invite = "dv_legacy_unbound_young";
    drop(Store::open_at(path.to_str(), ISSUED_AT).unwrap());
    // The tables as v0.13.0 left them: the invite keyed by itself, with no
    // membership on its row and no delivery columns.
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(crate::store::LEGACY_MEMBER_TABLES)
        .unwrap();
    conn.execute(
        "INSERT INTO invites (token, room, name, kind, issued_at, issued_by, revoked_at, member)
         VALUES (?1, 'parlor', 'worker', 'agent', ?2, 'master', NULL, '')",
        rusqlite::params![invite, ISSUED_AT],
    )
    .unwrap();
    drop(conn);

    // The upgrade and the first start, an hour after the invite was issued.
    set_now(ISSUED_AT + 3_600_000);
    let member = {
        let store = Arc::new(Store::open_at(path.to_str(), clock()).unwrap());
        let hub = hub_on(store.clone());
        let member = hub.member_by_name("worker").expect("bound to a membership");
        assert_eq!(
            connect(&hub, &member.id),
            vec![("parlor".to_string(), invite.to_string())]
        );
        assert_eq!(
            copy(&store, invite),
            (Some(invite.to_string()), None),
            "the binding ended a delivery nobody used"
        );
        member.id
    };

    // A restart before anyone used it: the same invite is still owed.
    set_now(clock() + 60_000);
    let hub = hub_on(Arc::new(Store::open_at(path.to_str(), clock()).unwrap()));
    assert_eq!(
        connect(&hub, &member),
        vec![("parlor".to_string(), invite.to_string())],
        "the delivery was lost at the restart, before any use and inside its window"
    );
}

#[test]
fn a_restart_resumes_an_owed_delivery_and_does_not_restart_an_ended_one() {
    let _clock = CLOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_now(ISSUED_AT);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("restart.db");
    let (member, owed, used) = {
        let hub = hub_on(Arc::new(Store::open_at(path.to_str(), ISSUED_AT).unwrap()));
        hub.create_loca("owed", &master(&hub), None).expect("loca");
        hub.create_loca("used", &master(&hub), None).expect("loca");
        let member = hub.admit_member("worker", "agent", "master").unwrap();
        let (_, owed) = hub
            .invite_member_to_room(&member.id, "owed", "master")
            .unwrap();
        let (_, used) = hub
            .invite_member_to_room(&member.id, "used", "master")
            .unwrap();
        use_invite(&hub, &used);
        (member.id, owed, used)
    };
    // The server stops in the middle and comes back an hour later.
    set_now(ISSUED_AT + 3_600_000);
    let hub = hub_on(Arc::new(Store::open_at(path.to_str(), clock()).unwrap()));
    assert_eq!(
        connect(&hub, &member),
        vec![("owed".to_string(), owed.clone())]
    );
    assert_eq!(
        held(&hub, &member),
        hashed(&[
            ("owed".to_string(), owed.clone()),
            ("used".to_string(), used.clone())
        ]),
        "both invites survive the restart, and nothing was minted"
    );
}

#[test]
fn the_deadline_counts_from_issue_through_restarts_and_ends_only_the_delivery() {
    let _clock = CLOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_now(ISSUED_AT);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deadline.db");
    let deadline = ISSUED_AT + INVITE_DELIVERY_WINDOW_MS;
    let (member, invite_key) = {
        let hub = hub_on(Arc::new(Store::open_at(path.to_str(), ISSUED_AT).unwrap()));
        hub.create_loca("parlor", &master(&hub), None)
            .expect("loca");
        hub.create_loca("swept", &master(&hub), None).expect("loca");
        let member = hub.admit_member("worker", "agent", "master").unwrap();
        let (_, invite_key) = hub
            .invite_member_to_room(&member.id, "parlor", "master")
            .unwrap();
        hub.invite_member_to_room(&member.id, "swept", "master")
            .unwrap();
        (member.id, invite_key)
    };
    // A restart a millisecond before the deadline still delivers it.
    set_now(deadline - 1);
    let store = Arc::new(Store::open_at(path.to_str(), clock()).unwrap());
    let hub = hub_on(store.clone());
    assert_eq!(connect(&hub, &member).len(), 2);
    // At the deadline, counted from issue (the restart reset nothing), a
    // connect sends neither, and clears the copy it finds.
    set_now(deadline);
    assert_eq!(
        connect(&hub, &member)
            .into_iter()
            .filter(|(room, _)| room == "parlor")
            .count(),
        0
    );
    assert_eq!(copy(&store, &invite_key).0, None);
    // The sweep clears the other at the same moment, and the invites live on.
    hub.expire_invite_deliveries();
    assert!(connect(&hub, &member).is_empty());
    assert!(hub.invites_for_member(&member).iter().all(|invite| store
        .invite_delivery_raw_by_hash(&invite.secret_hash)
        .unwrap()
        .0
        .is_none()));
    assert_eq!(held(&hub, &member).len(), 2);
    assert!(hub.invite_for("parlor", Some(&invite_key)).is_some());
}

#[test]
fn every_invite_minted_for_delivery_keeps_a_copy() {
    let _clock = CLOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_now(ISSUED_AT);
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open_at(dir.path().join("mint.db").to_str(), ISSUED_AT).unwrap());
    let hub = hub_on(store.clone());
    hub.create_loca("parlor", &master(&hub), None)
        .expect("loca");
    let member = hub.admit_member("worker", "agent", "master").unwrap();
    let (_, first) = hub
        .invite_member_to_room(&member.id, "parlor", "master")
        .unwrap();
    use_invite(&hub, &first);
    // A re-issue is a fresh delivery.
    let (_, fresh) = hub
        .reissue_invite(&member.id, "parlor", &master(&hub))
        .unwrap();
    assert_eq!(
        connect(&hub, &member.id),
        vec![("parlor".to_string(), fresh.clone())]
    );
    assert_eq!(copy(&store, &fresh).0, Some(fresh.clone()));
}

#[test]
fn an_invite_carried_over_by_a_group_move_keeps_a_copy() {
    let _clock = CLOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_now(ISSUED_AT);
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open_at(dir.path().join("move.db").to_str(), ISSUED_AT).unwrap());
    let hub = hub_on(store.clone());
    let master = master(&hub);
    hub.create_loca("mover", &master, None).expect("loca");
    let member = hub.admit_member("worker", "agent", "master").unwrap();
    let principal = hub.member_principal(&member.id).unwrap();
    let group = hub.create_group("team", &master).expect("group").id;
    hub.add_group_member(&group, &principal, &master)
        .expect("add");
    hub.set_loca_group("mover", Some(&group), &master)
        .expect("into the group");
    assert!(
        held(&hub, &member.id).is_empty(),
        "the group seats it, no davet"
    );
    hub.set_loca_group("mover", None, &master)
        .expect("back to default");
    let carried = held(&hub, &member.id);
    assert_eq!(carried.len(), 1, "carried over by invite");
    let sent = connect(&hub, &member.id);
    assert_eq!(hashed(&sent), carried);
    assert_eq!(
        store
            .invite_delivery_raw_by_hash(&carried[0].1)
            .unwrap()
            .0
            .as_deref(),
        Some(sent[0].1.as_str())
    );
}

#[test]
fn revoking_one_credential_is_announced_by_its_id() {
    let _clock = CLOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_now(ISSUED_AT);
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open_at(dir.path().join("door.db").to_str(), ISSUED_AT).unwrap());
    let hub = hub_on(store.clone());
    let member = hub.admit_member("worker", "agent", "master").unwrap();
    let principal = hub.member_principal(&member.id).unwrap();
    let (device, _) = hub.create_profile_credential(&principal, "device").unwrap();
    let mut lobby = hub.subscribe_lobby();
    hub.revoke_profile_credential(&principal, &device.id)
        .unwrap();
    match lobby.try_recv() {
        Ok(LobbyEvent::CredentialRevoked { credential_id }) => assert_eq!(credential_id, device.id),
        other => panic!("expected CredentialRevoked, got {other:?}"),
    }
}

/// An invite row that cannot be read stops the start; it is not left out of
/// the map as if it did not exist (the member would lose a seat silently).
#[test]
#[should_panic(expected = "persistent invites are unreadable")]
fn an_unreadable_invite_stops_the_start() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("invites.db");
    let store = Arc::new(Store::open_at(path.to_str(), ISSUED_AT).unwrap());
    store
        .issue_invite(
            &crate::store::InviteRecord::issued(
                "dv_damaged",
                "parlor",
                "",
                "worker",
                "agent",
                ISSUED_AT,
                "master",
            ),
            "dv_damaged",
        )
        .unwrap();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("UPDATE invites SET issued_at = 'not a time'")
        .unwrap();
    hub_on(store);
}

#[test]
#[should_panic(expected = "persistent memberships are unreadable")]
fn an_unreadable_membership_table_stops_the_start() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("members.db");
    let store = Arc::new(Store::open_at(path.to_str(), ISSUED_AT).unwrap());
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("DROP TABLE member_suspensions; DROP TABLE members")
        .unwrap();
    hub_on(store);
}
