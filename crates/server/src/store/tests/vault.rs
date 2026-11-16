//! The Vault's rules at the store: one write is a revision, the settings that
//! name it and its Journal line together; revisions survive a reopen; a
//! database from before the table opens with every loca as it was.

use protocol::{ChatMode, JournalEntry, RoomSettings, SenderType};
use rusqlite::Connection;

use super::super::{Store, VaultRevision};

/// A clock shaped like a real one (milliseconds since the epoch).
const T0: u64 = 1_800_000_000_000;

const V0_13_0_DUMP: &str = include_str!("../../../tests/fixtures/upgrade-v0.13.0/v0.13.0.sql");

fn write(store: &Store, room: &str, rev: u64, body: &str) -> rusqlite::Result<()> {
    let settings = RoomSettings {
        vault_rev: Some(rev),
        ..RoomSettings::default()
    };
    let entry = JournalEntry {
        id: rev,
        room: room.into(),
        by: "opal".into(),
        by_type: SenderType::User,
        text: format!("vault: rules updated (revision {rev})"),
        at: T0 + rev,
    };
    store.save_vault_revision(
        &VaultRevision {
            room,
            rev,
            body,
            by: "opal",
            by_type: SenderType::User,
            by_principal: Some("pr_opal"),
            at: T0 + rev,
        },
        &ChatMode::Free,
        &settings,
        &entry,
    )
}

#[test]
fn revisions_are_kept_across_a_reopen_with_the_settings_and_journal_that_name_them() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("vault.db");
    {
        let store = Store::open_at(path.to_str(), T0).unwrap();
        assert_eq!(store.vault("atelier").unwrap(), None);
        write(&store, "atelier", 1, "first").unwrap();
        write(&store, "atelier", 2, "second\nline").unwrap();
        write(&store, "other", 1, "elsewhere").unwrap();
        // The same revision twice is refused, and nothing of it lands.
        assert!(write(&store, "atelier", 2, "again").is_err());
    }
    let store = Store::open_at(path.to_str(), T0 + 10).unwrap();
    let doc = store.vault("atelier").unwrap().unwrap();
    assert_eq!((doc.rev, doc.body.as_str()), (2, "second\nline"));
    assert_eq!(doc.updated_by.as_deref(), Some("opal"));
    assert_eq!(doc.updated_at, Some(T0 + 2));
    let history: Vec<u64> = store
        .vault_history("atelier", 50)
        .unwrap()
        .iter()
        .map(|doc| doc.rev)
        .collect();
    assert_eq!(history, [2, 1]);
    assert_eq!(store.vault_history("atelier", 1).unwrap().len(), 1);
    let rooms = store.load().unwrap();
    let atelier = rooms.iter().find(|r| r.room == "atelier").unwrap();
    assert_eq!(atelier.settings.vault_rev, Some(2));
    let journal = store.load_journal("atelier").unwrap();
    assert_eq!(
        journal.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(),
        [
            "vault: rules updated (revision 1)",
            "vault: rules updated (revision 2)"
        ]
    );
    let principal: Option<String> = store
        .conn()
        .unwrap()
        .query_row(
            "SELECT updated_by_principal FROM loca_vault WHERE room = 'atelier' AND rev = 2",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(principal.as_deref(), Some("pr_opal"));
}

#[test]
fn a_database_from_before_the_vault_opens_with_every_loca_as_it_was() {
    // The published v0.13.0 database, and one shaped like 0.14.0's: the
    // current schema without the table.
    let directory = tempfile::tempdir().unwrap();
    let old = directory.path().join("v0_13_0.db");
    Connection::open(&old)
        .unwrap()
        .execute_batch(V0_13_0_DUMP)
        .unwrap();
    let v0_14 = directory.path().join("v0_14_0.db");
    {
        let store = Store::open_at(v0_14.to_str(), T0).unwrap();
        store
            .save_room("atelier", &ChatMode::Free, &RoomSettings::default())
            .unwrap();
    }
    Connection::open(&v0_14)
        .unwrap()
        .execute_batch("DROP TABLE loca_vault;")
        .unwrap();

    for path in [old, v0_14] {
        let rooms_before: Vec<(String, String)> = Connection::open(&path)
            .unwrap()
            .prepare("SELECT room, settings FROM rooms ORDER BY room")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(!rooms_before.is_empty(), "{path:?} holds locas");
        let store = Store::open_at(path.to_str(), T0).unwrap();
        let rooms_after: Vec<(String, String)> = store
            .conn()
            .unwrap()
            .prepare("SELECT room, settings FROM rooms ORDER BY room")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(rooms_after, rooms_before, "{path:?}: settings untouched");
        for (room, _) in &rooms_before {
            assert_eq!(store.vault(room).unwrap(), None, "{room}");
            let snapshot = store.load().unwrap();
            let loaded = snapshot.iter().find(|r| &r.room == room);
            assert!(
                loaded.is_none_or(|r| r.settings.vault_rev.is_none()),
                "{room}"
            );
        }
        // The new table works on the upgraded database.
        let room = &rooms_before[0].0;
        write(&store, room, 1, "rules").unwrap();
        assert_eq!(store.vault(room).unwrap().unwrap().body, "rules");
    }
}
