//! Personal appearance at the store: the allow-lists grow, and a row saved
//! before they grew still means exactly what it meant.

use rusqlite::params;
use serde_json::json;

use super::super::preferences::{
    PREFERENCE_ACCENTS, PREFERENCE_DENSITIES, PREFERENCE_FONT_FAMILIES, PREFERENCE_FONT_SIZES,
    PREFERENCE_THEMES,
};
use super::super::{PreferenceError, Preferences, Store};

/// A clock shaped like a real one (milliseconds since the epoch).
const T0: u64 = 1_800_000_000_000;

fn member(store: &Store, name: &str) -> String {
    let key = format!("mb_{name}PREFFIXTURE");
    store
        .add_member(&key, name, "user", T0 - 1_000, "master")
        .unwrap();
    store.principal_for_credential(&key).unwrap().id
}

fn open(directory: &tempfile::TempDir) -> Store {
    let path = directory.path().join("preferences.db");
    Store::open_at(path.to_str(), T0).unwrap()
}

fn as_json(prefs: &Preferences) -> serde_json::Value {
    serde_json::to_value(prefs).unwrap()
}

#[test]
fn absent_keys_take_their_defaults_including_the_new_font_family() {
    let directory = tempfile::tempdir().unwrap();
    let store = open(&directory);
    let ada = member(&store, "ada");
    assert_eq!(
        as_json(&store.principal_preferences(&ada)),
        json!({
            "theme": "dark", "accent": "teal", "density": "comfortable",
            "font_size": "normal", "font_family": "mono"
        })
    );
}

#[test]
fn every_allowed_value_is_accepted_and_saved() {
    let directory = tempfile::tempdir().unwrap();
    let store = open(&directory);
    let ada = member(&store, "ada");
    let lists: [(&str, &[&str]); 5] = [
        ("theme", PREFERENCE_THEMES),
        ("accent", PREFERENCE_ACCENTS),
        ("density", PREFERENCE_DENSITIES),
        ("font_size", PREFERENCE_FONT_SIZES),
        ("font_family", PREFERENCE_FONT_FAMILIES),
    ];
    // The new values this change adds, named so a shrunk list fails here.
    for (key, value) in [
        ("density", "spacious"),
        ("font_size", "small"),
        ("font_size", "larger"),
        ("font_family", "sans"),
        ("font_family", "serif"),
    ] {
        let allowed = lists.iter().find(|(k, _)| *k == key).unwrap().1;
        assert!(allowed.contains(&value), "{key}={value} must be allowed");
    }
    for (key, allowed) in lists {
        for (step, value) in allowed.iter().enumerate() {
            let saved = store
                .merge_principal_preferences(&ada, &json!({ key: value }), T0 + step as u64)
                .unwrap();
            assert_eq!(as_json(&saved)[key], *value);
            assert_eq!(as_json(&store.principal_preferences(&ada))[key], *value);
        }
    }
}

#[test]
fn one_unknown_value_rejects_the_whole_body_and_changes_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let store = open(&directory);
    let ada = member(&store, "ada");
    let before = store
        .merge_principal_preferences(&ada, &json!({ "font_family": "serif" }), T0)
        .unwrap();
    for bad in [
        json!({ "font_family": "sans", "density": "roomy" }),
        json!({ "font_size": "larger", "font_family": "comic" }),
        json!({ "font_family": "Georgia, serif" }),
        json!({ "font_family": "https://fonts.example/x.woff2" }),
        json!({ "font_size": "huge" }),
        json!({ "density": "Spacious" }),
        json!({ "font_family": null }),
    ] {
        let refused = store.merge_principal_preferences(&ada, &bad, T0 + 1);
        assert!(
            matches!(refused, Err(PreferenceError::Invalid(_))),
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
fn a_row_saved_in_the_old_four_key_shape_loads_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let store = open(&directory);
    let ada = member(&store, "ada");
    // Exactly what a server before this change wrote.
    let old = r#"{"theme":"light","accent":"violet","density":"compact","font_size":"large"}"#;
    store
        .conn()
        .unwrap()
        .execute(
            "INSERT INTO principal_preferences (principal_id, json, updated_at) VALUES (?1, ?2, ?3)",
            params![ada, old, T0],
        )
        .unwrap();
    let loaded = as_json(&store.principal_preferences(&ada));
    assert_eq!(
        loaded,
        json!({
            "theme": "light", "accent": "violet", "density": "compact",
            "font_size": "large", "font_family": "mono"
        })
    );
    // An older client then writes only the four keys it knows: accepted, and
    // the new key it does not know keeps its value.
    store
        .merge_principal_preferences(&ada, &json!({ "font_family": "sans" }), T0 + 1)
        .unwrap();
    let saved = store
        .merge_principal_preferences(
            &ada,
            &json!({ "theme": "dark", "accent": "teal", "density": "comfortable", "font_size": "normal" }),
            T0 + 2,
        )
        .unwrap();
    assert_eq!(
        as_json(&saved),
        json!({
            "theme": "dark", "accent": "teal", "density": "comfortable",
            "font_size": "normal", "font_family": "sans"
        })
    );
}
