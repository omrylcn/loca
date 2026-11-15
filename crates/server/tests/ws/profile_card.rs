//! A person's full name and icon on the real binary: each person sets only
//! their own, the open rosters follow live, the card survives a restart and
//! an older database, and a full name never acts as an `@name`.
use std::collections::HashMap;

use serde_json::json;

use super::*;

const ROOT: &str = "card-fixture-root";
const ROOM: &str = "card-table";

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The shared building key: a session taken with it names no principal.
const BUILDING: &str = "card-fixture-building";

async fn boot(db: &str) -> (u16, ServerGuard) {
    let env = vec![
        ("DB_PATH", db.to_string()),
        ("ROOM_TOKEN", BUILDING.to_string()),
        ("REQUIRE_INVITE", "1".into()),
        ("REQUIRE_SESSIONS", "1".into()),
    ];
    spawn_server_env(ROOT, &env).await
}

async fn send(
    base: &str,
    method: &str,
    path: &str,
    header: Option<(&str, &str)>,
    body: Option<Value>,
) -> (reqwest::StatusCode, Value) {
    let mut request =
        reqwest::Client::new().request(method.parse().unwrap(), format!("{base}{path}"));
    if let Some((name, value)) = header {
        request = request.header(name, value);
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.unwrap();
    let status = response.status();
    let text = response.text().await.unwrap();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

async fn ok(base: &str, method: &str, path: &str, header: (&str, &str), body: Value) -> Value {
    let (status, value) = send(base, method, path, Some(header), Some(body)).await;
    assert!(status.is_success(), "{method} {path}: {status} {value}");
    value
}

async fn session(base: &str, header: (&str, &str), name: &str, kind: &str) -> String {
    ok(
        base,
        "POST",
        "/sessions",
        header,
        json!({ "name": name, "kind": kind }),
    )
    .await["session_token"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn put_card(base: &str, session: &str, body: Value) -> (reqwest::StatusCode, Value) {
    send(
        base,
        "PUT",
        "/profile/card",
        Some(("x-session-token", session)),
        Some(body),
    )
    .await
}

async fn ws(port: u16, session: &str, query: &str) -> Ws {
    connect_ws_protocols(
        format!("ws://127.0.0.1:{port}/ws?room={ROOM}&{query}"),
        &["loca.v1".to_string(), format!("loca.session.{session}")],
    )
    .await
}

/// The next roster in which `name` wears exactly `full_name`.
async fn roster_where(ws: &mut Ws, name: &str, full_name: Option<&str>) -> Vec<Value> {
    let frame = wait_for(ws, |v| {
        v["t"] == "members"
            && v["members"].as_array().unwrap().iter().any(|m| {
                m["name"] == name && m.get("full_name").and_then(Value::as_str) == full_name
            })
    })
    .await;
    frame["members"].as_array().unwrap().clone()
}

fn member<'a>(roster: &'a [Value], name: &str) -> &'a Value {
    roster
        .iter()
        .find(|m| m["name"] == name)
        .unwrap_or_else(|| panic!("{name} on the roster"))
}

#[tokio::test]
async fn each_person_sets_only_their_own_card_and_open_rosters_follow_live_and_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("cards.db").to_string_lossy().to_string();
    let (port, guard) = boot(&db).await;
    let base = format!("http://127.0.0.1:{port}");
    create_test_room(port, ROOT, ROOM).await;
    let mut keys = HashMap::new();
    for (name, kind) in [("card-ada", "user"), ("card-bo", "agent")] {
        let key = ok(
            &base,
            "POST",
            "/members",
            ("x-admin-token", ROOT),
            json!({ "name": name, "kind": kind }),
        )
        .await["token"]
            .as_str()
            .unwrap()
            .to_string();
        ok(
            &base,
            "POST",
            &format!("/rooms/{ROOM}/invites"),
            ("x-admin-token", ROOT),
            json!({ "name": name }),
        )
        .await;
        keys.insert(name, key);
    }
    let ada = session(
        &base,
        ("x-room-token", &keys["card-ada"]),
        "card-ada",
        "user",
    )
    .await;
    let bo = session(
        &base,
        ("x-room-token", &keys["card-bo"]),
        "card-bo",
        "agent",
    )
    .await;
    let master = session(&base, ("x-admin-token", ROOT), "watcher", "user").await;

    // Ada and Bo sit at the table; the Master watches from an open client.
    let mut ada_ws = ws(port, &ada, "name=card-ada").await;
    wait_for(&mut ada_ws, |v| v["t"] == "history").await;
    let mut bo_ws = ws(port, &bo, "name=card-bo&filter=mentions&turn_max=1").await;
    let client = reqwest::Client::new();
    wait_for_member(
        &client,
        &base,
        ROOM,
        "card-bo",
        Some(("x-admin-token", ROOT)),
    )
    .await;
    let mut watcher = ws(port, &master, "name=watcher&watch=1").await;
    wait_for(&mut watcher, |v| v["t"] == "history").await;

    // Signed out: nothing. Not JSON, not an object: refused.
    let (status, _) = send(
        &base,
        "PUT",
        "/profile/card",
        None,
        Some(json!({ "icon": "leaf" })),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED);
    let (status, _) = put_card(&base, &bo, json!(["leaf"])).await;
    assert_eq!(status, reqwest::StatusCode::BAD_REQUEST);

    // An agent sets its own through the same route; both open clients see it.
    let (status, saved) = put_card(
        &base,
        &bo,
        json!({ "full_name": "Ada Lovelace", "icon": "leaf" }),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::OK, "{saved}");
    assert_eq!(
        saved,
        json!({ "full_name": "Ada Lovelace", "icon": "leaf" })
    );
    for client in [&mut ada_ws, &mut watcher] {
        let roster = roster_where(client, "card-bo", Some("Ada Lovelace")).await;
        assert_eq!(member(&roster, "card-bo")["icon"], "leaf");
        assert!(member(&roster, "card-ada").get("full_name").is_none());
        assert!(member(&roster, "card-bo")["principal_id"]
            .as_str()
            .is_some_and(|id| id.starts_with("pr_")));
    }

    // Ada, an ordinary member, sets the same full name: still two people.
    let (status, _) = put_card(&base, &ada, json!({ "full_name": "Ada Lovelace" })).await;
    assert_eq!(status, reqwest::StatusCode::OK);
    let roster = roster_where(&mut watcher, "card-ada", Some("Ada Lovelace")).await;
    let named: Vec<&Value> = roster
        .iter()
        .filter(|m| m["full_name"] == "Ada Lovelace")
        .collect();
    assert_eq!(named.len(), 2, "{roster:?}");
    assert_ne!(named[0]["name"], named[1]["name"]);
    let (_, profiles) = send(
        &base,
        "GET",
        "/profiles",
        Some(("x-admin-token", ROOT)),
        None,
    )
    .await;
    let twins: Vec<&Value> = profiles
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["full_name"] == "Ada Lovelace")
        .collect();
    assert_eq!(twins.len(), 2);
    assert_ne!(twins[0]["id"], twins[1]["id"]);

    // A body that names somebody else changes nobody.
    let ada_id = send(
        &base,
        "GET",
        "/profile",
        Some(("x-session-token", &ada)),
        None,
    )
    .await
    .1["principal"]["id"]
        .clone();
    for bad in [
        json!({ "full_name": "Bo Was Here", "principal_id": ada_id }),
        json!({ "full_name": "Bo Was Here", "name": "card-ada" }),
        json!({ "display_name": "card-ada" }),
    ] {
        let (status, _) = put_card(&base, &bo, bad.clone()).await;
        assert_eq!(status, reqwest::StatusCode::BAD_REQUEST, "{bad}");
    }
    // Each class of refused value, over the wire.
    for bad in [
        json!({ "full_name": "Master" }),
        json!({ "full_name": "Ada\nLovelace" }),
        json!({ "full_name": "Ada\u{7}" }),
        json!({ "full_name": "\u{202E}ecalevoL" }),
        json!({ "full_name": "Ad\u{200B}a" }),
        json!({ "full_name": "@card-ada" }),
        json!({ "full_name": "Ada \u{1F600}" }),
        json!({ "full_name": "a".repeat(65) }),
        json!({ "icon": "\u{1F642}" }),
        json!({ "icon": "https://example.invalid/me.png" }),
        json!({ "full_name": "Fine Name", "icon": "rocket" }),
    ] {
        let (status, _) = put_card(&base, &bo, bad.clone()).await;
        assert_eq!(status, reqwest::StatusCode::BAD_REQUEST, "{bad}");
    }
    let ada_profile = send(
        &base,
        "GET",
        "/profile",
        Some(("x-session-token", &ada)),
        None,
    )
    .await
    .1;
    assert_eq!(ada_profile["principal"]["full_name"], "Ada Lovelace");
    assert!(ada_profile["principal"].get("icon").is_none());
    let bo_profile = send(
        &base,
        "GET",
        "/profile",
        Some(("x-session-token", &bo)),
        None,
    )
    .await
    .1;
    assert_eq!(
        bo_profile["principal"]["full_name"], "Ada Lovelace",
        "the refused bodies changed nothing"
    );
    assert_eq!(bo_profile["principal"]["icon"], "leaf");

    // Bo wears Ada's @name as a full name: allowed, and it is only a label.
    let (status, _) = put_card(&base, &bo, json!({ "full_name": "card-ada" })).await;
    assert_eq!(status, reqwest::StatusCode::OK);
    let roster = roster_where(&mut watcher, "card-bo", Some("card-ada")).await;
    assert_eq!(member(&roster, "card-ada")["full_name"], "Ada Lovelace");
    // A call to @card-ada is not a call to Bo; a call to @card-bo is.
    for text in ["@card-ada are you there?", "@card-bo and you?"] {
        ok(
            &base,
            "POST",
            &format!("/rooms/{ROOM}/messages"),
            ("x-session-token", &master),
            json!({ "sender": "ignored", "sender_type": "user", "text": text }),
        )
        .await;
    }
    let reached = wait_for(&mut bo_ws, |v| v["t"] == "msg" || v["t"] == "turn").await;
    let text = |v: &Value| {
        if v["t"] == "msg" {
            v["message"]["text"].clone()
        } else {
            v["messages"][0]["text"].clone()
        }
    };
    assert_eq!(
        text(&reached),
        "@card-bo and you?",
        "a full name is never an address"
    );
    // What Bo says is attributed to its @name.
    let said = ok(
        &base,
        "POST",
        &format!("/rooms/{ROOM}/messages"),
        ("x-session-token", &bo),
        json!({ "sender": "card-ada", "sender_type": "user", "text": "hello from bo" }),
    )
    .await;
    assert_eq!(said["sender"], "card-bo");
    let heard = wait_for(&mut ada_ws, |v| {
        v["t"] == "msg" && v["message"]["text"] == "hello from bo"
    })
    .await;
    assert_eq!(heard["message"]["sender"], "card-bo");
    assert!(
        heard["message"].get("full_name").is_none(),
        "a message frame carries no card"
    );

    // A rank word in another script is accepted and grants nothing.
    let (status, _) = put_card(&base, &bo, json!({ "full_name": "\u{041C}aster" })).await;
    assert_eq!(status, reqwest::StatusCode::OK);
    let roster = roster_where(&mut watcher, "card-bo", Some("\u{041C}aster")).await;
    assert!(member(&roster, "card-bo").get("role").is_none());
    let (status, _) = send(&base, "GET", "/keys", Some(("x-session-token", &bo)), None).await;
    assert!(status.is_client_error(), "still no Master: {status}");
    let (_, ranks) = send(
        &base,
        "GET",
        "/building/ranks",
        Some(("x-admin-token", ROOT)),
        None,
    )
    .await;
    assert!(ranks
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["name"] != "card-bo"));

    // The Building lists carry the card; whoami too.
    let (_, residents) = send(
        &base,
        "GET",
        "/residents",
        Some(("x-admin-token", ROOT)),
        None,
    )
    .await;
    let row = residents
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "card-ada")
        .unwrap()
        .clone();
    assert_eq!(row["full_name"], "Ada Lovelace");
    assert_eq!(row["principal_id"], ada_id);
    let (_, members) = send(
        &base,
        "GET",
        "/members",
        Some(("x-admin-token", ROOT)),
        None,
    )
    .await;
    let row = members
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "card-bo")
        .unwrap()
        .clone();
    assert_eq!(row["full_name"], "\u{041C}aster");
    assert_eq!(row["icon"], "leaf");
    let (_, who) = send(
        &base,
        "GET",
        "/whoami",
        Some(("x-session-token", &bo)),
        None,
    )
    .await;
    assert_eq!(who["name"], "card-bo");
    assert_eq!(who["full_name"], "\u{041C}aster");
    assert!(who["principal_id"]
        .as_str()
        .is_some_and(|id| id.starts_with("pr_")));

    // Clearing: the roster goes back to the plain shape plus the verified
    // principal the seat always names.
    let (status, cleared) = put_card(&base, &ada, json!({ "full_name": null, "icon": "" })).await;
    assert_eq!(status, reqwest::StatusCode::OK);
    assert_eq!(cleared, json!({ "full_name": null, "icon": null }));
    let roster = roster_where(&mut watcher, "card-ada", None).await;
    assert_eq!(
        member(&roster, "card-ada"),
        &json!({ "name": "card-ada", "type": "user", "principal_id": ada_id })
    );

    // A restart keeps Bo's card.
    drop((ada_ws, bo_ws, watcher));
    drop(guard);
    wait_for_server_shutdown(port).await;
    let (port, _guard) = boot(&db).await;
    let base = format!("http://127.0.0.1:{port}");
    let bo = session(
        &base,
        ("x-room-token", &keys["card-bo"]),
        "card-bo",
        "agent",
    )
    .await;
    let profile = send(
        &base,
        "GET",
        "/profile",
        Some(("x-session-token", &bo)),
        None,
    )
    .await
    .1;
    assert_eq!(profile["principal"]["full_name"], "\u{041C}aster");
    assert_eq!(profile["principal"]["icon"], "leaf");
    let mut bo_ws = ws(port, &bo, "name=card-bo").await;
    wait_for(&mut bo_ws, |v| v["t"] == "history").await;
    let roster = roster_where(&mut bo_ws, "card-bo", Some("\u{041C}aster")).await;
    assert_eq!(member(&roster, "card-bo")["icon"], "leaf");
}

#[tokio::test]
async fn a_v0_13_0_database_upgrades_and_its_people_read_as_before() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("old.db");
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute_batch(include_str!("../fixtures/upgrade-v0.13.0/v0.13.0.sql"))
        .unwrap();
    let db = db.to_string_lossy().to_string();
    // The generator's synthetic root key (see ws/upgrade.rs).
    let root = "fixture-v0130-root";
    let (port, guard) = spawn_server_env(root, &[("DB_PATH", db.clone())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let (status, profiles) = send(
        &base,
        "GET",
        "/profiles",
        Some(("x-admin-token", root)),
        None,
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::OK);
    assert!(!profiles.as_array().unwrap().is_empty());
    for profile in profiles.as_array().unwrap() {
        assert!(
            profile.get("full_name").is_none() && profile.get("icon").is_none(),
            "{profile}"
        );
    }
    let (status, saved) = send(
        &base,
        "PUT",
        "/profile/card",
        Some(("x-admin-token", root)),
        Some(json!({ "full_name": "Fixture Master", "icon": "sun" })),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::OK, "{saved}");
    drop(guard);
    wait_for_server_shutdown(port).await;
    let (port, _guard) = spawn_server_env(root, &[("DB_PATH", db)]).await;
    let base = format!("http://127.0.0.1:{port}");
    let (_, profile) = send(
        &base,
        "GET",
        "/profile",
        Some(("x-admin-token", root)),
        None,
    )
    .await;
    assert_eq!(profile["principal"]["full_name"], "Fixture Master");
    assert_eq!(profile["principal"]["icon"], "sun");
}

/// A message wears a card only when the server says who verifiably wrote it.
/// A line the root key wrote under "alice" names no writer before, after
/// the real Alice arrives, after a restart and after Alice renames herself;
/// Alice's own lines name her.
#[tokio::test]
async fn only_a_verified_writer_is_named_on_a_message() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("writers.db").to_string_lossy().to_string();
    let (port, guard) = boot(&db).await;
    let base = format!("http://127.0.0.1:{port}");
    create_test_room(port, ROOT, ROOM).await;
    let master = session(&base, ("x-admin-token", ROOT), "watcher", "user").await;
    let mut watcher = ws(port, &master, "name=watcher&watch=1").await;
    wait_for(&mut watcher, |v| v["t"] == "history").await;

    // A session taken with the shared building key carries no principal:
    // the name in its body, "alice", is a label nobody verified.
    let shared = session(&base, ("x-room-token", BUILDING), "alice", "user").await;
    ok(
        &base,
        "POST",
        &format!("/rooms/{ROOM}/messages"),
        ("x-session-token", &shared),
        json!({ "sender": "alice", "sender_type": "user", "text": "unverified words" }),
    )
    .await;
    let unverified = wait_for(&mut watcher, |v| {
        v["t"] == "msg" && v["message"]["text"] == "unverified words"
    })
    .await["message"]
        .clone();
    assert_eq!(unverified["sender"], "alice");
    assert!(unverified.get("sender_principal").is_none(), "{unverified}");
    ok(
        &base,
        "DELETE",
        "/sessions",
        ("x-session-token", &shared),
        Value::Null,
    )
    .await;

    // The real Alice arrives (a Smaster, so she may rename herself later).
    let code = ok(
        &base,
        "POST",
        "/smasters",
        ("x-admin-token", ROOT),
        json!({ "name": "alice" }),
    )
    .await["token"]
        .as_str()
        .unwrap()
        .to_string();
    let alice = session(&base, ("x-admin-token", &code), "alice", "user").await;
    let alice_id = send(
        &base,
        "GET",
        "/profile",
        Some(("x-session-token", &alice)),
        None,
    )
    .await
    .1["principal"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, _) = put_card(
        &base,
        &alice,
        json!({ "full_name": "Alice Real", "icon": "star" }),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::OK);
    let mut alice_ws = ws(port, &alice, "name=alice").await;
    let history = wait_for(&mut alice_ws, |v| v["t"] == "history").await;
    let old = history["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["text"] == "unverified words")
        .unwrap()
        .clone();
    assert!(old.get("sender_principal").is_none(), "history: {old}");
    let roster = roster_where(&mut watcher, "alice", Some("Alice Real")).await;
    assert_eq!(member(&roster, "alice")["principal_id"], alice_id.as_str());
    alice_ws
        .send(WsMessage::Text(
            json!({ "t": "send", "text": "alice speaks" }).to_string(),
        ))
        .await
        .unwrap();
    let hers = wait_for(&mut watcher, |v| {
        v["t"] == "msg" && v["message"]["text"] == "alice speaks"
    })
    .await["message"]
        .clone();
    assert_eq!(hers["sender_principal"], alice_id.as_str());
    let said = ok(
        &base,
        "POST",
        &format!("/rooms/{ROOM}/messages"),
        ("x-session-token", &alice),
        json!({ "sender": "ignored", "sender_type": "user", "text": "alice over rest" }),
    )
    .await;
    assert_eq!(said["sender_principal"], alice_id.as_str());

    // Every way a client reads messages says the same.
    let writers = |messages: &Value| -> Vec<(String, Option<String>)> {
        messages
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| {
                m["text"]
                    .as_str()
                    .is_some_and(|t| t.contains("words") || t.starts_with("alice"))
            })
            .map(|m| {
                (
                    m["text"].as_str().unwrap().to_string(),
                    m["sender_principal"].as_str().map(str::to_string),
                )
            })
            .collect()
    };
    let expected = vec![
        ("unverified words".to_string(), None),
        ("alice speaks".to_string(), Some(alice_id.clone())),
        ("alice over rest".to_string(), Some(alice_id.clone())),
    ];
    let as_master = Some(("x-admin-token", ROOT));
    for path in [
        format!("/rooms/{ROOM}/messages"),
        format!("/rooms/{ROOM}/messages?since=0&limit=100"),
    ] {
        let (_, messages) = send(&base, "GET", &path, as_master, None).await;
        assert_eq!(writers(&messages), expected, "{path}");
    }
    let (_, found) = send(
        &base,
        "GET",
        &format!("/rooms/{ROOM}/search?q=alice"),
        as_master,
        None,
    )
    .await;
    let found = &found["messages"];
    let mut seen: Vec<(String, Option<String>)> = found
        .as_array()
        .unwrap_or_else(|| panic!("search answers messages: {found}"))
        .iter()
        .map(|m| {
            (
                m["text"].as_str().unwrap().to_string(),
                m["sender_principal"].as_str().map(str::to_string),
            )
        })
        .collect();
    seen.sort();
    assert_eq!(
        seen,
        vec![
            ("alice over rest".to_string(), Some(alice_id.clone())),
            ("alice speaks".to_string(), Some(alice_id.clone())),
            ("unverified words".to_string(), None),
        ],
        "search finds by sender and names only the verified writer"
    );

    // A restart, then Alice renames herself: history is relabelled by
    // writer, and the unverified line keeps its label and names nobody.
    drop((alice_ws, watcher));
    drop(guard);
    wait_for_server_shutdown(port).await;
    let (port, _guard) = boot(&db).await;
    let base = format!("http://127.0.0.1:{port}");
    let alice = session(&base, ("x-admin-token", &code), "alice", "user").await;
    ok(
        &base,
        "PUT",
        "/profile",
        ("x-session-token", &alice),
        json!({ "display_name": "alice-renamed" }),
    )
    .await;
    for path in [
        format!("/rooms/{ROOM}/messages"),
        format!("/rooms/{ROOM}/messages?since=0&limit=100"),
    ] {
        let (_, messages) = send(&base, "GET", &path, Some(("x-admin-token", ROOT)), None).await;
        let rows: Vec<(String, String, Option<String>)> = messages
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["text"] == "unverified words" || m["text"] == "alice speaks")
            .map(|m| {
                (
                    m["text"].as_str().unwrap().to_string(),
                    m["sender"].as_str().unwrap().to_string(),
                    m["sender_principal"].as_str().map(str::to_string),
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                ("unverified words".into(), "alice".into(), None),
                (
                    "alice speaks".into(),
                    "alice-renamed".into(),
                    Some(alice_id.clone())
                ),
            ],
            "{path}"
        );
    }
}
