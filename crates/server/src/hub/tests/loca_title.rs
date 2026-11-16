//! A loca's title in the hub: display only. A title change writes the loca's
//! settings and one Journal line and touches no other row, so the id (and
//! every record keyed by it) stays where it is; and a title is refused when
//! it would read like another loca the caller can see.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use protocol::{RoomSettings, SenderType};
use rusqlite::types::Value;

use super::super::{Hub, HubConfig, IdentityEdit, IdentityError, RequestAuthority};
use crate::store::Store;

const ROOT: &str = "title-fixture-root";

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

fn title(text: &str) -> IdentityEdit {
    IdentityEdit {
        title: Some(text.into()),
        ..IdentityEdit::default()
    }
}

/// Every row of every table, as text, by table.
fn every_row(path: &std::path::Path) -> BTreeMap<String, Vec<String>> {
    let conn = rusqlite::Connection::open(path).unwrap();
    let tables: Vec<String> = conn
        .prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' \
             ORDER BY name",
        )
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|t| t.unwrap())
        .collect();
    tables
        .into_iter()
        .map(|table| {
            let mut stmt = conn.prepare(&format!("SELECT * FROM \"{table}\"")).unwrap();
            let width = stmt.column_count();
            let mut rows: Vec<String> = stmt
                .query_map([], |r| {
                    Ok((0..width)
                        .map(|i| format!("{:?}", r.get::<_, Value>(i).unwrap()))
                        .collect::<Vec<_>>()
                        .join(" | "))
                })
                .unwrap()
                .map(|row| row.unwrap())
                .collect();
            rows.sort();
            (table, rows)
        })
        .collect()
}

#[test]
fn a_title_change_touches_no_row_outside_rooms_and_journal() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("title.db");
    let hub = hub_on(Arc::new(Store::open(path.to_str()).unwrap()));
    let none = IdentityEdit::default();
    hub.create_loca_with("release-14", &master(&hub), None, None, &none)
        .expect("loca");
    hub.create_loca_with("harbor", &master(&hub), None, None, &none)
        .expect("loca");
    // Records keyed by the id: a seat's invite, members, Journal lines.
    let a = hub.admit_member("title-a", "agent", "master").unwrap();
    hub.invite_member_to_room(&a.id, "release-14", "master")
        .unwrap();

    let before = every_row(&path);
    let settings = hub
        .edit_identity(
            "release-14",
            &title("Release 15"),
            "Master",
            SenderType::User,
            |_| true,
        )
        .expect("titled");
    assert_eq!(settings.title.as_deref(), Some("Release 15"));
    let after = every_row(&path);

    let changed: Vec<&String> = before
        .keys()
        .chain(after.keys())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .filter(|table| before.get(*table) != after.get(*table))
        .collect();
    assert_eq!(changed, ["journal", "rooms"], "only these tables changed");
    // The rooms table still has the same ids, and only the titled loca's
    // settings changed.
    let ids = |rows: &[String]| {
        rows.iter()
            .map(|row| row.split(" | ").next().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&before["rooms"]), ids(&after["rooms"]));
    let moved: Vec<&String> = after["rooms"]
        .iter()
        .filter(|row| !before["rooms"].contains(row))
        .collect();
    assert_eq!(moved.len(), 1, "{moved:?}");
    assert!(moved[0].starts_with("Text(\"release-14\")"), "{moved:?}");
    assert!(moved[0].contains("Release 15"), "{moved:?}");
    // One Journal line added, none rewritten.
    assert_eq!(after["journal"].len(), before["journal"].len() + 1);
    assert!(before["journal"]
        .iter()
        .all(|row| after["journal"].contains(row)));
    assert_eq!(
        hub.journal("release-14").last().map(|e| e.text.clone()),
        Some("identity: title \"Release 15\"".into())
    );
}

#[test]
fn a_title_may_not_read_like_another_loca_the_caller_sees() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub_on(Arc::new(
        Store::open(dir.path().join("unique.db").to_str()).unwrap(),
    ));
    let none = IdentityEdit::default();
    for room in ["release-14", "harbor", "hidden-den"] {
        hub.create_loca_with(room, &master(&hub), None, None, &none)
            .expect("loca");
    }
    let sees = |room: &str| room != "hidden-den";
    let edit = |room: &str, text: &str| {
        hub.edit_identity(room, &title(text), "Master", SenderType::User, sees)
    };
    edit("harbor", "Dock").expect("a free title");

    // Another visible loca's id, or its title, without letter case: refused,
    // and nothing changes.
    for taken in ["HARBOR", "dock", " Dock "] {
        assert_eq!(edit("release-14", taken), Err(IdentityError::TitleTaken));
        assert_eq!(hub.settings("release-14").title, None, "{taken}");
    }
    // A loca the caller cannot see is not compared, so the answer does not
    // tell it exists.
    assert_eq!(
        edit("release-14", "Hidden-Den").unwrap().title.as_deref(),
        Some("Hidden-Den")
    );
    // Its own id is not another loca's.
    assert_eq!(
        edit("harbor", "Harbor").unwrap().title.as_deref(),
        Some("Harbor")
    );
    // Clearing is never refused.
    assert_eq!(edit("harbor", "").unwrap().title, None);
}
