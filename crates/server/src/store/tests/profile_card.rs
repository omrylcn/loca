//! A person's full name and icon at the store: on the principal row, partial
//! writes, clearing, a restart, and a database written before the columns.

use rusqlite::{params, Connection};

use super::super::{ProfileCard, ProfileCardError, Store};

/// A clock shaped like a real one (milliseconds since the epoch).
const T0: u64 = 1_800_000_000_000;

const V0_13_0_DUMP: &str = include_str!("../../../tests/fixtures/upgrade-v0.13.0/v0.13.0.sql");

fn member(store: &Store, name: &str) -> String {
    let key = format!("mb_{name}CARDFIXTURE");
    store
        .add_member(&key, name, "user", T0 - 1_000, "master")
        .unwrap();
    store.principal_for_credential(&key).unwrap().id
}

fn card(full_name: Option<&str>, icon: Option<&str>) -> ProfileCard {
    ProfileCard {
        full_name: full_name.map(str::to_string),
        icon: icon.map(str::to_string),
    }
}

#[test]
fn a_card_is_set_in_parts_cleared_and_survives_a_restart() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cards.db");
    let (ada, bo) = {
        let store = Store::open_at(path.to_str(), T0).unwrap();
        let ada = member(&store, "ada");
        let bo = member(&store, "bo");
        assert_eq!(store.profile_card(&ada), ProfileCard::default());
        assert!(store
            .people_by_name()
            .values()
            .all(|(_, card)| card.is_empty()));

        let saved = store
            .set_profile_card(&ada, Some(Some("Ada Lovelace")), None)
            .unwrap();
        assert_eq!(saved, card(Some("Ada Lovelace"), None));
        // A field left out keeps its value.
        let saved = store
            .set_profile_card(&ada, None, Some(Some("leaf")))
            .unwrap();
        assert_eq!(saved, card(Some("Ada Lovelace"), Some("leaf")));
        // Bo's write never touches Ada's row.
        store
            .set_profile_card(&bo, Some(Some("Bo Ek")), Some(Some("moon")))
            .unwrap();
        store.set_profile_card(&bo, None, Some(None)).unwrap();
        assert_eq!(store.profile_card(&bo), card(Some("Bo Ek"), None));
        (ada, bo)
    };
    let store = Store::open_at(path.to_str(), T0 + 1).unwrap();
    assert_eq!(
        store.profile_card(&ada),
        card(Some("Ada Lovelace"), Some("leaf"))
    );
    let by_name = store.people_by_name();
    assert_eq!(
        by_name["ada"],
        (ada.clone(), card(Some("Ada Lovelace"), Some("leaf")))
    );
    assert_eq!(by_name["bo"], (bo.clone(), card(Some("Bo Ek"), None)));
    let by_id = store.profile_cards_by_principal();
    assert_eq!(by_id.len(), 2);
    assert_eq!(by_id[&ada], card(Some("Ada Lovelace"), Some("leaf")));
    // Cleared entirely, the person keeps an id but no card.
    store.set_profile_card(&bo, Some(None), None).unwrap();
    assert!(!store.profile_cards_by_principal().contains_key(&bo));
    assert_eq!(
        store.people_by_name()["bo"],
        (bo.clone(), ProfileCard::default())
    );
}

#[test]
fn a_disabled_principal_cannot_write_and_is_not_listed() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_at(directory.path().join("off.db").to_str(), T0).unwrap();
    let ada = member(&store, "ada");
    store
        .set_profile_card(&ada, Some(Some("Ada Lovelace")), None)
        .unwrap();
    store
        .conn()
        .unwrap()
        .execute(
            "UPDATE principals SET disabled_at = ?2 WHERE id = ?1",
            params![ada, T0 + 5],
        )
        .unwrap();
    assert!(!store.people_by_name().contains_key("ada"));
    assert!(store.profile_cards_by_principal().is_empty());
    assert_eq!(store.profile_card(&ada), ProfileCard::default());
    assert_eq!(
        store.set_profile_card(&ada, Some(Some("Someone Else")), None),
        Err(ProfileCardError::NoProfile)
    );
    assert_eq!(
        store.set_profile_card("pr_nobody", Some(Some("Nobody")), None),
        Err(ProfileCardError::NoProfile)
    );
}

#[test]
fn a_name_two_active_principals_share_lends_neither_card() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_at(directory.path().join("twins.db").to_str(), T0).unwrap();
    let ada = member(&store, "ada");
    store
        .set_profile_card(&ada, Some(Some("Ada Lovelace")), None)
        .unwrap();
    // A row from before names were unique, wearing the same name.
    store
        .conn()
        .unwrap()
        .execute(
            "INSERT INTO principals (id, display_name, kind, building_role, created_at, full_name)
             VALUES ('pr_legacy_twin', 'ada', 'human', 'member', ?1, 'Not Ada')",
            params![T0],
        )
        .unwrap();
    assert!(!store.people_by_name().contains_key("ada"));
    // Each principal still has its own card.
    assert_eq!(
        store.profile_card(&ada).full_name.as_deref(),
        Some("Ada Lovelace")
    );
    assert_eq!(
        store.profile_card("pr_legacy_twin").full_name.as_deref(),
        Some("Not Ada")
    );
}

#[test]
fn a_database_from_v0_13_0_gains_the_columns_and_every_person_reads_as_before() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("old.db");
    Connection::open(&path)
        .unwrap()
        .execute_batch(V0_13_0_DUMP)
        .unwrap();
    let before: Vec<(String, String)> = Connection::open(&path)
        .unwrap()
        .prepare("SELECT id, display_name FROM principals WHERE disabled_at IS NULL ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(!before.is_empty(), "the fixture holds people");

    let store = Store::open_at(path.to_str(), T0).unwrap();
    let columns: Vec<String> = store
        .conn()
        .unwrap()
        .prepare("PRAGMA table_info(principals)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(columns.contains(&"full_name".to_string()));
    assert!(columns.contains(&"icon".to_string()));
    for (id, _) in &before {
        assert_eq!(store.profile_card(id), ProfileCard::default(), "{id}");
    }
    assert!(store.profile_cards_by_principal().is_empty());
    let (id, name) = &before[0];
    let saved = store
        .set_profile_card(id, Some(Some("Upgraded Person")), Some(Some("sun")))
        .unwrap();
    assert_eq!(saved, card(Some("Upgraded Person"), Some("sun")));
    drop(store);
    // Opening again is a no-op for the columns, and the card stays.
    let store = Store::open_at(path.to_str(), T0 + 1).unwrap();
    assert_eq!(
        store.people_by_name().get(name),
        Some(&(id.clone(), card(Some("Upgraded Person"), Some("sun"))))
    );
}
