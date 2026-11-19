//! The name a new loca may be opened under (`src/hub/loca_name.rs`): ASCII
//! letters, digits, `.`, `_` and `-`, starting with a letter or digit, at
//! most 128 characters, and no other loca under the same agent kit key
//! (`DAVET_<name>`, every other character turned into `_`). A display name
//! with spaces is the loca's title. A loca that already has another name
//! keeps it, and every route still serves it.
use super::*;

const MASTER: &str = "names-master-key";
const ADMIN: (&str, &str) = ("x-admin-token", MASTER);

async fn send(
    method: reqwest::Method,
    url: String,
    header: (&str, &str),
    body: Option<Value>,
) -> (u16, String) {
    let mut req = reqwest::Client::new()
        .request(method, url)
        .header(header.0, header.1);
    if let Some(body) = body {
        req = req.json(&body);
    }
    let r = req.send().await.unwrap();
    let status = r.status().as_u16();
    (status, r.text().await.unwrap_or_default())
}

async fn create(base: &str, name: &str) -> (u16, String) {
    send(
        reqwest::Method::POST,
        format!("{base}/rooms"),
        ADMIN,
        Some(serde_json::json!({ "name": name })),
    )
    .await
}

fn path(room: &str) -> String {
    room.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

async fn closed_building(db: &std::path::Path, extra: &[(&str, String)]) -> (u16, ServerGuard) {
    let mut env = vec![
        ("DB_PATH", db.to_str().unwrap().to_string()),
        ("REQUIRE_INVITE", "1".to_string()),
    ];
    env.extend(extra.iter().map(|(k, v)| (*k, v.clone())));
    spawn_server_env(MASTER, &env).await
}

/// Start the real binary and expect it to refuse to start: it exits non-zero
/// within the hang guard, and its output says why.
async fn refuses_to_start(env: &[(&str, String)]) -> String {
    let bin = env!("CARGO_BIN_EXE_room-server");
    let mut cmd = tokio::process::Command::new(bin);
    cmd.env("PORT", "0")
        .env("RUST_LOG", "warn")
        .env("ADMIN_TOKEN", MASTER)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let child = cmd.spawn().unwrap();
    let output = tokio::time::timeout(ASYNC_STATE_HANG_GUARD, child.wait_with_output())
        .await
        .expect("the server started, but it must refuse to")
        .unwrap();
    assert!(!output.status.success(), "the server exited cleanly");
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[tokio::test]
async fn a_new_loca_name_is_ascii_letters_digits_dot_underscore_dash() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _guard) = closed_building(&dir.path().join("names.db"), &[]).await;
    let base = format!("http://127.0.0.1:{port}");

    let (status, body) = create(&base, "my loca").await;
    assert_eq!(status, 400, "a space: {body}");
    assert!(
        body.contains("title"),
        "the refusal points to the title: {body}"
    );
    assert!(
        body.contains("This Loca"),
        "and says where it is set: {body}"
    );

    for name in [
        "a/b",
        "a#b",
        "a@b",
        "işler",
        "ÇAY",
        "party🎉",
        "-dash-first",
        ".hidden",
        "_under",
    ] {
        let (status, body) = create(&base, name).await;
        assert_eq!(status, 400, "{name:?} must be refused: {body}");
        assert!(body.contains("title"), "{name:?}: {body}");
    }
    let (status, body) = create(&base, &"a".repeat(129)).await;
    assert_eq!(status, 400, "over 128: {body}");

    for name in [
        "my-loca",
        "release.14",
        "Room_2",
        "9lives",
        &"b".repeat(128),
    ] {
        let (status, body) = create(&base, name).await;
        assert_eq!(status, 201, "{name:?} must be accepted: {body}");
    }

    // The same kit key as an existing loca: the kit would keep one invite
    // for both.
    let (status, body) = create(&base, "my_loca").await;
    assert_eq!(status, 409, "{body}");
    assert!(
        !body.contains("my-loca"),
        "does not name the other loca (the caller may not be allowed to see it): {body}"
    );
    assert!(
        body.contains("DAVET_my_loca"),
        "and the key they share: {body}"
    );
    let (status, body) = create(&base, "my.loca").await;
    assert_eq!(status, 409, "{body}");
    let (status, body) = create(&base, "release_14").await;
    assert_eq!(status, 409, "{body}");

    // A private conversation's id is `bb-` and a hash anyone who can list
    // people can compute. A name under its kit key is refused the same way
    // whether or not the conversation exists, so nobody learns that it does.
    for name in ["bb_0123456789abcdef", "BB_x", "bb.x"] {
        let (status, body) = create(&base, name).await;
        assert_eq!(status, 400, "{name:?}: {body}");
        assert!(body.contains("private conversations"), "{name:?}: {body}");
    }
    // Capitals and small letters share a key on Windows.
    let (status, body) = create(&base, "room_2").await;
    assert_eq!(status, 409, "Room_2 exists: {body}");
    assert!(!body.contains("Room_2"), "{body}");

    // Nothing refused was opened.
    let (_, rooms) = send(reqwest::Method::GET, format!("{base}/rooms"), ADMIN, None).await;
    let rooms: Vec<Value> = serde_json::from_str(&rooms).unwrap();
    let ids: Vec<&str> = rooms.iter().filter_map(|r| r["room"].as_str()).collect();
    for refused in [
        "my loca",
        "my_loca",
        "my.loca",
        "a/b",
        "işler",
        "release_14",
    ] {
        assert!(!ids.contains(&refused), "{refused:?} was opened: {ids:?}");
    }
}

/// A loca opened under an old name (a space, a Turkish letter) before the
/// rule: inserted into the database the way an older server left it. It
/// still takes messages and settings, and a new loca with a different kit
/// key still opens beside it.
#[tokio::test]
async fn an_existing_loca_with_an_old_name_keeps_working() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("old-names.db");
    let (port, guard) = closed_building(&db, &[]).await;
    let base = format!("http://127.0.0.1:{port}");
    let (status, body) = create(&base, "template").await;
    assert_eq!(status, 201, "{body}");
    drop(guard);
    wait_for_server_shutdown(port).await;

    let old_names = ["old loca", "çay ocağı"];
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        for old in old_names {
            conn.execute(
                "INSERT INTO rooms (room, mode, settings) \
                 SELECT ?1, mode, settings FROM rooms WHERE room = 'template'",
                [old],
            )
            .unwrap();
        }
    }

    let (port, _guard) = closed_building(&db, &[]).await;
    let base = format!("http://127.0.0.1:{port}");
    // A member of the Building, so that the invite below is answered for
    // what it is (a 409 here used to mean "not a member yet" and proved
    // nothing about the loca).
    let (status, body) = send(
        reqwest::Method::POST,
        format!("{base}/members"),
        ADMIN,
        Some(serde_json::json!({ "name": "old-name-agent", "kind": "agent" })),
    )
    .await;
    assert!(status < 300, "admit: {status} {body}");
    for old in old_names {
        let at = format!("{base}/rooms/{}", path(old));
        let (status, body) = send(
            reqwest::Method::POST,
            format!("{at}/messages"),
            ADMIN,
            Some(serde_json::json!({
                "sender": "master", "sender_type": "user", "text": "still here"
            })),
        )
        .await;
        assert!(status < 300, "post in {old:?}: {status} {body}");
        let (status, body) =
            send(reqwest::Method::GET, format!("{at}/messages"), ADMIN, None).await;
        assert_eq!(status, 200, "read {old:?}: {body}");
        assert!(body.contains("still here"), "{old:?}: {body}");

        let (status, body) = send(
            reqwest::Method::PUT,
            format!("{at}/settings"),
            ADMIN,
            Some(serde_json::json!({ "title": "Shown Title" })),
        )
        .await;
        assert_eq!(status, 200, "title on {old:?}: {body}");
        let (status, body) = send(
            reqwest::Method::PUT,
            format!("{at}/settings"),
            ADMIN,
            Some(serde_json::json!({ "title": "" })),
        )
        .await;
        assert_eq!(status, 200, "clear title on {old:?}: {body}");
        let (status, body) =
            send(reqwest::Method::GET, format!("{at}/settings"), ADMIN, None).await;
        assert_eq!(status, 200, "settings of {old:?}: {body}");
        let (status, body) = send(
            reqwest::Method::POST,
            format!("{at}/invites"),
            ADMIN,
            Some(serde_json::json!({ "name": "old-name-agent" })),
        )
        .await;
        assert!(status < 300, "invite into {old:?}: {status} {body}");
    }
    // The old loca's kit key is taken: the new name that maps to it is
    // refused; one that does not still opens.
    let (status, body) = create(&base, "old_loca").await;
    assert_eq!(status, 409, "{body}");
    assert!(!body.contains("old loca"), "{body}");
    let (status, body) = create(&base, "old-loca-2").await;
    assert_eq!(status, 201, "{body}");
}

/// `ROOM_RENAME` gives a loca a new id, so the new id follows the rule; a
/// rename to a valid id still works (`loca_identity.rs` covers that).
#[tokio::test]
async fn room_rename_to_a_name_the_rule_refuses_does_not_start() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("rename.db");
    let (port, guard) = closed_building(&db, &[]).await;
    let base = format!("http://127.0.0.1:{port}");
    for name in ["proj", "my-loca"] {
        let (status, body) = create(&base, name).await;
        assert_eq!(status, 201, "{body}");
    }
    drop(guard);
    wait_for_server_shutdown(port).await;

    let env = |rename: &str| {
        vec![
            ("DB_PATH", db.to_str().unwrap().to_string()),
            ("REQUIRE_INVITE", "1".to_string()),
            ("ROOM_RENAME", rename.to_string()),
        ]
    };
    let out = refuses_to_start(&env("proj:my proj")).await;
    assert!(out.contains("title"), "{out}");
    let out = refuses_to_start(&env("proj:my_loca")).await;
    assert!(out.contains("DAVET_my_loca"), "{out}");

    // Nothing moved; a valid rename then goes through.
    let (port, _guard) = closed_building(&db, &[("ROOM_RENAME", "proj:proj-2".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let (status, _) = send(
        reqwest::Method::GET,
        format!("{base}/rooms/proj-2/settings"),
        ADMIN,
        None,
    )
    .await;
    assert_eq!(status, 200);
}

/// The home loca (`LOCA_AGENT_ROOM`) is opened at boot. It is not held to the
/// rule by force (a refusal would stop an upgrade); the default `iye` passes
/// it anyway.
#[tokio::test]
async fn a_new_home_loca_follows_the_rule() {
    // A home configured before the rule and never stored does not stop the
    // upgrade from starting: it opens as configured.
    let (port, _old) = spawn_server_env(MASTER, &[("LOCA_AGENT_ROOM", "ev odası".into())]).await;
    let (status, body) = send(
        reqwest::Method::GET,
        format!(
            "http://127.0.0.1:{port}/rooms/{}/settings",
            path("ev odası")
        ),
        ADMIN,
        None,
    )
    .await;
    assert_eq!(status, 200, "an old-style home still opens: {body}");
    let (port, _guard) = spawn_server_env(MASTER, &[("LOCA_AGENT_ROOM", "home-1".into())]).await;
    let (status, _) = send(
        reqwest::Method::GET,
        format!("http://127.0.0.1:{port}/rooms/home-1/settings"),
        ADMIN,
        None,
    )
    .await;
    assert_eq!(status, 200);
}

/// The loopback sandbox (no ADMIN_TOKEN) opens a loca the first time a route
/// or the socket names it. That is opening a loca too.
#[tokio::test]
async fn the_sandbox_does_not_open_a_loca_the_rule_refuses() {
    let (port, _guard) = spawn_server().await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let post = |room: &str| {
        client
            .post(format!("{base}/rooms/{}/messages", path(room)))
            .json(&serde_json::json!({ "sender": "x", "sender_type": "user", "text": "hi" }))
            .send()
    };
    let r = post("my-loca").await.unwrap();
    assert!(r.status().is_success(), "{}", r.status());
    let r = post("my loca").await.unwrap();
    assert_eq!(r.status().as_u16(), 400);
    assert!(r.text().await.unwrap().contains("title"));
    let r = post("my_loca").await.unwrap();
    assert_eq!(r.status().as_u16(), 409);

    let url = format!("ws://127.0.0.1:{port}/ws?room={}&name=x", path("sand box"));
    assert!(
        tokio_tungstenite::connect_async(url).await.is_err(),
        "the socket opened a loca the rule refuses"
    );
    let rooms: Vec<Value> = client
        .get(format!("{base}/rooms"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ids: Vec<&str> = rooms.iter().filter_map(|r| r["room"].as_str()).collect();
    assert!(ids.contains(&"my-loca"), "{ids:?}");
    for refused in ["my loca", "my_loca", "sand box"] {
        assert!(!ids.contains(&refused), "{refused:?} was opened: {ids:?}");
    }
}

/// In the sandbox an invite or a call opens its loca too. For a name the
/// rule refuses, the answer used to be 404 "create the loca before issuing a
/// davet", which sends the person to a create that is refused as well. It is
/// the rule's own answer: 400 naming the rule, 409 for a kit-key clash.
#[tokio::test]
async fn the_sandbox_answers_an_invite_to_a_refused_name_with_the_rule() {
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let console_port = socket.local_addr().unwrap().port();
    drop(socket);
    let (port, _guard) =
        spawn_server_env("", &[("ADMIN_CONSOLE_PORT", console_port.to_string())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let console = format!("http://127.0.0.1:{console_port}");
    let (status, body) = send(
        reqwest::Method::POST,
        format!("{console}/api/members"),
        ("x-loca-console", "1"),
        Some(serde_json::json!({ "name": "ada", "kind": "agent" })),
    )
    .await;
    assert!(status < 300, "{status} {body}");
    let (status, body) = send(
        reqwest::Method::POST,
        format!("{base}/rooms/my-loca/messages"),
        ("x-unused", "1"),
        Some(serde_json::json!({ "sender": "x", "sender_type": "user", "text": "hi" })),
    )
    .await;
    assert!(status < 300, "{status} {body}");

    for (loca, want) in [("my loca", 400), ("my_loca", 409)] {
        let (status, body) = send(
            reqwest::Method::POST,
            format!("{console}/api/invites"),
            ("x-loca-console", "1"),
            Some(serde_json::json!({ "loca": loca, "name": "ada" })),
        )
        .await;
        assert_eq!(status, want, "desk invite to {loca:?}: {body}");
        assert!(!body.contains("create the loca"), "{body}");
        let (status, body) = send(
            reqwest::Method::POST,
            format!("{base}/rooms/{}/call", path(loca)),
            ("x-unused", "1"),
            Some(serde_json::json!({ "name": "ada" })),
        )
        .await;
        assert_eq!(status, want, "call into {loca:?}: {body}");
        assert!(!body.contains("create the loca"), "{body}");
    }
    let (status, body) = send(
        reqwest::Method::POST,
        format!("{console}/api/invites"),
        ("x-loca-console", "1"),
        Some(serde_json::json!({ "loca": "new-loca", "name": "ada" })),
    )
    .await;
    assert!(
        status < 300,
        "a name the rule takes still opens: {status} {body}"
    );
}

/// v0.18 and earlier accepted the name `lobby`. On upgrade such a loca would
/// become the Building's Lobby room: every member could read it and its
/// Operator would moderate the whole Building. The server refuses to start
/// until `ROOM_RENAME=lobby:<new>` gives it a new id, and no rename may take
/// the Lobby's id.
#[tokio::test]
async fn a_loca_named_lobby_from_before_the_lobby_room_must_be_renamed() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("legacy-lobby.db");
    let (port, guard) = closed_building(&db, &[]).await;
    let base = format!("http://127.0.0.1:{port}");
    let (status, body) = create(&base, "oldlobby").await;
    assert_eq!(status, 201, "{body}");
    let (status, body) = send(
        reqwest::Method::POST,
        format!("{base}/members"),
        ADMIN,
        Some(serde_json::json!({ "name": "alice", "kind": "user" })),
    )
    .await;
    assert_eq!(status, 201, "{body}");
    let (status, body) = send(
        reqwest::Method::POST,
        format!("{base}/rooms/oldlobby/invites"),
        ADMIN,
        Some(serde_json::json!({ "name": "alice" })),
    )
    .await;
    assert!((200..300).contains(&status), "{body}");
    let (status, body) = send(
        reqwest::Method::POST,
        format!("{base}/rooms/oldlobby/messages"),
        ADMIN,
        Some(serde_json::json!({ "sender": "master", "sender_type": "user", "text": "PRIVATE plan" })),
    )
    .await;
    assert!((200..300).contains(&status), "{body}");
    drop(guard);
    wait_for_server_shutdown(port).await;

    // What an older server left behind: that loca under the id `lobby`.
    {
        let c = rusqlite::Connection::open(&db).unwrap();
        let tables: Vec<String> = c
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for table in tables {
            let has_room = c
                .prepare(&format!(
                    "SELECT 1 FROM pragma_table_info('{table}') WHERE name = 'room'"
                ))
                .unwrap()
                .exists([])
                .unwrap();
            if has_room {
                c.execute(
                    &format!("UPDATE {table} SET room = 'lobby' WHERE room = 'oldlobby'"),
                    [],
                )
                .unwrap();
            }
        }
    }

    let env = |extra: Option<&str>| {
        let mut env = vec![
            ("DB_PATH", db.to_str().unwrap().to_string()),
            ("REQUIRE_INVITE", "1".to_string()),
        ];
        if let Some(rename) = extra {
            env.push(("ROOM_RENAME", rename.to_string()));
        }
        env
    };
    let out = refuses_to_start(&env(None)).await;
    assert!(out.contains("ROOM_RENAME=lobby:"), "{out}");
    let out = refuses_to_start(&env(Some("lobby:Lobby"))).await;
    assert!(out.contains("Lobby room"), "{out}");

    // Renamed, it starts; the loca keeps its words under the new id and the
    // Lobby room starts empty.
    let (port, _guard) = closed_building(&db, &[("ROOM_RENAME", "lobby:old-lobby".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let (status, body) = send(
        reqwest::Method::GET,
        format!("{base}/rooms/old-lobby/messages"),
        ADMIN,
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert!(body.to_string().contains("PRIVATE plan"), "{body}");
    let (status, body) = send(
        reqwest::Method::GET,
        format!("{base}/rooms/lobby/messages"),
        ADMIN,
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert!(!body.to_string().contains("PRIVATE plan"), "{body}");
}
