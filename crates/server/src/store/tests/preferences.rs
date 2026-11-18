//! Personal appearance at the store: the allow-lists grow, and a row saved
//! before they grew still means exactly what it meant.

use rusqlite::params;
use serde_json::json;

use super::super::preferences::{
    PREFERENCE_ACCENTS, PREFERENCE_DENSITIES, PREFERENCE_FONT_FAMILIES, PREFERENCE_FONT_SIZES,
    PREFERENCE_LAYOUTS, PREFERENCE_MEASURES, PREFERENCE_MOTIONS, PREFERENCE_OWN_TINTS,
    PREFERENCE_THEMES, PREFERENCE_TIME_FORMATS,
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
            "font_size": "normal", "font_family": "mono", "accent_custom": "",
            "layout": "bubbles", "time_format": "auto", "measure": "normal",
            "own_tint": "on", "motion": "system",
            "sound": "on", "sound_message": "off", "sound_mention": "on", "sound_dm": "on",
            "sound_notice": "on", "sound_volume": "medium", "sound_background": "on"
        })
    );
}

#[test]
fn the_reading_choices_and_a_custom_accent_are_saved_and_a_bad_one_changes_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let store = open(&directory);
    let ada = member(&store, "ada");
    // Every theme and accent an older server accepted stays.
    for theme in ["dark", "light"] {
        assert!(PREFERENCE_THEMES.contains(&theme));
    }
    for accent in ["teal", "blue", "violet", "amber", "rose"] {
        assert!(PREFERENCE_ACCENTS.contains(&accent));
    }
    let lists: [(&str, &[&str]); 7] = [
        ("theme", PREFERENCE_THEMES),
        ("accent", PREFERENCE_ACCENTS),
        ("layout", PREFERENCE_LAYOUTS),
        ("time_format", PREFERENCE_TIME_FORMATS),
        ("measure", PREFERENCE_MEASURES),
        ("own_tint", PREFERENCE_OWN_TINTS),
        ("motion", PREFERENCE_MOTIONS),
    ];
    for (key, allowed) in lists {
        for value in allowed.iter() {
            let saved = store
                .merge_principal_preferences(&ada, &json!({ key: value }), T0)
                .unwrap();
            assert_eq!(as_json(&saved)[key], *value);
        }
    }
    let saved = store
        .merge_principal_preferences(
            &ada,
            &json!({ "accent": "custom", "accent_custom": "#3A7BD5", "theme": "paper" }),
            T0 + 1,
        )
        .unwrap();
    assert_eq!(saved.accent_custom, "#3a7bd5", "stored lower-case");
    let before = store.principal_preferences(&ada);
    assert_eq!(before, saved);
    for bad in [
        json!({ "accent_custom": "#3a7bd" }),
        json!({ "accent_custom": "3a7bd5" }),
        json!({ "accent_custom": "#3a7bd5ff" }),
        json!({ "accent_custom": "red" }),
        json!({ "accent_custom": "#zzzzzz" }),
        json!({ "accent_custom": "#123456; background: url(x)" }),
        json!({ "accent_custom": null }),
        json!({ "layout": "grid" }),
        json!({ "time_format": "H24" }),
        json!({ "measure": "huge" }),
        json!({ "own_tint": true }),
        json!({ "motion": "none", "layout": "flat" }),
        json!({ "theme": "solarized" }),
        json!({ "accent": "#3a7bd5" }),
    ] {
        assert!(
            matches!(
                store.merge_principal_preferences(&ada, &bad, T0 + 2),
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
    // An empty custom colour clears it.
    let cleared = store
        .merge_principal_preferences(&ada, &json!({ "accent_custom": "" }), T0 + 3)
        .unwrap();
    assert_eq!(cleared.accent_custom, "");
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
            "font_size": "large", "font_family": "mono",
            "accent_custom": "", "layout": "bubbles", "time_format": "auto", "measure": "normal", "own_tint": "on", "motion": "system",
            "sound": "on", "sound_message": "off", "sound_mention": "on", "sound_dm": "on",
            "sound_notice": "on", "sound_volume": "medium", "sound_background": "on"
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
            "font_size": "normal", "font_family": "sans",
            "accent_custom": "", "layout": "bubbles", "time_format": "auto", "measure": "normal", "own_tint": "on", "motion": "system",
            "sound": "on", "sound_message": "off", "sound_mention": "on", "sound_dm": "on",
            "sound_notice": "on", "sound_volume": "medium", "sound_background": "on"
        })
    );
}
