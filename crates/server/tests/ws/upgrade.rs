//! A database written by the published v0.13.0 server, served by this build's
//! real binary. The fixture was made by driving the v0.13.0 binary through its
//! API (`tests/fixtures/upgrade-v0.13.0/README.md`); `expected.json` holds
//! that server's own answers, which this build must still give. The store
//! half of this check is `src/store/tests/upgrade_v0_13_0.rs`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::*;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/upgrade-v0.13.0"
);

/// The generator's synthetic ADMIN_TOKEN. The Master's principal id is
/// derived from it, so the upgraded server must be started with the same one.
const ROOT: &str = "fixture-v0130-root";

const DAY_MS: u64 = 24 * 60 * 60 * 1000;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

struct Fixture {
    _directory: tempfile::TempDir,
    db: PathBuf,
    expected: Value,
    /// Every credential the views name, read from what v0.13.0 stored.
    credentials: HashMap<String, String>,
    /// The Smaster codes, browser session tokens, membership keys and invites
    /// v0.13.0 kept in clear text, which the upgrade must leave nowhere in
    /// the database.
    hashed_away: Vec<String>,
    principals: HashMap<String, String>,
}

fn pairs(conn: &rusqlite::Connection, sql: &str) -> Vec<(String, String)> {
    conn.prepare(sql)
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// A copy of the fixture, as a database file and its blob store, and the
/// credentials v0.13.0 stored in it.
///
/// Two time values are moved so that the outcome does not depend on the day
/// the test runs; nothing else is edited. The Master's browser session was
/// paired for a year, so its expiry (kept in two tables) is moved to at least
/// 30 days from now. fx-late's approval time is moved to at least 25 hours
/// ago, which is where it is on any day after the fixture's first, so the
/// 24-hour rule applies to it here as it would to a database found later.
fn load() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("loca.db");
    let expected: Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{FIXTURE}/expected.json")).unwrap())
            .unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch(&std::fs::read_to_string(format!("{FIXTURE}/v0.13.0.sql")).unwrap())
        .unwrap();
    copy_tree(
        &Path::new(FIXTURE).join("attachments"),
        &directory.path().join("attachments"),
    );

    let mut credentials = HashMap::from([("root".to_string(), ROOT.to_string())]);
    let keys = pairs(&conn, "SELECT name, token FROM members");
    for (name, key) in &keys {
        credentials.insert(format!("member:{name}"), key.clone());
    }
    let codes = pairs(&conn, "SELECT name, token FROM smasters");
    for (name, code) in &codes {
        credentials.insert(format!("smaster:{name}"), code.clone());
    }
    let invites = pairs(&conn, "SELECT room || ':' || name, token FROM invites");
    for (seat, invite) in &invites {
        credentials.insert(format!("davet:{seat}"), invite.clone());
    }
    for (name, handle) in expected["join_requests"].as_object().unwrap() {
        credentials.insert(
            format!("join:{name}"),
            handle["secret"].as_str().unwrap().to_string(),
        );
    }
    let sessions = pairs(&conn, "SELECT name, token FROM admin_sessions");
    let master_session = sessions
        .iter()
        .find(|(name, _)| name == "Master")
        .unwrap()
        .1
        .clone();
    credentials.insert("master-session".into(), master_session.clone());
    let hashed_away = codes
        .iter()
        .chain(sessions.iter())
        .chain(keys.iter())
        .chain(invites.iter())
        .map(|(_, secret)| secret.clone())
        .collect();
    let principals = pairs(&conn, "SELECT display_name, id FROM principals")
        .into_iter()
        .collect();

    let now = now_ms();
    let session_hash = hex(&Sha256::digest(master_session.as_bytes()));
    for (sql, key) in [
        (
            "UPDATE admin_sessions SET expires_at = max(expires_at, ?1) WHERE token = ?2",
            &master_session,
        ),
        (
            "UPDATE principal_sessions SET expires_at = max(expires_at, ?1) WHERE id = ?2",
            &session_hash,
        ),
    ] {
        assert_eq!(
            conn.execute(sql, rusqlite::params![now + 30 * DAY_MS, key])
                .unwrap(),
            1,
            "{sql}"
        );
    }
    // The snapshot was written on 2026-11-14. fx-spare's Smaster code was
    // never used, and an unused code stops working seven days after it was
    // issued (`Hub::SMASTER_USE_MS`, the background sweep): without this the
    // test began to fail a week after the snapshot. Its issue time moves to a
    // day ago, as the sessions' expiry moves above; no answer shows it.
    assert_eq!(
        conn.execute(
            "UPDATE smasters SET issued_at = max(issued_at, ?1)
             WHERE name = 'fx-spare' AND revoked_at IS NULL",
            [now - DAY_MS],
        )
        .unwrap(),
        1
    );
    assert_eq!(
        conn.execute(
            "UPDATE join_requests SET decided_at = min(decided_at, ?1)
             WHERE name = 'fx-late' AND status = 'approved' AND mb_token IS NOT NULL",
            [now - 25 * 60 * 60 * 1000],
        )
        .unwrap(),
        1
    );
    drop(conn);
    Fixture {
        _directory: directory,
        db,
        expected,
        credentials,
        hashed_away,
        principals,
    }
}

/// Every field v0.13.0 answered is still answered, with the same value.
/// Fields this build adds are allowed. Several lists come out of hash maps,
/// so a list of plain values is compared as a set, and a list of records
/// keyed by a text `id` or by `name` is matched by that key (the extension
/// catalog may hold entries v0.13.0 did not have). A list of
/// records with a numeric `id` (messages, Journal lines, tasks, ...) is a
/// sequence and is compared in order, as is any other list.
fn assert_still_answers(expected: &Value, actual: &Value, at: &str, ignored: &[&str]) {
    match (expected, actual) {
        (Value::Object(expected), Value::Object(actual)) => {
            for (key, value) in expected {
                if ignored.contains(&key.as_str()) {
                    continue;
                }
                let now = actual
                    .get(key)
                    .unwrap_or_else(|| panic!("{at}.{key} is no longer answered"));
                assert_still_answers(value, now, &format!("{at}.{key}"), ignored);
            }
        }
        (Value::Array(expected), Value::Array(actual)) => {
            // The extension catalog grows (the Vault came after v0.13.0):
            // every entry v0.13.0 listed must still be there, matched by id
            // below; a new one is allowed. Every other list keeps its length.
            if at.ends_with(".catalog") {
                assert!(expected.len() <= actual.len(), "{at}: {actual:?}");
            } else {
                assert_eq!(expected.len(), actual.len(), "{at}: {actual:?}");
            }
            if expected
                .iter()
                .all(|value| !value.is_object() && !value.is_array())
            {
                let sorted = |values: &Vec<Value>| {
                    let mut texts: Vec<String> = values.iter().map(Value::to_string).collect();
                    texts.sort();
                    texts
                };
                assert_eq!(sorted(expected), sorted(actual), "{at}");
            } else if let Some(key) = ["id", "name"]
                .into_iter()
                .find(|key| expected.iter().all(|value| value[*key].is_string()))
            {
                for expected in expected {
                    let id = &expected[key];
                    let actual = actual
                        .iter()
                        .find(|value| &value[key] == id)
                        .unwrap_or_else(|| panic!("{at}: {key} {id} is no longer listed"));
                    assert_still_answers(expected, actual, &format!("{at}[{key}={id}]"), ignored);
                }
            } else {
                for (i, (expected, actual)) in expected.iter().zip(actual).enumerate() {
                    assert_still_answers(expected, actual, &format!("{at}[{i}]"), ignored);
                }
            }
        }
        _ => assert_eq!(expected, actual, "{at}"),
    }
}

async fn ask(base: &str, path: &str, header: &str, credential: &str) -> (u16, Value) {
    let response = reqwest::Client::new()
        .get(format!("{base}{path}"))
        .header(header, credential)
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = response.bytes().await.unwrap();
    let body = if path.contains("/attachments/") {
        if status == 200 {
            serde_json::json!({ "sha256": hex(&Sha256::digest(&bytes)) })
        } else {
            Value::Null
        }
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, body)
}

async fn post_with(base: &str, path: &str, header: &str, credential: &str) -> u16 {
    reqwest::Client::new()
        .post(format!("{base}{path}"))
        .header(header, credential)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

/// Ask every view v0.13.0 answered and compare. The views whose answers the
/// newer rules change are fx-late's status, the suspended fx-paused's own
/// key, and fx-team's tasks (the Loca Board's now), each checked on its own.
async fn replay(base: &str, fixture: &Fixture) {
    let late = fixture.expected["join_requests"]["fx-late"]["id"]
        .as_str()
        .unwrap();
    for view in fixture.expected["views"].as_array().unwrap() {
        let path = view["path"].as_str().unwrap();
        let header = view["header"].as_str().unwrap();
        let credential = view["credential"].as_str().unwrap();
        let (status, body) = ask(base, path, header, &fixture.credentials[credential]).await;
        let at = format!("GET {path} as {credential}");
        if path == format!("/join-requests/{late}") {
            assert_eq!(status, 200, "{at}");
            assert_eq!(body["status"], "approved", "{at}");
            assert_eq!(body["bootstrap_ready"], false, "{at}");
            assert_eq!(body["bootstrap_expired"], true, "{at}");
            continue;
        }
        if path.ends_with("/tasks") {
            // The tasks are the Loca Board's now, which a loca from v0.13.0
            // does not have on: they are kept and refused, and come back
            // as they were when it is turned on (checked after the restarts).
            assert_eq!(view["status"], 200, "{at}: v0.13.0's answer");
            // (`ask` keeps JSON bodies only; the refusal's words are held
            // in loca_board.rs.)
            assert_eq!(status, 403, "{at}: {body}");
            continue;
        }
        if credential == "member:fx-paused" && header == "x-room-token" {
            // A suspended member's own key is now told so; v0.13.0 answered
            // it as any refused key.
            assert_eq!(view["status"], 401, "{at}: v0.13.0's answer");
            assert_eq!(status, 403, "{at}: {body}");
            continue;
        }
        assert_eq!(status, view["status"], "{at}: {body}");
        // A sign-in records its use; the Master session's expiry was moved
        // by `load`.
        // Focus's summary no longer mentions the tasks it gave to the Loca
        // Board; the catalog's summaries are what people read, not data.
        let ignored: &[&str] = if credential == "master-session" {
            &["last_used_at", "expires_at", "summary"]
        } else {
            &["last_used_at", "summary"]
        };
        assert_still_answers(&view["body"], &body, &at, ignored);
    }
}

#[tokio::test]
async fn a_database_written_by_the_published_v0_13_0_gives_the_same_answers_after_the_upgrade() {
    let fixture = load();
    let db = fixture.db.to_str().unwrap().to_string();
    let jr = |name: &str| {
        fixture.expected["join_requests"][name]["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let secret = |name: &str| fixture.credentials[&format!("join:{name}")].clone();

    // The first start migrates; the second must find nothing to change and
    // answer the same.
    for start in ["first", "second"] {
        let (port, server) = spawn_server_env(
            ROOT,
            &[("DB_PATH", db.clone()), ("REQUIRE_INVITE", "1".into())],
        )
        .await;
        let base = format!("http://127.0.0.1:{port}");
        replay(&base, &fixture).await;

        // fx-spare's code was never used under v0.13.0 (the generator never
        // presents it); it signs in now, as the same principal.
        let (status, profile) = ask(
            &base,
            "/profile",
            "x-admin-token",
            &fixture.credentials["smaster:fx-spare"],
        )
        .await;
        assert_eq!(status, 200, "{start} start: {profile}");
        assert_eq!(
            profile["principal"]["id"],
            fixture.principals["fx-spare"].as_str()
        );

        // The approval fx-late collected but never acknowledged: its copy
        // is past the 24-hour limit, so collecting it again is refused, and
        // its key keeps working (the `/profile` view above). A late
        // acknowledgement is still accepted (checked after the restarts).
        assert_eq!(
            post_with(
                &base,
                &format!("/join-requests/{}/bootstrap", jr("fx-late")),
                "x-join-secret",
                &secret("fx-late"),
            )
            .await,
            410,
            "{start} start"
        );
        // The acknowledged, the denied and the pending request hand out
        // nothing.
        for name in ["fx-done", "fx-no", "fx-wait"] {
            let status = post_with(
                &base,
                &format!("/join-requests/{}/bootstrap", jr(name)),
                "x-join-secret",
                &secret(name),
            )
            .await;
            assert!(
                !(200..300).contains(&status),
                "{start} start: {name} collected a key ({status})"
            );
        }
        drop(server);
        wait_for_server_shutdown(port).await;
    }

    let (port, _server) = spawn_server_env(
        ROOT,
        &[("DB_PATH", db.clone()), ("REQUIRE_INVITE", "1".into())],
    )
    .await;
    assert_eq!(
        post_with(
            &format!("http://127.0.0.1:{port}"),
            &format!("/join-requests/{}/ack", jr("fx-late")),
            "x-join-secret",
            &secret("fx-late"),
        )
        .await,
        200,
        "a late acknowledgement is accepted"
    );
    // The tasks v0.13.0 stored come back as they were once the Loca Board is
    // turned on (fx-team has a Lead: its goal is active).
    let base = format!("http://127.0.0.1:{port}");
    let on = reqwest::Client::new()
        .put(format!("{base}/rooms/fx-team/extensions/board"))
        .header("x-admin-token", ROOT)
        .send()
        .await
        .unwrap();
    assert_eq!(on.status(), 200, "{}", on.text().await.unwrap());
    let view = fixture.expected["views"]
        .as_array()
        .unwrap()
        .iter()
        .find(|view| view["path"] == "/rooms/fx-team/tasks")
        .expect("v0.13.0 listed fx-team's tasks");
    let credential = view["credential"].as_str().unwrap();
    let (status, body) = ask(
        &base,
        "/rooms/fx-team/tasks",
        view["header"].as_str().unwrap(),
        &fixture.credentials[credential],
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_still_answers(&view["body"], &body, "fx-team's tasks", &["last_used_at"]);

    // No Smaster code, browser session token, membership key or invite
    // v0.13.0 stored is left in any column: the join-request copies of the
    // two approved members' keys are cleared (fx-late's at its deadline,
    // fx-done's at its acknowledgement). The one exception is a live invite's
    // bounded delivery copy (`invites.delivery_token`) while its 24 hours
    // from issue last: the upgrade gives one to every invite issued less than
    // 24 hours before it, as a call made while its member was away.
    let conn = rusqlite::Connection::open(&db).unwrap();
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let now = now_ms();
    for table in tables {
        let mut statement = conn.prepare(&format!("SELECT * FROM \"{table}\"")).unwrap();
        let names: Vec<String> = statement.column_names().map(str::to_string).collect();
        let issued_at = names.iter().position(|name| name == "issued_at");
        let mut rows = statement.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            for (i, column) in names.iter().enumerate() {
                if table == "invites" && column == "delivery_token" {
                    let issued: i64 = row.get(issued_at.unwrap()).unwrap();
                    if row.get::<_, Option<String>>(i).unwrap().is_some() {
                        assert!(
                            now < issued as u64 + DAY_MS,
                            "an invite's delivery copy outlived its 24 hours"
                        );
                    }
                    continue;
                }
                let value = match row.get_ref(i).unwrap() {
                    rusqlite::types::ValueRef::Text(bytes)
                    | rusqlite::types::ValueRef::Blob(bytes) => {
                        String::from_utf8_lossy(bytes).into_owned()
                    }
                    _ => continue,
                };
                for secret in &fixture.hashed_away {
                    assert!(
                        !value.contains(secret.as_str()),
                        "{table}.{column} still holds a secret v0.13.0 stored"
                    );
                }
            }
        }
    }
}

/// v0.13.0 remembered an operation sent over REST under a key built from that
/// request (`agent:fx-bot`, `user:fx-ada`: a session taken with the member's
/// own key), and recorded no writer. A display name does not prove who wrote
/// such a row (a namesake, a name admitted again, a renamed rank), so after
/// the upgrade a re-send of it is neither answered with the stored message
/// nor posted again: REST says so with 409, a WebSocket `send` is dropped,
/// and the loca still holds the message exactly once.
#[tokio::test]
async fn an_operation_v0_13_0_stored_under_a_name_is_never_posted_again() {
    let fixture = load();
    let conn = rusqlite::Connection::open(&fixture.db).unwrap();
    let stored: HashMap<String, (u64, String)> = conn
        .prepare("SELECT op_id, id, text FROM messages WHERE op_id IN ('fx-op-bot-0001', 'fx-op-ada-0001')")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, (row.get(1)?, row.get(2)?))))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(stored.len(), 2, "the fixture's REST operations");
    drop(conn);
    let (port, _server) = spawn_server_env(
        ROOT,
        &[
            ("DB_PATH", fixture.db.to_str().unwrap().to_string()),
            ("REQUIRE_INVITE", "1".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    for (name, kind, op) in [
        ("fx-bot", "agent", "fx-op-bot-0001"),
        ("fx-ada", "user", "fx-op-ada-0001"),
    ] {
        let (id, text) = &stored[op];
        let response = client
            .post(format!("{base}/sessions"))
            .header(
                "x-room-token",
                &fixture.credentials[&format!("member:{name}")],
            )
            .json(&serde_json::json!({ "name": name, "kind": kind }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 201, "{name}'s session");
        let session: Value = response.json().await.unwrap();
        let session = session["session_token"].as_str().unwrap().to_string();

        let response = client
            .post(format!("{base}/rooms/fx-team/messages"))
            .header("x-session-token", &session)
            .json(&serde_json::json!({
                "sender": name, "sender_type": kind, "text": text, "op_id": op,
            }))
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        let body = response.text().await.unwrap();
        assert_eq!(status, 409, "{name} over REST: {body}");
        assert!(
            body.contains("new operation") && !body.contains(text.as_str()),
            "{name}: the refusal says what to do and carries nobody's message: {body}"
        );
        let _ = id;

        let mut ws = connect_ws_protocols(
            format!("ws://127.0.0.1:{port}/ws?room=fx-team&name={name}&type={kind}"),
            &["loca.v1".into(), format!("loca.session.{session}")],
        )
        .await;
        wait_for(&mut ws, |v| v["t"] == "history").await;
        for (text, op) in [
            (text.clone(), op.to_string()),
            (format!("{name} marker"), format!("{op}-marker")),
        ] {
            let frame = serde_json::json!({"t": "send", "text": text, "op_id": op});
            ws.send(WsMessage::Text(frame.to_string().into()))
                .await
                .unwrap();
        }
        let marker = format!("{name} marker");
        wait_for(&mut ws, |v| {
            v["t"] == "msg" && v["message"]["text"] == marker
        })
        .await;
    }

    let response = client
        .get(format!("{base}/rooms/fx-team/messages?since=0&limit=1000"))
        .header("x-admin-token", ROOT)
        .send()
        .await
        .unwrap();
    let all: Vec<Value> = response.json().await.unwrap();
    for (id, text) in stored.values() {
        let rows: Vec<u64> = all
            .iter()
            .filter(|m| m["text"] == text.as_str())
            .map(|m| m["id"].as_u64().unwrap())
            .collect();
        assert_eq!(rows, [*id], "{text}: {all:#?}");
    }
}

/// What v0.13.0 (and every later build before membership keys stopped being
/// used as identifiers) stored for a member over an invite session: its
/// operations under `mb:<membership key>`, whether sent over REST or over
/// the socket (the seat key); its reactions under the same key; its pending
/// uploads with that key as the uploader. The v0.13.0 fixture was generated
/// without an invite session, so these rows are added to it here exactly as
/// that code wrote them. After the upgrade the database holds none of these
/// keys, and the same member, under a new session taken with the same
/// invite, gets each operation's original message back over REST and over
/// the WebSocket, takes back its reaction, and uploads as the same uploader.
#[tokio::test]
async fn an_invite_session_s_older_operations_reactions_and_uploads_stay_its_own() {
    let fixture = load();
    let key = fixture.credentials["member:fx-ada"].clone();
    let invite = fixture.credentials["davet:fx-team:fx-ada"].clone();
    let older = format!("mb:{key}");
    let pending_sha = hex(&Sha256::digest(b"fx pending note from ada\n"));
    let conn = rusqlite::Connection::open(&fixture.db).unwrap();
    let mut operations = Vec::new();
    for (text, op) in [
        ("fx invite post over rest", "fx-op-ada-invite-rest"),
        ("fx invite post over the socket", "fx-op-ada-invite-socket"),
    ] {
        conn.execute(
            "INSERT INTO messages
             (id, room, sender, sender_type, target, text, reply_to, ts, kind, principal, op_id)
             SELECT max(id) + 1, 'fx-team', 'fx-ada', 'user', NULL, ?1, NULL, 1794677279000,
                    'say', ?2, ?3
             FROM messages",
            rusqlite::params![text, older, op],
        )
        .unwrap();
        operations.push((conn.last_insert_rowid() as u64, text, op));
    }
    conn.execute(
        "INSERT INTO message_reactions (room, message_id, principal, reactor, emoji, created_at)
         VALUES ('fx-team', 8, ?1, 'fx-ada', '✓', 1794677279100)",
        [&older],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO attachment_pending (sha, room, uploader, name, mime, size, created_at)
         VALUES (?1, 'fx-team', ?2, 'fx-pending.txt', 'text/plain', 25, ?3)",
        rusqlite::params![pending_sha, older, now_ms()],
    )
    .unwrap();
    drop(conn);

    let (port, _server) = spawn_server_env(
        ROOT,
        &[
            ("DB_PATH", fixture.db.to_str().unwrap().to_string()),
            ("REQUIRE_INVITE", "1".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let member_id = management_id("mbid_", &format!("member:{key}"));

    // Nothing in the three tables names the key any more.
    let conn = rusqlite::Connection::open(&fixture.db).unwrap();
    for (table, column) in [
        ("messages", "principal"),
        ("message_reactions", "principal"),
        ("attachment_pending", "uploader"),
    ] {
        let holding: i64 = conn
            .query_row(
                &format!("SELECT count(*) FROM {table} WHERE instr({column}, ?1) > 0"),
                [&key],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(holding, 0, "{table}.{column} still names the key");
    }
    drop(conn);

    let response = client
        .post(format!("{base}/sessions"))
        .header("x-room-token", &invite)
        .json(&serde_json::json!({ "name": "fx-ada", "kind": "user" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 201, "an invite session");
    let session: Value = response.json().await.unwrap();
    let session = session["session_token"].as_str().unwrap().to_string();

    for (id, text, op) in &operations {
        let response = client
            .post(format!("{base}/rooms/fx-team/messages"))
            .header("x-session-token", &session)
            .json(&serde_json::json!({
                "sender": "fx-ada", "sender_type": "user", "text": text, "op_id": op,
            }))
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success(), "{op} over REST");
        let again: Value = response.json().await.unwrap();
        assert_eq!(again["id"].as_u64(), Some(*id), "{op} over REST: {again}");
    }
    let mut ws = connect_ws_protocols(
        format!("ws://127.0.0.1:{port}/ws?room=fx-team&name=fx-ada&type=user"),
        &["loca.v1".into(), format!("loca.session.{session}")],
    )
    .await;
    wait_for(&mut ws, |v| v["t"] == "history").await;
    for (_, text, op) in &operations {
        let frame = serde_json::json!({"t": "send", "text": text, "op_id": op});
        ws.send(WsMessage::Text(frame.to_string().into()))
            .await
            .unwrap();
    }
    let frame =
        serde_json::json!({"t": "send", "text": "fx ada marker", "op_id": "fx-op-ada-marker"});
    ws.send(WsMessage::Text(frame.to_string().into()))
        .await
        .unwrap();
    wait_for(&mut ws, |v| {
        v["t"] == "msg" && v["message"]["text"] == "fx ada marker"
    })
    .await;
    let all: Vec<Value> = client
        .get(format!("{base}/rooms/fx-team/messages?since=0&limit=1000"))
        .header("x-admin-token", ROOT)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    for (id, text, _) in &operations {
        let rows: Vec<u64> = all
            .iter()
            .filter(|m| m["text"] == *text)
            .map(|m| m["id"].as_u64().unwrap())
            .collect();
        assert_eq!(rows, [*id], "{text}: {all:#?}");
    }

    // The reaction is still hers to take back.
    let taken_back = client
        .post(format!("{base}/rooms/fx-team/messages/8/reactions"))
        .header("x-session-token", &session)
        .json(&serde_json::json!({ "emoji": "✓", "active": false }))
        .send()
        .await
        .unwrap();
    assert_eq!(taken_back.status().as_u16(), 200);
    let reactions: Vec<Value> = client
        .get(format!("{base}/rooms/fx-team/reactions"))
        .header("x-session-token", &session)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        !reactions
            .iter()
            .any(|r| r["message_id"] == 8 && r["emoji"] == "✓"),
        "{reactions:?}"
    );

    // A new upload names the same uploader the older one does now.
    let uploaded = client
        .post(format!("{base}/rooms/fx-team/attachments"))
        .header("x-session-token", &session)
        .header("content-type", "text/plain")
        .header("x-filename", "fx-after.txt")
        .body("fx upload from ada after the upgrade\n")
        .send()
        .await
        .unwrap();
    assert!(uploaded.status().is_success(), "{}", uploaded.status());
    let uploaded: Value = uploaded.json().await.unwrap();
    let conn = rusqlite::Connection::open(&fixture.db).unwrap();
    let uploader = |sha: &str| -> String {
        conn.query_row(
            "SELECT uploader FROM attachment_pending WHERE sha = ?1 AND room = 'fx-team'",
            [sha],
            |row| row.get(0),
        )
        .unwrap()
    };
    assert_eq!(uploader(&pending_sha), format!("mb:{member_id}"));
    assert_eq!(
        uploader(uploaded["id"].as_str().unwrap()),
        uploader(&pending_sha)
    );
}
