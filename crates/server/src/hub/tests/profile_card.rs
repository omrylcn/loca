//! A person's full name and icon through the hub: what a full name may hold,
//! and that it is only ever a label beside the `@name`.

use std::sync::Arc;

use protocol::{Member, SenderType, ServerFrame};
use serde_json::json;

use super::super::profile_card::{
    validate_full_name, validate_icon, FULL_NAME_MAX_CHARS, PROFILE_ICONS,
};
use super::critical_restart_hub;
use crate::store::{ProfileCardError, Store};

fn refused(raw: &str) -> String {
    match validate_full_name(raw) {
        Err(reason) => reason,
        Ok(kept) => panic!("{raw:?} was accepted as {kept:?}"),
    }
}

#[test]
fn a_full_name_keeps_letters_of_every_script_and_the_punctuation_of_names() {
    for (raw, kept) in [
        ("Ömer Yalçın", "Ömer Yalçın"),
        ("José María O'Neill", "José María O'Neill"),
        ("Anne-Marie d\u{2019}Arc", "Anne-Marie d\u{2019}Arc"),
        ("J. R. R. Tolkien", "J. R. R. Tolkien"),
        ("Lluís Gil\u{00B7}la", "Lluís Gil\u{00B7}la"),
        ("山田 太郎", "山田 太郎"),
        ("Мария Иванова", "Мария Иванова"),
        ("محمد علي", "محمد علي"),
        ("O\u{308}mer", "O\u{308}mer"),
        ("  Ada    Lovelace  ", "Ada Lovelace"),
        // Equal to somebody's @name is allowed: the @name is shown beside it.
        ("pref-ada", "pref-ada"),
        // A rank word in another script reads alike but is only a label.
        ("\u{041C}aster", "\u{041C}aster"),
        ("Master Yoda", "Master Yoda"),
    ] {
        assert_eq!(
            validate_full_name(raw),
            Ok(Some(kept.to_string())),
            "{raw:?}"
        );
    }
    let longest = "a".repeat(FULL_NAME_MAX_CHARS);
    assert_eq!(validate_full_name(&longest), Ok(Some(longest.clone())));
    // Empty or only spaces clears.
    assert_eq!(validate_full_name(""), Ok(None));
    assert_eq!(validate_full_name("   "), Ok(None));
}

#[test]
fn each_class_of_refused_character_is_refused_for_its_own_reason() {
    for raw in [
        "Ada\nLovelace",
        "Ada\rLovelace",
        "Ada\u{2028}Lovelace",
        "Ada\u{2029}L",
        "Ada\u{85}L",
    ] {
        assert!(refused(raw).contains("one line"), "{raw:?}");
    }
    for raw in [
        "Ada\u{7}",
        "Ada\u{0}",
        "Ada\tLovelace",
        "Ada\u{1B}[31m",
        "Ada\u{9F}",
    ] {
        assert!(refused(raw).contains("control"), "{raw:?}");
    }
    for raw in [
        "\u{202E}ecalevoL adA",
        "Ada\u{202D}x",
        "Ada\u{2066}x\u{2069}",
        "Ada\u{200F}",
        "Ada\u{061C}",
    ] {
        assert!(refused(raw).contains("direction"), "{raw:?}");
    }
    for raw in [
        "Ad\u{200B}a",
        "Ad\u{200D}a",
        "Ada\u{FEFF}",
        "Ma\u{00AD}ster",
        "\u{3164}",
        "Ada \u{3164}",
        "A\u{034F}da",
        "Ada\u{FE0F}",
        "Ada\u{E0041}",
    ] {
        assert!(refused(raw).contains("invisible"), "{raw:?}");
    }
    for raw in ["@pref-ada", "Ada @ Home", "ada@example"] {
        assert!(refused(raw).contains("not an address"), "{raw:?}");
    }
    for raw in [
        "Ada 2",
        "Ada \u{1F600}",
        "Ada <b>",
        "Ada & co",
        "Ada#1",
        "Ada_Lovelace",
        "\u{2800}",
    ] {
        assert!(refused(raw).contains("letters, spaces"), "{raw:?}");
    }
    for raw in ["Ada\u{00A0}Lovelace", "Ada\u{3000}Lovelace", "Ada\u{2003}L"] {
        assert!(refused(raw).contains("plain spaces"), "{raw:?}");
    }
    for raw in ["\u{301}Ada", "A\u{301}\u{301}\u{301}da", "Ada \u{301}"] {
        assert!(refused(raw).contains("accent"), "{raw:?}");
    }
    for raw in ["-- ..", "'"] {
        assert!(refused(raw).contains("one letter"), "{raw:?}");
    }
    assert!(refused(&"a".repeat(FULL_NAME_MAX_CHARS + 1)).contains("at most"));
    for raw in [
        "Master",
        "master",
        "MASTER",
        "Mas ter",
        "S-master",
        "Operator",
        "lead",
        "All",
        "every one",
        "Root",
        "Ad.min",
    ] {
        assert!(refused(raw).contains("rank or title"), "{raw:?}");
    }
}

#[test]
fn an_icon_is_one_of_the_closed_list() {
    for icon in PROFILE_ICONS {
        assert_eq!(validate_icon(icon), Ok(Some(icon.to_string())));
    }
    assert_eq!(validate_icon(""), Ok(None));
    for raw in [
        "\u{1F642}",
        "https://example.invalid/a.png",
        "Leaf",
        "../leaf",
        "leaf ",
        "star*",
    ] {
        assert!(validate_icon(raw).is_err(), "{raw:?}");
    }
}

fn last_roster(rx: &mut tokio::sync::broadcast::Receiver<ServerFrame>) -> Option<Vec<Member>> {
    let mut last = None;
    while let Ok(frame) = rx.try_recv() {
        if let ServerFrame::Members { members } = frame {
            last = Some(members);
        }
    }
    last
}

fn entry<'a>(roster: &'a [Member], name: &str) -> &'a Member {
    roster
        .iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("{name} is on the roster"))
}

#[test]
fn two_people_with_one_full_name_stay_two_and_an_open_door_seat_borrows_no_card() {
    let temp = tempfile::tempdir().unwrap();
    let store =
        Arc::new(Store::open(Some(temp.path().join("cards.db").to_str().unwrap())).unwrap());
    let hub = critical_restart_hub(store, "root");
    let ada = hub.admit_member("ada", "user", "master").unwrap();
    let bo = hub.admit_member("bo", "agent", "master").unwrap();
    let ada_principal = ada.principal_id.clone().unwrap();
    let bo_principal = bo.principal_id.clone().unwrap();
    for principal in [&ada_principal, &bo_principal] {
        hub.update_profile_card(
            principal,
            &json!({ "full_name": "Ada Lovelace", "icon": "leaf" }),
        )
        .unwrap();
    }
    let mut rx = hub.subscribe("ops").0;
    let (ada_seat, bo_seat) = (format!("mb:{}", ada.id), format!("mb:{}", bo.id));
    hub.prove_seat("ops", &ada_seat, Some(&ada_principal));
    assert!(hub.join("ops", &ada_seat, "ada", SenderType::User, 1));
    hub.prove_seat("ops", &bo_seat, Some(&bo_principal));
    assert!(hub.join("ops", &bo_seat, "bo", SenderType::Agent, 2));
    let roster = last_roster(&mut rx).expect("a roster frame");
    assert_eq!(roster.len(), 2, "two people, not one");
    for name in ["ada", "bo"] {
        let member = entry(&roster, name);
        assert_eq!(member.full_name.as_deref(), Some("Ada Lovelace"));
        assert_eq!(member.icon.as_deref(), Some("leaf"));
        assert_eq!(member.role, None, "a full name carries no rank");
    }
    assert_eq!(
        entry(&roster, "ada").principal_id.as_deref(),
        Some(ada_principal.as_str())
    );
    assert_eq!(
        entry(&roster, "bo").principal_id.as_deref(),
        Some(bo_principal.as_str())
    );
    assert_eq!(
        serde_json::to_value(hub.members("ops")).unwrap(),
        serde_json::to_value(&roster).unwrap(),
        "GET /rooms/:id/members says the same"
    );

    // A seat whose principal the door did not verify (typed at an open
    // door) wears the plain name, even under the same label.
    let mut open = hub.subscribe("open").0;
    hub.prove_seat("open", "name:ada", None);
    assert!(hub.join("open", "name:ada", "ada", SenderType::User, 3));
    let roster = last_roster(&mut open).expect("a roster frame");
    assert_eq!(entry(&roster, "ada").full_name, None);
    assert_eq!(entry(&roster, "ada").icon, None);
    assert_eq!(entry(&roster, "ada").principal_id, None);

    // A body that names another person changes nobody.
    for bad in [
        json!({ "full_name": "Bo Takes Over", "principal_id": ada_principal }),
        json!({ "full_name": "Bo Takes Over", "name": "ada" }),
        json!({ "full_name": "Bo Ek", "icon": "rocket" }),
        json!({ "full_name": 7 }),
        json!(["Bo"]),
    ] {
        assert!(
            matches!(
                hub.update_profile_card(&bo_principal, &bad),
                Err(ProfileCardError::Invalid(_))
            ),
            "{bad} must be refused"
        );
    }
    assert_eq!(
        hub.profile_card(&ada_principal).full_name.as_deref(),
        Some("Ada Lovelace")
    );
    assert_eq!(
        hub.profile_card(&bo_principal).full_name.as_deref(),
        Some("Ada Lovelace")
    );

    // Clearing Bo's sends the roster again where Bo sits; Ada keeps hers.
    let cleared = hub
        .update_profile_card(&bo_principal, &json!({ "full_name": null, "icon": "" }))
        .unwrap();
    assert!(cleared.is_empty());
    let roster = last_roster(&mut rx).expect("the change is announced");
    assert_eq!(entry(&roster, "bo").full_name, None);
    assert_eq!(entry(&roster, "bo").icon, None);
    assert_eq!(
        entry(&roster, "ada").full_name.as_deref(),
        Some("Ada Lovelace")
    );
    assert!(
        last_roster(&mut open).is_none(),
        "no roster where Bo has no seat"
    );
}
