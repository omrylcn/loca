//! A database written by the published v0.13.0 server, opened by this build's
//! store. The fixture was made by driving the real v0.13.0 binary through its
//! API (`crates/server/tests/fixtures/upgrade-v0.13.0/`, see its README); the
//! real-binary half of this check is `crates/server/tests/ws/upgrade.rs`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::super::{hashed_id, keys::fingerprint, secret_hash, Store, JOIN_BOOTSTRAP_WINDOW_MS};
use super::{files_containing, sql_values_containing};

const DUMP: &str = include_str!("../../../tests/fixtures/upgrade-v0.13.0/v0.13.0.sql");
const EXPECTED: &str = include_str!("../../../tests/fixtures/upgrade-v0.13.0/expected.json");

/// A new database file holding exactly what v0.13.0 wrote.
fn fixture_copy(directory: &Path) -> PathBuf {
    let path = directory.join("loca.db");
    Connection::open(&path)
        .expect("create")
        .execute_batch(DUMP)
        .expect("load the v0.13.0 dump");
    path
}

fn columns(conn: &Connection, table: &str) -> Vec<String> {
    conn.prepare(&format!("PRAGMA table_info(\"{table}\")"))
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// Every table and its columns, as v0.13.0 left them.
fn schema(conn: &Connection) -> BTreeMap<String, Vec<String>> {
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    tables
        .into_iter()
        .map(|table| {
            let columns = columns(conn, &table);
            (table, columns)
        })
        .collect()
}

/// The rows of `table` restricted to `columns`, each rendered with its type,
/// sorted, so two reads compare equal exactly when the values are equal.
fn rows(conn: &Connection, table: &str, columns: &[String]) -> Vec<String> {
    let list = columns
        .iter()
        .map(|column| format!("\"{column}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let mut statement = conn
        .prepare(&format!("SELECT {list} FROM \"{table}\""))
        .unwrap();
    let mut out = Vec::new();
    let mut found = statement.query([]).unwrap();
    while let Some(row) = found.next().unwrap() {
        let mut rendered = Vec::new();
        for i in 0..columns.len() {
            rendered.push(match row.get_ref(i).unwrap() {
                rusqlite::types::ValueRef::Null => "N".to_string(),
                rusqlite::types::ValueRef::Integer(value) => format!("I{value}"),
                rusqlite::types::ValueRef::Real(value) => format!("R{value}"),
                rusqlite::types::ValueRef::Text(text) => {
                    format!("T{}", String::from_utf8_lossy(text))
                }
                rusqlite::types::ValueRef::Blob(blob) => format!("B{blob:?}"),
            });
        }
        out.push(rendered.join("|"));
    }
    out.sort();
    out
}

fn digest(rows: &[String]) -> String {
    let mut hasher = Sha256::new();
    for row in rows {
        hasher.update(row.as_bytes());
        hasher.update(b"\n");
    }
    crate::store::hex_bytes(&hasher.finalize())
}

/// One content digest per table over every column it has now.
fn table_digests(path: &Path) -> BTreeMap<String, String> {
    let conn = Connection::open(path).unwrap();
    schema(&conn)
        .into_iter()
        .map(|(table, columns)| {
            let digest = digest(&rows(&conn, &table, &columns));
            (table, digest)
        })
        .collect()
}

fn text(conn: &Connection, sql: &str, param: &str) -> String {
    conn.query_row(sql, [param], |row| row.get(0))
        .unwrap_or_else(|error| panic!("{sql} [{param}]: {error}"))
}

/// The approval time v0.13.0 stored for fx-late, the request that was
/// approved and collected but never acknowledged.
fn late_approval(conn: &Connection) -> u64 {
    conn.query_row(
        "SELECT decided_at FROM join_requests WHERE name = 'fx-late'",
        [],
        |row| row.get(0),
    )
    .unwrap()
}

/// v0.13.0's own answer to `path`, as the generator recorded it.
fn recorded(path: &str) -> Value {
    let expected: Value = serde_json::from_str(EXPECTED).unwrap();
    expected["views"]
        .as_array()
        .unwrap()
        .iter()
        .find(|view| view["path"] == path)
        .unwrap_or_else(|| panic!("no recorded view {path}"))["body"]
        .clone()
}

/// One membership as v0.13.0 stored it, in clear text.
struct OldMember {
    key: String,
    name: String,
    kind: String,
    joined_at: u64,
    admitted_by: String,
    revoked_at: Option<u64>,
}

/// One invite as v0.13.0 stored it, in clear text, with its member's key.
struct OldInvite {
    invite: String,
    room: String,
    member: String,
    name: String,
    kind: String,
    issued_at: u64,
    issued_by: String,
    revoked_at: Option<u64>,
}

fn old_members(conn: &Connection) -> Vec<OldMember> {
    conn.prepare(
        "SELECT token, name, kind, joined_at, admitted_by, revoked_at FROM members ORDER BY name",
    )
    .unwrap()
    .query_map([], |row| {
        Ok(OldMember {
            key: row.get(0)?,
            name: row.get(1)?,
            kind: row.get(2)?,
            joined_at: row.get(3)?,
            admitted_by: row.get(4)?,
            revoked_at: row.get(5)?,
        })
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn old_invites(conn: &Connection) -> Vec<OldInvite> {
    conn.prepare(
        "SELECT token, room, member, name, kind, issued_at, issued_by, revoked_at
         FROM invites ORDER BY issued_at",
    )
    .unwrap()
    .query_map([], |row| {
        Ok(OldInvite {
            invite: row.get(0)?,
            room: row.get(1)?,
            member: row.get(2)?,
            name: row.get(3)?,
            kind: row.get(4)?,
            issued_at: row.get(5)?,
            issued_by: row.get(6)?,
            revoked_at: row.get(7)?,
        })
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn mbid(key: &str) -> String {
    hashed_id("mbid_", &format!("member:{key}"))
}

/// The upgrade opens a real v0.13.0 database, changes only what its
/// migrations are for, keeps every identity with the identifiers v0.13.0
/// gave it, leaves no Smaster code, browser session token, membership key or
/// invite anywhere in the files, and a second open changes nothing.
#[test]
fn a_database_written_by_v0_13_0_opens_keeps_every_record_and_reopens_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let path = fixture_copy(directory.path());
    let p = path.to_str().unwrap();

    let before = Connection::open(&path).unwrap();
    let old_schema = schema(&before);
    let old_rows: BTreeMap<String, Vec<String>> = old_schema
        .iter()
        .map(|(table, columns)| (table.clone(), rows(&before, table, columns)))
        .collect();
    let smasters: Vec<(String, String, u64, Option<u64>)> = before
        .prepare("SELECT token, name, issued_at, revoked_at FROM smasters ORDER BY name")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let sessions: Vec<(String, String, String, u64)> = before
        .prepare("SELECT token, name, kind, expires_at FROM admin_sessions ORDER BY name")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        smasters.len(),
        3,
        "fixture: a used, an unused and a revoked Smaster"
    );
    assert_eq!(
        sessions.len(),
        2,
        "fixture: the Master's and fx-deputy's browser sessions"
    );
    let deputy_principal = text(
        &before,
        "SELECT id FROM principals WHERE display_name = ?1",
        "fx-deputy",
    );
    let approved_at = late_approval(&before);
    let members = old_members(&before);
    let invites = old_invites(&before);
    let suspended: Vec<(String, u64, String)> = before
        .prepare("SELECT member_token, suspended_at, reason FROM member_suspensions")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        members.len(),
        7,
        "fixture: ada, bea, bot, gone, paused, and the two approved join requests"
    );
    assert_eq!(
        invites.len(),
        7,
        "fixture: seven invites, two of them ended"
    );
    assert_eq!(suspended.len(), 1, "fixture: fx-paused");
    let late_key = text(
        &before,
        "SELECT token FROM members WHERE name = ?1",
        "fx-late",
    );
    drop(before);

    // Opened more than 24 hours after fx-late's approval, as every run after
    // the day the fixture was written is.
    let now = approved_at + JOIN_BOOTSTRAP_WINDOW_MS + 60_000;
    drop(Store::open_at(Some(p), now).expect("this build opens a v0.13.0 database"));

    let after = Connection::open(&path).unwrap();
    // Every table v0.13.0 had is still there with every column it had, and
    // apart from the three tables the upgrade exists to change, every value
    // in those columns is exactly what v0.13.0 wrote. The registries keyed
    // by a secret are rebuilt (checked below, row by row).
    let keyed_by_a_secret = [
        "smasters",
        "admin_sessions",
        "members",
        "invites",
        "member_suspensions",
    ];
    let rebuilt = ["join_requests"];
    for (table, old_columns) in &old_schema {
        if keyed_by_a_secret.contains(&table.as_str()) {
            continue;
        }
        let new_columns = columns(&after, table);
        for column in old_columns {
            assert!(
                new_columns.contains(column),
                "{table}.{column} disappeared in the upgrade"
            );
        }
        if table == "principal_sessions" {
            // Sessions are not records: this build ends each at its expiry
            // and removes ended ones at open. v0.13.0's member sessions (12
            // hours) are over by `now`; what must remain, untouched, is every
            // session that is still live then.
            let position = |name: &str| {
                old_columns
                    .iter()
                    .position(|column| column == name)
                    .unwrap_or_else(|| panic!("principal_sessions.{name}"))
            };
            let (expires, revoked) = (position("expires_at"), position("revoked_at"));
            let live: Vec<String> = old_rows[table]
                .iter()
                .filter(|row| {
                    let cells: Vec<&str> = row.split('|').collect();
                    let expires_at: u64 = cells[expires]
                        .strip_prefix('I')
                        .and_then(|value| value.parse().ok())
                        .unwrap_or(0);
                    cells[revoked] == "N" && expires_at > now
                })
                .cloned()
                .collect();
            assert!(
                !live.is_empty() && live.len() < old_rows[table].len(),
                "fixture: some sessions are over by the open and some are not"
            );
            assert_eq!(
                rows(&after, table, old_columns),
                live,
                "the upgrade must keep exactly the sessions still live at the open"
            );
            continue;
        }
        if !rebuilt.contains(&table.as_str()) {
            assert_eq!(
                rows(&after, table, old_columns),
                old_rows[table],
                "the upgrade changed {table}"
            );
        }
    }

    // The Smaster registry keeps each Smaster as the same principal, with
    // the same credential and the management id v0.13.0 showed, holding only
    // the hash v0.13.0 itself kept in `credentials`.
    let listed = recorded("/smasters");
    assert_eq!(
        after
            .query_row("SELECT count(*) FROM smasters", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        3
    );
    for (code, name, issued_at, revoked_at) in &smasters {
        let principal = text(
            &after,
            "SELECT id FROM principals WHERE display_name = ?1",
            name,
        );
        let (credential, hash): (String, String) = after
            .query_row(
                "SELECT id, secret_hash FROM credentials
                 WHERE principal_id = ?1 AND legacy_source LIKE 'smaster:%'",
                [&principal],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(hash, secret_hash(code), "{name}: v0.13.0's credential hash");
        let row: (String, String, String, String, u64, Option<u64>) = after
            .query_row(
                "SELECT management_id, secret_hash, principal_id, credential_id, issued_at,
                        revoked_at
                 FROM smasters WHERE name = ?1",
                [name],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            (&row.1, &row.2, &row.3, row.4, row.5),
            (&hash, &principal, &credential, *issued_at, *revoked_at),
            "{name}"
        );
        if let Some(shown) = listed
            .as_array()
            .unwrap()
            .iter()
            .find(|smaster| smaster["name"] == name.as_str())
        {
            assert_eq!(
                shown["id"],
                row.0.as_str(),
                "{name}: the smid_ v0.13.0 showed"
            );
        } else {
            assert!(revoked_at.is_some(), "v0.13.0 listed every live Smaster");
        }
    }

    // A browser session still live at the open is the same session, keyed by
    // its hash. One that was over by then is removed at the open, as every
    // ended session is; the fixture has one of each.
    let mut kept = 0;
    for (token, name, kind, expires_at) in &sessions {
        let row: Option<(String, String, u64)> = after
            .query_row(
                "SELECT name, kind, expires_at FROM admin_sessions WHERE secret_hash = ?1",
                [secret_hash(token)],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .unwrap();
        if *expires_at > now {
            kept += 1;
            assert_eq!(
                row,
                Some((name.clone(), kind.clone(), *expires_at)),
                "{name}'s live session is gone or changed"
            );
        } else {
            assert_eq!(row, None, "{name}'s session ended before the open");
        }
    }
    assert_eq!(
        (kept, sessions.len()),
        (1, 2),
        "fixture: one browser session outlives the open and one does not"
    );

    // Join requests: the 24-hour rule clears fx-late's key copy and marks it
    // expired at the time of the open; nothing else in the table changes.
    let requests: BTreeMap<String, (String, Option<String>, Option<u64>)> = after
        .prepare("SELECT name, status, mb_token, bootstrap_expired_at FROM join_requests")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get(0)?, (row.get(1)?, row.get(2)?, row.get(3)?)))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(requests["fx-wait"], ("pending".into(), None, None));
    assert_eq!(requests["fx-late"], ("approved".into(), None, Some(now)));
    assert_eq!(requests["fx-done"], ("approved".into(), None, None));
    assert_eq!(requests["fx-no"], ("denied".into(), None, None));
    let mut old_requests = old_rows["join_requests"].clone();
    for row in old_requests.iter_mut() {
        *row = row.replace(&format!("|T{late_key}|"), "|N|");
    }
    old_requests.sort();
    assert_eq!(
        rows(&after, "join_requests", &old_schema["join_requests"]),
        old_requests,
        "only fx-late's key copy is cleared"
    );

    // Only the Smaster's socket post, whose operation scope names its
    // principal, is attributed; no row is attributed by name.
    let attributed: Vec<(String, String)> = after
        .prepare("SELECT text, sender_principal FROM messages WHERE sender_principal IS NOT NULL")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        attributed,
        vec![("fx socket word from deputy".to_string(), deputy_principal)]
    );

    // Memberships: each is the row v0.13.0 had, under the management id
    // v0.13.0 showed, naming the principal and the credential v0.13.0 bound
    // to its key (`principals` and `credentials` are unchanged, above), with
    // only the key's hash and fingerprint in place of the key.
    let residents = recorded("/residents");
    for m in &members {
        let (credential, principal): (String, String) = after
            .query_row(
                "SELECT id, principal_id FROM credentials WHERE legacy_source = ?1",
                [format!("member:{}", secret_hash(&m.key))],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or_else(|error| panic!("{}: v0.13.0's binding: {error}", m.name));
        let row: (
            String,
            String,
            String,
            String,
            String,
            u64,
            String,
            Option<u64>,
        ) = after
            .query_row(
                "SELECT secret_hash, principal_id, credential_id, fingerprint, kind, joined_at,
                        admitted_by, revoked_at
                 FROM members WHERE id = ?1 AND name = ?2",
                [mbid(&m.key), m.name.clone()],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                },
            )
            .unwrap_or_else(|error| panic!("{} under its mbid_: {error}", m.name));
        assert_eq!(
            row,
            (
                secret_hash(&m.key),
                principal,
                credential,
                fingerprint(&m.key),
                m.kind.clone(),
                m.joined_at,
                m.admitted_by.clone(),
                m.revoked_at
            ),
            "{}",
            m.name
        );
        if let Some(shown) = residents
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == m.name.as_str())
        {
            assert_eq!(shown["membership_id"], mbid(&m.key).as_str(), "{}", m.name);
        }
    }
    assert_eq!(
        after
            .query_row("SELECT count(*) FROM members", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        members.len() as i64
    );
    // Invites: the same, under the ivid_ v0.13.0 showed, naming their member
    // by its mbid_. They were issued more than 24 hours before this open, so
    // none is given a delivery copy.
    let mut shown_invites: Vec<Value> = Vec::new();
    for room in ["fx-team", "fx-ops"] {
        shown_invites.extend(
            recorded(&format!("/rooms/{room}/invites"))
                .as_array()
                .unwrap()
                .iter()
                .cloned(),
        );
    }
    for i in &invites {
        let id = hashed_id("ivid_", &format!("invite:{}", i.invite));
        let row: (
            String,
            String,
            String,
            String,
            String,
            String,
            u64,
            String,
            Option<u64>,
        ) = after
            .query_row(
                "SELECT secret_hash, fingerprint, room, member_id, name, kind, issued_at,
                            issued_by, revoked_at
                     FROM invites WHERE id = ?1",
                [&id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                    ))
                },
            )
            .unwrap_or_else(|error| panic!("{} in {}: {error}", i.name, i.room));
        assert_eq!(
            row,
            (
                secret_hash(&i.invite),
                fingerprint(&i.invite),
                i.room.clone(),
                mbid(&i.member),
                i.name.clone(),
                i.kind.clone(),
                i.issued_at,
                i.issued_by.clone(),
                i.revoked_at
            ),
            "{} in {}",
            i.name,
            i.room
        );
        let copy: (Option<String>, Option<u64>) = after
            .query_row(
                "SELECT delivery_token, first_used_at FROM invites WHERE id = ?1",
                [&id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(copy, (None, None), "{} in {}", i.name, i.room);
        match shown_invites
            .iter()
            .find(|shown| shown["id"] == id.as_str())
        {
            Some(shown) => assert_eq!(shown["member_id"], mbid(&i.member).as_str()),
            None => assert!(i.revoked_at.is_some(), "v0.13.0 listed every live invite"),
        }
    }
    let suspensions: Vec<(String, u64, String)> = after
        .prepare("SELECT member_id, suspended_at, reason FROM member_suspensions")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        suspensions,
        suspended
            .iter()
            .map(|(key, at, reason)| (mbid(key), *at, reason.clone()))
            .collect::<Vec<_>>()
    );

    // No Smaster code, browser session token, membership key or invite is
    // left in any column or anywhere in the database file and its WAL: fx-
    // late's join-request copy was cleared at its deadline, fx-done's at its
    // acknowledgement.
    let secrets: Vec<&String> = smasters
        .iter()
        .map(|row| &row.0)
        .chain(sessions.iter().map(|row| &row.0))
        .chain(members.iter().map(|m| &m.key))
        .chain(invites.iter().map(|i| &i.invite))
        .collect();
    for secret in &secrets {
        assert_eq!(sql_values_containing(&after, secret), Vec::<String>::new());
    }
    drop(after);
    for secret in &secrets {
        assert_eq!(files_containing(&path, secret), Vec::<String>::new());
    }

    // A second open, an hour later, changes no value in any table.
    let first = table_digests(&path);
    drop(Store::open_at(Some(p), now + 60 * 60 * 1000).expect("second open"));
    assert_eq!(
        table_digests(&path),
        first,
        "reopening changed the database"
    );
}

/// The key copy of a request v0.13.0 approved lasts 24 hours from the
/// approval time v0.13.0 stored, whenever this build first opens the file.
#[test]
fn a_v0_13_0_approval_keeps_its_key_copy_until_24_hours_after_the_stored_approval() {
    for (offset, cleared) in [(-1i64, false), (0, true)] {
        let directory = tempfile::tempdir().unwrap();
        let path = fixture_copy(directory.path());
        let (approved_at, key) = {
            let conn = Connection::open(&path).unwrap();
            (
                late_approval(&conn),
                text(
                    &conn,
                    "SELECT mb_token FROM join_requests WHERE name = ?1",
                    "fx-late",
                ),
            )
        };
        let now = (approved_at + JOIN_BOOTSTRAP_WINDOW_MS).saturating_add_signed(offset);
        drop(Store::open_at(path.to_str(), now).unwrap());
        let conn = Connection::open(&path).unwrap();
        let (copy, expired): (Option<String>, Option<u64>) = conn
            .query_row(
                "SELECT mb_token, bootstrap_expired_at FROM join_requests WHERE name = 'fx-late'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        if cleared {
            assert_eq!((copy, expired), (None, Some(now)), "at the deadline");
        } else {
            assert_eq!((copy, expired), (Some(key), None), "a millisecond before");
        }
    }
}
