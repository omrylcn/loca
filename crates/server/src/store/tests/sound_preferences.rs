//! Sounds in the personal preferences: a closed set of switches and a volume,
//! saved per principal like the look, and read leniently when a stored value
//! is no longer allowed.

use rusqlite::params;
use serde_json::json;

use super::super::{PreferenceError, Store};

const T0: u64 = 1_800_000_000_000;

fn member(store: &Store, name: &str) -> String {
    let key = format!("mb_{name}SOUNDFIXTURE");
    store
        .add_member(&key, name, "user", T0 - 1_000, "master")
        .unwrap();
    store.principal_for_credential(&key).unwrap().id
}

fn open(directory: &tempfile::TempDir) -> Store {
    let path = directory.path().join("sounds.db");
    Store::open_at(path.to_str(), T0).unwrap()
}

fn as_json(store: &Store, principal: &str) -> serde_json::Value {
    serde_json::to_value(store.principal_preferences(principal)).unwrap()
}

/// Every key, in the order the Sounds section shows it, with the values it
/// takes, default first. Written out here (not read from the allow-list) so
/// a shrunk list fails this test.
fn sound_keys() -> [(&'static str, &'static [&'static str]); 7] {
    const ON_FIRST: &[&str] = &["on", "off"];
    [
        ("sound", ON_FIRST),
        ("sound_message", &["off", "on"]),
        ("sound_mention", ON_FIRST),
        ("sound_dm", ON_FIRST),
        ("sound_notice", ON_FIRST),
        ("sound_volume", &["medium", "low", "high"]),
        ("sound_background", ON_FIRST),
    ]
}

#[test]
fn sounds_default_to_on_for_what_is_addressed_to_you_and_off_for_plain_chatter() {
    let directory = tempfile::tempdir().unwrap();
    let store = open(&directory);
    let ada = member(&store, "ada");
    let prefs = as_json(&store, &ada);
    assert_eq!(prefs["sound"], "on");
    assert_eq!(prefs["sound_message"], "off", "plain messages stay quiet");
    assert_eq!(prefs["sound_mention"], "on");
    assert_eq!(prefs["sound_dm"], "on");
    assert_eq!(prefs["sound_notice"], "on");
    assert_eq!(prefs["sound_volume"], "medium");
    assert_eq!(prefs["sound_background"], "on");
    // Each list starts with its default, as the look's lists do.
    for (key, allowed) in sound_keys() {
        assert_eq!(prefs[key], allowed[0], "{key} default is its list's first");
    }
}

#[test]
fn every_allowed_sound_value_is_saved_and_read_back() {
    let directory = tempfile::tempdir().unwrap();
    let store = open(&directory);
    let ada = member(&store, "ada");
    for (step, (key, allowed)) in sound_keys().into_iter().enumerate() {
        for value in allowed.iter() {
            let saved = store
                .merge_principal_preferences(&ada, &json!({ key: value }), T0 + step as u64)
                .unwrap();
            assert_eq!(serde_json::to_value(&saved).unwrap()[key], *value);
            assert_eq!(as_json(&store, &ada)[key], *value);
        }
    }
    // A sound change leaves the look alone, and the look leaves the sounds.
    store
        .merge_principal_preferences(&ada, &json!({ "sound_dm": "off" }), T0 + 10)
        .unwrap();
    let after = store
        .merge_principal_preferences(&ada, &json!({ "theme": "light" }), T0 + 11)
        .unwrap();
    let after = serde_json::to_value(&after).unwrap();
    assert_eq!(after["theme"], "light");
    assert_eq!(after["sound_dm"], "off");
}

#[test]
fn a_bad_sound_value_or_key_refuses_the_whole_body_and_changes_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let store = open(&directory);
    let ada = member(&store, "ada");
    let before = store
        .merge_principal_preferences(
            &ada,
            &json!({ "sound_volume": "low", "sound_message": "on" }),
            T0,
        )
        .unwrap();
    for bad in [
        json!({ "sound": "loud" }),
        json!({ "sound": true }),
        json!({ "sound_volume": "max" }),
        json!({ "sound_volume": 0.5 }),
        json!({ "sound_volume": "Medium" }),
        json!({ "sound_background": "sometimes" }),
        json!({ "sound_mention": null }),
        json!({ "sound_ping": "on" }),
        json!({ "sound_dm": "off", "sound_volume": "deafening" }),
        json!({ "sound_notice": "off", "theme": "solarized" }),
    ] {
        assert!(
            matches!(
                store.merge_principal_preferences(&ada, &bad, T0 + 1),
                Err(PreferenceError::Invalid(_))
            ),
            "{bad} must be refused"
        );
        assert_eq!(
            store.principal_preferences(&ada),
            before,
            "{bad} changed the row"
        );
    }
}

#[test]
fn a_stored_sound_value_no_longer_allowed_falls_back_to_its_default() {
    let directory = tempfile::tempdir().unwrap();
    let store = open(&directory);
    let ada = member(&store, "ada");
    // A row written by an older server has no sound keys at all; one with a
    // retired value keeps the rest of the document.
    let stored =
        r#"{"theme":"paper","sound_volume":"deafening","sound_dm":"off","sound_message":"maybe"}"#;
    store
        .conn()
        .unwrap()
        .execute(
            "INSERT INTO principal_preferences (principal_id, json, updated_at) VALUES (?1, ?2, ?3)",
            params![ada, stored, T0],
        )
        .unwrap();
    let loaded = as_json(&store, &ada);
    assert_eq!(loaded["theme"], "paper");
    assert_eq!(loaded["sound_dm"], "off");
    assert_eq!(loaded["sound_volume"], "medium");
    assert_eq!(loaded["sound_message"], "off");
    assert_eq!(loaded["sound"], "on");
}
