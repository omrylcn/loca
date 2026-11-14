//! Adversarial review gates: assertions enforce denial, not just printed probes.
use super::*;

const ROOT: &str = "review-fixture-root";

async fn moderation_fixture(base: &str) -> (String, String) {
    create_test_room(
        base.rsplit(':').next().unwrap().parse().unwrap(),
        ROOT,
        "mod-review",
    )
    .await;
    let (status, smaster) = http(
        "POST",
        base,
        "/smasters",
        ("x-admin-token", ROOT),
        serde_json::json!({"name":"review-deputy"}),
    )
    .await;
    assert_eq!(status, 201);
    let member = member_session(base, "review-participant").await;
    assert_eq!(
        http(
            "POST",
            base,
            "/rooms/mod-review/invites",
            ("x-admin-token", ROOT),
            serde_json::json!({"name":"review-participant"})
        )
        .await
        .0,
        200
    );
    (smaster["token"].as_str().unwrap().into(), member)
}

async fn moderate_as(base: &str, credential: (&str, &str), action: &str) -> u16 {
    http(
        "POST",
        base,
        "/rooms/mod-review/moderate",
        credential,
        serde_json::json!({"name":"review-participant","action":action}),
    )
    .await
    .0
}

#[tokio::test]
async fn moderation_preserves_superior_decisions_across_restart() {
    let (dir, port, guard) = setup().await;
    let base = format!("http://127.0.0.1:{port}");
    let (smaster, _) = moderation_fixture(&base).await;
    assert_eq!(
        moderate_as(&base, ("x-admin-token", ROOT), "mute").await,
        200
    );
    for action in ["unmute", "mute"] {
        assert_eq!(
            moderate_as(&base, ("x-admin-token", &smaster), action).await,
            403,
            "a lower rank must neither remove nor overwrite a superior restriction"
        );
    }
    assert_eq!(
        moderate_as(&base, ("x-admin-token", ROOT), "unmute").await,
        200
    );
    assert_eq!(
        moderate_as(&base, ("x-admin-token", &smaster), "mute").await,
        200
    );
    assert_eq!(
        moderate_as(&base, ("x-admin-token", &smaster), "unmute").await,
        200
    );
    assert_eq!(
        moderate_as(&base, ("x-admin-token", ROOT), "ban").await,
        200
    );
    drop(guard);
    let (port, _guard) = spawn_server_env(
        ROOT,
        &[
            (
                "DB_PATH",
                dir.path().join("review.db").to_string_lossy().into(),
            ),
            ("REQUIRE_INVITE", "1".into()),
            ("REQUIRE_SESSIONS", "1".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    for action in ["unban", "ban"] {
        assert_eq!(
            moderate_as(&base, ("x-admin-token", &smaster), action).await,
            403
        );
    }
    assert_eq!(
        moderate_as(&base, ("x-admin-token", ROOT), "unban").await,
        200
    );
}

#[tokio::test]
async fn moderation_and_opaque_revoke_cannot_end_a_superior_davet() {
    let (_dir, port, _guard) = setup().await;
    let base = format!("http://127.0.0.1:{port}");
    let (smaster, member) = moderation_fixture(&base).await;
    for action in ["kick", "release", "ban"] {
        assert_eq!(
            moderate_as(&base, ("x-admin-token", &smaster), action).await,
            403
        );
    }
    let (status, invites) = http(
        "GET",
        &base,
        "/rooms/mod-review/invites",
        ("x-admin-token", ROOT),
        Value::Null,
    )
    .await;
    assert_eq!(status, 200);
    let reference = invites[0]["id"].as_str().unwrap();
    assert_eq!(
        http(
            "DELETE",
            &base,
            &format!("/rooms/mod-review/invites/{reference}"),
            ("x-admin-token", &smaster),
            Value::Null
        )
        .await
        .0,
        403
    );
    assert_eq!(
        http(
            "GET",
            &base,
            "/rooms/mod-review/messages",
            ("x-session-token", &member),
            Value::Null
        )
        .await
        .0,
        200,
        "a rejected moderation must preserve the participant's access"
    );
    assert_eq!(
        moderate_as(&base, ("x-admin-token", ROOT), "kick").await,
        200
    );
}

#[tokio::test]
async fn smaster_session_issues_and_releases_its_own_davet_without_master_provenance() {
    let (_dir, port, _guard) = setup().await;
    let base = format!("http://127.0.0.1:{port}");
    let (smaster, _) = moderation_fixture(&base).await;
    assert_eq!(
        moderate_as(&base, ("x-admin-token", ROOT), "release").await,
        200
    );
    let (status, session) = http(
        "POST",
        &base,
        "/sessions",
        ("x-admin-token", &smaster),
        serde_json::json!({"name":"review-deputy","kind":"user"}),
    )
    .await;
    assert_eq!(status, 201);
    let session = session["session_token"].as_str().unwrap();
    let (status, invite) = http(
        "POST",
        &base,
        "/rooms/mod-review/invites",
        ("x-session-token", session),
        serde_json::json!({"name":"review-participant"}),
    )
    .await;
    assert_eq!(status, 200);
    assert!(invite["issued_by"]
        .as_str()
        .unwrap()
        .starts_with("smaster:"));
    assert_eq!(
        moderate_as(&base, ("x-session-token", session), "release").await,
        200
    );
}

#[tokio::test]
async fn legacy_moderation_without_provenance_migrates_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("legacy.db");
    {
        let connection = rusqlite::Connection::open(&db).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE bans (room TEXT NOT NULL, name TEXT NOT NULL,
            kind TEXT NOT NULL, at INTEGER NOT NULL, PRIMARY KEY(room,name,kind));
            INSERT INTO bans VALUES ('mod-review','review-participant','mute',1);",
            )
            .unwrap();
    }
    let (port, _guard) = spawn_server_env(ROOT, &[("DB_PATH", db.to_string_lossy().into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let (status, smaster) = http(
        "POST",
        &base,
        "/smasters",
        ("x-admin-token", ROOT),
        serde_json::json!({"name":"legacy-deputy"}),
    )
    .await;
    assert_eq!(status, 201);
    assert_eq!(
        moderate_as(
            &base,
            ("x-admin-token", smaster["token"].as_str().unwrap()),
            "unmute"
        )
        .await,
        403
    );
    assert_eq!(
        moderate_as(&base, ("x-admin-token", ROOT), "unmute").await,
        200
    );
}

#[tokio::test]
async fn failed_ban_rolls_back_restriction_and_invitation_together() {
    let (dir, port, _guard) = setup().await;
    let base = format!("http://127.0.0.1:{port}");
    let (_, member) = moderation_fixture(&base).await;
    let connection = rusqlite::Connection::open(dir.path().join("review.db")).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER deny_invite_cutoff BEFORE UPDATE ON invites
        BEGIN SELECT RAISE(ABORT,'fixture failure'); END;",
        )
        .unwrap();
    assert_eq!(
        moderate_as(&base, ("x-admin-token", ROOT), "ban").await,
        503
    );
    let (status, state) = http(
        "GET",
        &base,
        "/rooms/mod-review/moderate",
        ("x-admin-token", ROOT),
        Value::Null,
    )
    .await;
    assert_eq!(status, 200);
    assert!(state["banned"].as_array().unwrap().is_empty());
    assert_eq!(
        http(
            "GET",
            &base,
            "/rooms/mod-review/messages",
            ("x-session-token", &member),
            Value::Null
        )
        .await
        .0,
        200
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM bans", [], |row| row.get::<_, u64>(0))
            .unwrap(),
        0
    );
    connection
        .execute_batch("DROP TRIGGER deny_invite_cutoff")
        .unwrap();
    assert_eq!(
        moderate_as(&base, ("x-admin-token", ROOT), "ban").await,
        200
    );
}

#[tokio::test]
async fn restricted_profile_cannot_rename_around_moderation() {
    let (_dir, port, _guard) = setup().await;
    let base = format!("http://127.0.0.1:{port}");
    let (smaster, _) = moderation_fixture(&base).await;
    assert_eq!(
        http(
            "POST",
            &base,
            "/rooms/mod-review/moderate",
            ("x-admin-token", ROOT),
            serde_json::json!({"name":"review-deputy","action":"mute"})
        )
        .await
        .0,
        200
    );
    assert_eq!(
        http(
            "PUT",
            &base,
            "/profile",
            ("x-admin-token", &smaster),
            serde_json::json!({"display_name":"renamed-deputy"})
        )
        .await
        .0,
        403
    );
    assert_eq!(
        http(
            "POST",
            &base,
            "/rooms/mod-review/moderate",
            ("x-admin-token", ROOT),
            serde_json::json!({"name":"review-deputy","action":"unmute"})
        )
        .await
        .0,
        200
    );
    assert_eq!(
        http(
            "PUT",
            &base,
            "/profile",
            ("x-admin-token", &smaster),
            serde_json::json!({"display_name":"renamed-deputy"})
        )
        .await
        .0,
        200
    );
}

async fn http(
    method: &str,
    base: &str,
    path: &str,
    header: (&str, &str),
    body: Value,
) -> (u16, Value) {
    let response = reqwest::Client::new()
        .request(method.parse().unwrap(), format!("{base}{path}"))
        .header(header.0, header.1)
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    let text = response.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

async fn setup() -> (tempfile::TempDir, u16, ServerGuard) {
    let dir = tempfile::tempdir().unwrap();
    let (port, guard) = spawn_server_env(
        ROOT,
        &[
            (
                "DB_PATH",
                dir.path().join("review.db").to_string_lossy().into(),
            ),
            ("REQUIRE_INVITE", "1".into()),
            ("REQUIRE_SESSIONS", "1".into()),
        ],
    )
    .await;
    (dir, port, guard)
}

async fn member_session(base: &str, name: &str) -> String {
    let (status, member) = http(
        "POST",
        base,
        "/members",
        ("x-admin-token", ROOT),
        serde_json::json!({"name":name,"kind":"user"}),
    )
    .await;
    assert_eq!(status, 201);
    let (status, session) = http(
        "POST",
        base,
        "/sessions",
        ("x-room-token", member["token"].as_str().unwrap()),
        serde_json::json!({"name":name,"kind":"user"}),
    )
    .await;
    assert_eq!(status, 201);
    session["session_token"].as_str().unwrap().into()
}

async fn private_pair(base: &str) -> (String, String) {
    let alice = member_session(base, "review-alice").await;
    member_session(base, "review-bob").await;
    http(
        "POST",
        base,
        "/rooms",
        ("x-admin-token", ROOT),
        serde_json::json!({"name":"shared"}),
    )
    .await;
    for name in ["review-alice", "review-bob"] {
        assert_eq!(
            http(
                "POST",
                base,
                "/rooms/shared/invites",
                ("x-admin-token", ROOT),
                serde_json::json!({"name":name})
            )
            .await
            .0,
            200
        );
    }
    let (status, pair) = http(
        "POST",
        base,
        "/birebirs",
        ("x-session-token", &alice),
        serde_json::json!({"name":"review-bob"}),
    )
    .await;
    assert_eq!(status, 201);
    (alice, pair["birebir"]["room"].as_str().unwrap().into())
}

async fn scoped_session(base: &str) -> String {
    http(
        "POST",
        base,
        "/rooms",
        ("x-admin-token", ROOT),
        serde_json::json!({"name":"unrelated"}),
    )
    .await;
    let (status, invite) = http(
        "POST",
        base,
        "/rooms/unrelated/call",
        ("x-admin-token", ROOT),
        serde_json::json!({"name":"review-bob"}),
    )
    .await;
    assert_eq!(status, 201);
    let (status, session) = http(
        "POST",
        base,
        "/sessions",
        ("x-room-token", invite["token"].as_str().unwrap()),
        serde_json::json!({"name":"review-bob","kind":"user"}),
    )
    .await;
    assert_eq!(status, 201);
    session["session_token"].as_str().unwrap().into()
}

#[tokio::test]
async fn scoped_davet_does_not_open_other_rooms_or_private_pairs() {
    let (_dir, port, _guard) = setup().await;
    let base = format!("http://127.0.0.1:{port}");
    let (_, room) = private_pair(&base).await;
    let scoped = scoped_session(&base).await;
    let header = ("x-session-token", scoped.as_str());
    assert_eq!(
        http("GET", &base, "/profile?room=unrelated", header, Value::Null)
            .await
            .0,
        200
    );
    assert_eq!(
        http("GET", &base, "/profile?room=shared", header, Value::Null)
            .await
            .0,
        401
    );
    assert_eq!(
        http(
            "GET",
            &base,
            "/rooms/unrelated/messages",
            header,
            Value::Null
        )
        .await
        .0,
        200
    );
    for path in [
        "/rooms/shared/messages".into(),
        format!("/rooms/{room}/messages"),
    ] {
        assert!(
            http("GET", &base, &path, header, Value::Null).await.0 >= 400,
            "a room davet must not become another room's authority"
        );
    }
    let (status, pairs) = http("GET", &base, "/birebirs", header, Value::Null).await;
    assert!(status >= 400 || pairs.as_array().is_some_and(Vec::is_empty));
    assert!(
        http(
            "POST",
            &base,
            "/birebirs",
            header,
            serde_json::json!({"name":"review-alice"})
        )
        .await
        .0 >= 400
    );
}

#[tokio::test]
async fn scoped_davet_cannot_mint_or_manage_identity_credentials() {
    let (_dir, port, _guard) = setup().await;
    let base = format!("http://127.0.0.1:{port}");
    private_pair(&base).await;
    let scoped = scoped_session(&base).await;
    let header = ("x-session-token", scoped.as_str());
    assert!(
        http(
            "POST",
            &base,
            "/profile/credentials",
            header,
            serde_json::json!({"label":"must-not-be-issued"})
        )
        .await
        .0 >= 400
    );
    assert!(
        http("GET", &base, "/profile/credentials", header, Value::Null)
            .await
            .0
            >= 400
    );
}

#[tokio::test]
async fn encoded_private_room_cannot_be_invited_or_managed_by_an_outside_rank() {
    let (_dir, port, _guard) = setup().await;
    let base = format!("http://127.0.0.1:{port}");
    let (alice, room) = private_pair(&base).await;
    member_session(&base, "review-outsider").await;
    for encoded in [
        room.clone(),
        room.replacen("bb-", "%62b-", 1),
        room.replacen("bb-", "bb%2D", 1),
    ] {
        for suffix in ["call", "invites"] {
            assert!(
                http(
                    "POST",
                    &base,
                    &format!("/rooms/{encoded}/{suffix}"),
                    ("x-admin-token", ROOT),
                    serde_json::json!({"name":"review-outsider"})
                )
                .await
                .0 >= 400,
                "a third person must not receive a private pair's davet"
            );
        }
        assert!(
            http(
                "PUT",
                &base,
                &format!("/rooms/{encoded}/settings"),
                ("x-admin-token", ROOT),
                serde_json::json!({"rate_limit":123})
            )
            .await
            .0 >= 400
        );
        assert_eq!(
            http(
                "GET",
                &base,
                &format!("/rooms/{encoded}/messages"),
                ("x-session-token", &alice),
                Value::Null
            )
            .await
            .0,
            200
        );
    }
}

#[tokio::test]
async fn disabled_extensions_are_fenced_under_the_decoded_room_name() {
    let (_dir, port, _guard) = setup().await;
    let base = format!("http://127.0.0.1:{port}");
    for (name, encoded) in [("alpha", "%61lpha"), ("şirket", "%C5%9Firket")] {
        assert_eq!(
            http(
                "POST",
                &base,
                "/rooms",
                ("x-admin-token", ROOT),
                serde_json::json!({"name":name,"extensions":[]})
            )
            .await
            .0,
            201
        );
        for suffix in ["notes", "goals", "tasks", "wiki", "journal"] {
            assert_eq!(
                http(
                    "GET",
                    &base,
                    &format!("/rooms/{encoded}/{suffix}"),
                    ("x-admin-token", ROOT),
                    Value::Null
                )
                .await
                .0,
                403,
                "disabled extension {suffix} must remain disabled for {name}"
            );
        }
    }
}

#[tokio::test]
async fn renaming_a_group_room_preserves_its_access_boundary() {
    let (dir, port, guard) = setup().await;
    let base = format!("http://127.0.0.1:{port}");
    let (_, deputy) = http(
        "POST",
        &base,
        "/smasters",
        ("x-admin-token", ROOT),
        serde_json::json!({"name":"review-deputy"}),
    )
    .await;
    let key = deputy["token"].as_str().unwrap();
    let (_, group) = http(
        "POST",
        &base,
        "/groups",
        ("x-admin-token", ROOT),
        serde_json::json!({"name":"private-team"}),
    )
    .await;
    assert_eq!(
        http(
            "POST",
            &base,
            "/rooms",
            ("x-admin-token", ROOT),
            serde_json::json!({"name":"private-before","group":group["id"]})
        )
        .await
        .0,
        201
    );
    assert_eq!(
        http(
            "GET",
            &base,
            "/rooms/private-before/messages",
            ("x-admin-token", key),
            Value::Null
        )
        .await
        .0,
        404
    );
    drop(guard);
    let (port, _guard) = spawn_server_env(
        ROOT,
        &[
            (
                "DB_PATH",
                dir.path().join("review.db").to_string_lossy().into(),
            ),
            ("REQUIRE_INVITE", "1".into()),
            ("ROOM_RENAME", "private-before:private-after".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    assert_eq!(
        http(
            "GET",
            &base,
            "/rooms/private-after/messages",
            ("x-admin-token", key),
            Value::Null
        )
        .await
        .0,
        404
    );
    assert_eq!(
        http(
            "GET",
            &base,
            "/rooms/private-after/messages",
            ("x-admin-token", ROOT),
            Value::Null
        )
        .await
        .0,
        200
    );
}

#[tokio::test]
async fn runtime_ids_do_not_disclose_hidden_group_rooms() {
    let (_dir, port, _guard) = setup().await;
    let base = format!("http://127.0.0.1:{port}");
    let (_, deputy) = http(
        "POST",
        &base,
        "/smasters",
        ("x-admin-token", ROOT),
        serde_json::json!({"name":"outside-deputy"}),
    )
    .await;
    let key = deputy["token"].as_str().unwrap();
    let (_, bot) = http(
        "POST",
        &base,
        "/members",
        ("x-admin-token", ROOT),
        serde_json::json!({"name":"health-bot","kind":"agent"}),
    )
    .await;
    let (_, group) = http(
        "POST",
        &base,
        "/groups",
        ("x-admin-token", ROOT),
        serde_json::json!({"name":"hidden-team"}),
    )
    .await;
    let (_, profiles) = http(
        "GET",
        &base,
        "/profiles",
        ("x-admin-token", ROOT),
        Value::Null,
    )
    .await;
    let principal = profiles
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["display_name"] == "health-bot")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    assert_eq!(
        http(
            "PUT",
            &base,
            &format!(
                "/groups/{}/members/{principal}",
                group["id"].as_str().unwrap()
            ),
            ("x-admin-token", ROOT),
            Value::Null
        )
        .await
        .0,
        200
    );
    assert_eq!(
        http(
            "POST",
            &base,
            "/rooms",
            ("x-admin-token", ROOT),
            serde_json::json!({"name":"health-secret","group":group["id"]})
        )
        .await
        .0,
        201
    );
    assert_eq!(http("POST", &base, "/runtime/health", ("x-room-token", bot["token"].as_str().unwrap()),
        serde_json::json!({"wake":"OK","ack":"OK","attention_id":"attention:health-secret:goal:3","delivery_id":"health-secret:10"})).await.0, 200);
    let (_, outside) = http(
        "GET",
        &base,
        "/residents",
        ("x-admin-token", key),
        Value::Null,
    )
    .await;
    assert!(
        !outside.to_string().contains("health-secret"),
        "opaque-looking runtime ids must not leak a hidden room"
    );
    let (_, master) = http(
        "GET",
        &base,
        "/residents",
        ("x-admin-token", ROOT),
        Value::Null,
    )
    .await;
    assert!(master.to_string().contains("health-secret"));
}

#[tokio::test]
async fn hidden_group_and_missing_room_have_the_same_websocket_refusal() {
    let (_dir, port, _guard) = setup().await;
    let base = format!("http://127.0.0.1:{port}");
    let (_, deputy) = http(
        "POST",
        &base,
        "/smasters",
        ("x-admin-token", ROOT),
        serde_json::json!({"name":"ws-deputy"}),
    )
    .await;
    let key = deputy["token"].as_str().unwrap();
    let (_, group) = http(
        "POST",
        &base,
        "/groups",
        ("x-admin-token", ROOT),
        serde_json::json!({"name":"ws-private"}),
    )
    .await;
    http(
        "POST",
        &base,
        "/rooms",
        ("x-admin-token", ROOT),
        serde_json::json!({"name":"ws-secret","group":group["id"]}),
    )
    .await;
    let mut statuses = Vec::new();
    for room in ["ws-secret", "ws-absent"] {
        let mut req = format!("ws://127.0.0.1:{port}/ws?room={room}&name=ws-deputy&type=user")
            .into_client_request()
            .unwrap();
        req.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            HeaderValue::from_str(&format!("loca.v1, loca.admin.{key}")).unwrap(),
        );
        match tokio_tungstenite::connect_async(req).await {
            Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
                statuses.push(response.status())
            }
            _ => panic!("an outside rank must not enter either room"),
        }
    }
    assert_eq!(statuses[0], statuses[1]);
}

#[tokio::test]
async fn console_cannot_bypass_private_pair_admission() {
    let dir = tempfile::tempdir().unwrap();
    let socket = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let console_port = socket.local_addr().unwrap().port();
    drop(socket);
    let (port, _guard) = spawn_server_env(
        ROOT,
        &[
            (
                "DB_PATH",
                dir.path().join("console.db").to_string_lossy().into(),
            ),
            ("REQUIRE_SESSIONS", "1".into()),
            ("ADMIN_CONSOLE_PORT", console_port.to_string()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let (_, room) = private_pair(&base).await;
    member_session(&base, "console-outsider").await;
    let console = format!("http://127.0.0.1:{console_port}");
    let (status, _) = http(
        "POST",
        &console,
        "/api/invites",
        ("x-loca-console", "1"),
        serde_json::json!({"loca":room,"name":"console-outsider","kind":"user"}),
    )
    .await;
    assert_eq!(
        status, 403,
        "the shared admission rule must also protect the console"
    );
}

#[tokio::test]
async fn legacy_third_party_davet_and_derived_session_cannot_open_a_private_pair() {
    let (dir, port, guard) = setup().await;
    let base = format!("http://127.0.0.1:{port}");
    let (_, room) = private_pair(&base).await;
    member_session(&base, "legacy-outsider").await;
    let (_, invitation) = http(
        "POST",
        &base,
        "/rooms/shared/invites",
        ("x-admin-token", ROOT),
        serde_json::json!({"name":"legacy-outsider"}),
    )
    .await;
    let token = invitation["token"].as_str().unwrap();
    drop(guard);
    // Model a davet persisted by the old, vulnerable admission path.
    let connection = rusqlite::Connection::open(dir.path().join("review.db")).unwrap();
    connection
        .execute(
            "UPDATE invites SET room = ?1 WHERE token = ?2",
            rusqlite::params![room, token],
        )
        .unwrap();
    drop(connection);
    let (port, _guard) = spawn_server_env(
        ROOT,
        &[
            (
                "DB_PATH",
                dir.path().join("review.db").to_string_lossy().into(),
            ),
            ("REQUIRE_INVITE", "1".into()),
            ("REQUIRE_SESSIONS", "1".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let path = format!("/rooms/{room}/messages");
    assert!(
        http("GET", &base, &path, ("x-room-token", token), Value::Null)
            .await
            .0
            >= 400
    );
    let (status, session) = http(
        "POST",
        &base,
        "/sessions",
        ("x-room-token", token),
        serde_json::json!({"name":"legacy-outsider","kind":"user"}),
    )
    .await;
    if status < 300 {
        assert!(
            http(
                "GET",
                &base,
                &path,
                (
                    "x-session-token",
                    session["session_token"].as_str().unwrap()
                ),
                Value::Null
            )
            .await
            .0 >= 400
        );
    }
}
