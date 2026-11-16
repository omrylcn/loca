//! A loca's own seats in the hub: what is held (live invites or people at
//! the table, whichever is more), an edit below it refused, and the two
//! Building numbers (default and ceiling).

use std::collections::HashSet;
use std::sync::Arc;

use protocol::{RoomSettings, SenderType};

use super::super::{Hub, HubConfig, IdentityEdit, IdentityError, InviteError, RequestAuthority};
use crate::store::Store;

const ROOT: &str = "seats-fixture-root";

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

fn seats(n: u32) -> IdentityEdit {
    IdentityEdit {
        seats: Some(n),
        ..IdentityEdit::default()
    }
}

#[test]
fn held_seats_are_the_live_invites_or_the_people_seated_whichever_is_more() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub_on(Arc::new(
        Store::open(dir.path().join("seats.db").to_str()).unwrap(),
    ));
    hub.create_loca_with("atelier", &master(&hub), None, None, &seats(6))
        .expect("loca");
    assert_eq!(hub.room_capacity("atelier"), 6);

    // Two invites, nobody seated: two held.
    let a = hub.admit_member("held-a", "agent", "master").unwrap();
    let b = hub.admit_member("held-b", "agent", "master").unwrap();
    hub.invite_member_to_room(&a.id, "atelier", "master")
        .unwrap();
    hub.invite_member_to_room(&b.id, "atelier", "master")
        .unwrap();
    assert_eq!(
        hub.edit_identity("atelier", &seats(1), "Master", SenderType::User, |_| false)
            .unwrap_err(),
        IdentityError::Invalid(
            "seats must be between 2 and 32 (this Building's ceiling, LOCA_CAPACITY_MAX)".into()
        ),
        "below the minimum is a merit refusal, not a held-seat one"
    );
    // Three people sit without an invite (ranks, a sandbox key): three held.
    for (n, who) in ["rank-1", "rank-2", "rank-3"].iter().enumerate() {
        assert!(hub.join("atelier", who, who, SenderType::User, n as u64 + 1));
    }
    assert_eq!(
        hub.edit_identity("atelier", &seats(2), "Master", SenderType::User, |_| false)
            .unwrap_err(),
        IdentityError::SeatsHeld { held: 3, wanted: 2 }
    );
    assert_eq!(
        hub.room_capacity("atelier"),
        6,
        "a refused edit changes nothing"
    );
    let lowered = hub
        .edit_identity("atelier", &seats(3), "Master", SenderType::User, |_| false)
        .expect("exactly the seats held");
    assert_eq!(lowered.seats, Some(3));
    assert_eq!(hub.room_capacity("atelier"), 3);

    // Two leave: the invites (two) are now what is held.
    hub.leave("atelier", "rank-1", 1);
    hub.leave("atelier", "rank-2", 2);
    let held = hub
        .edit_identity("atelier", &seats(2), "Master", SenderType::User, |_| false)
        .expect("two invites, one seated: two held");
    assert_eq!(held.seats, Some(2));
    let c = hub.admit_member("held-c", "agent", "master").unwrap();
    assert_eq!(
        hub.invite_member_to_room(&c.id, "atelier", "master")
            .unwrap_err(),
        InviteError::Full
    );
}

#[test]
fn the_ceiling_is_never_below_the_default_nor_above_thirty_two() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub_on(Arc::new(
        Store::open(dir.path().join("ceiling.db").to_str()).unwrap(),
    ));
    assert_eq!((hub.loca_capacity(), hub.loca_capacity_max()), (7, 32));
    hub.set_loca_capacity(12);
    assert_eq!(hub.set_loca_capacity_max(5), 12);
    assert_eq!(hub.set_loca_capacity_max(40), 32);
    assert_eq!(hub.set_loca_capacity_max(20), 20);
    // A loca without a number has the default; one with a number keeps it
    // under the ceiling, and the shown settings say the number in force.
    hub.create_loca_with("plain", &master(&hub), None, None, &IdentityEdit::default())
        .unwrap();
    hub.create_loca_with("wide", &master(&hub), None, None, &seats(18))
        .unwrap();
    assert_eq!(hub.room_capacity("plain"), 12);
    assert_eq!(hub.room_capacity("wide"), 18);
    hub.set_loca_capacity_max(15);
    assert_eq!(hub.room_capacity("wide"), 15);
    assert_eq!(hub.settings("wide").seats, Some(15));
    hub.set_loca_capacity_max(32);
    assert_eq!(
        hub.settings("wide").seats,
        Some(18),
        "the stored number is kept"
    );
}
