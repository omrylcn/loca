//! Notifications (docs/concepts.md "Notifications"): a Building-level
//! announcement. Who may send to whom, frozen audiences, per-person reads,
//! the Lobby frame for agents, limits, and a restart.
use super::*;

const MASTER: &str = "ntf-master-key";

async fn send(
    method: reqwest::Method,
    url: String,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> (u16, Value) {
    let mut req = reqwest::Client::new().request(method, url);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    if let Some(body) = body {
        req = req.json(&body);
    }
    let r = req.send().await.unwrap();
    let status = r.status().as_u16();
    let text = r.text().await.unwrap_or_default();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

/// Who is asking: a browser session or a personal key (an agent's mb_).
#[derive(Clone, Copy)]
enum Who<'a> {
    Session(&'a str),
    Key(&'a str),
}

impl<'a> Who<'a> {
    fn header(self) -> [(&'static str, &'a str); 1] {
        match self {
            Who::Session(token) => [("x-session-token", token)],
            Who::Key(token) => [("x-admin-token", token)],
        }
    }
}

async fn get(url: String, who: Who<'_>) -> (u16, Value) {
    send(reqwest::Method::GET, url, &who.header(), None).await
}

async fn post(url: String, who: Who<'_>, body: Value) -> (u16, Value) {
    send(reqwest::Method::POST, url, &who.header(), Some(body)).await
}

async fn session(base: &str, header: (&str, &str), name: &str, kind: &str) -> String {
    let (status, v) = send(
        reqwest::Method::POST,
        format!("{base}/sessions"),
        &[header],
        Some(serde_json::json!({ "name": name, "kind": kind })),
    )
    .await;
    assert!(status < 300, "session for {name}: {status} {v}");
    v["session_token"].as_str().unwrap().to_string()
}

async fn master_session(base: &str) -> String {
    let (_, pairing) = send(
        reqwest::Method::POST,
        format!("{base}/pairings?ttl_hours=1"),
        &[("x-admin-token", MASTER)],
        None,
    )
    .await;
    session(
        base,
        ("x-pairing-code", pairing["pairing_code"].as_str().unwrap()),
        "",
        "user",
    )
    .await
}

async fn smaster(base: &str, name: &str) -> String {
    let (status, v) = send(
        reqwest::Method::POST,
        format!("{base}/smasters"),
        &[("x-admin-token", MASTER)],
        Some(serde_json::json!({ "name": name })),
    )
    .await;
    assert!(status < 300, "smaster {name}: {status} {v}");
    let key = v["token"].as_str().unwrap().to_string();
    session(base, ("x-admin-token", &key), name, "user").await
}

async fn member(base: &str, name: &str, kind: &str) -> String {
    let (status, v) = send(
        reqwest::Method::POST,
        format!("{base}/members"),
        &[("x-admin-token", MASTER)],
        Some(serde_json::json!({ "name": name, "kind": kind })),
    )
    .await;
    assert!(status < 300, "member {name}: {status} {v}");
    v["token"].as_str().unwrap().to_string()
}

async fn principal_of(base: &str, name: &str) -> String {
    let (_, profiles) = send(
        reqwest::Method::GET,
        format!("{base}/profiles"),
        &[("x-admin-token", MASTER)],
        None,
    )
    .await;
    profiles
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["display_name"] == name)
        .unwrap_or_else(|| panic!("no profile named {name}"))["id"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn found_group(base: &str, by: &str, name: &str, members: &[&str]) -> String {
    let (status, group) = post(
        format!("{base}/groups"),
        Who::Session(by),
        serde_json::json!({ "name": name }),
    )
    .await;
    assert_eq!(status, 201, "{group}");
    let gid = group["id"].as_str().unwrap().to_string();
    for person in members {
        add_to_group(base, by, &gid, person).await;
    }
    gid
}

async fn add_to_group(base: &str, by: &str, gid: &str, person: &str) {
    let id = principal_of(base, person).await;
    let (status, v) = send(
        reqwest::Method::PUT,
        format!("{base}/groups/{gid}/members/{id}"),
        &[("x-session-token", by)],
        None,
    )
    .await;
    assert_eq!(status, 200, "add {person}: {v}");
}

async fn notify(base: &str, who: Who<'_>, title: &str, audience: Value) -> (u16, Value) {
    post(
        format!("{base}/notifications"),
        who,
        serde_json::json!({ "title": title, "body": format!("{title} body"), "audience": audience }),
    )
    .await
}

/// (titles newest first, unread count)
async fn inbox(base: &str, who: Who<'_>) -> (Vec<String>, u64) {
    let (status, v) = get(format!("{base}/notifications"), who).await;
    assert_eq!(status, 200, "{v}");
    let titles = v["notifications"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["title"].as_str().unwrap().to_string())
        .collect();
    (titles, v["unread"].as_u64().unwrap())
}

async fn lobby(
    port: u16,
    key: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    connect_ws_protocols(
        format!("ws://127.0.0.1:{port}/lobby/ws"),
        &["loca.v1".into(), format!("loca.membership.{key}")],
    )
    .await
}

async fn building(db: &std::path::Path) -> (u16, ServerGuard) {
    spawn_server_env(
        MASTER,
        &[
            ("DB_PATH", db.to_str().unwrap().into()),
            ("REQUIRE_SESSIONS", "1".into()),
            ("REQUIRE_INVITE", "1".into()),
        ],
    )
    .await
}

#[tokio::test]
async fn the_master_notifies_everyone_and_reads_are_per_person() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("ntf-all.db");
    let (port, _guard) = building(&db).await;
    let base = format!("http://127.0.0.1:{port}");

    let master = master_session(&base).await;
    let kaan = smaster(&base, "kaan").await;
    let ali_key = member(&base, "ali", "user").await;
    let veli_key = member(&base, "veli", "user").await;
    let bot_key = member(&base, "bot", "agent").await;
    let ali = session(&base, ("x-room-token", &ali_key), "ali", "user").await;

    let (status, sent) = notify(
        &base,
        Who::Session(&master),
        "Water off at noon",
        serde_json::json!({ "all": true }),
    )
    .await;
    assert_eq!(status, 201, "{sent}");
    let id = sent["id"].as_i64().unwrap();
    assert_eq!(
        sent["recipients"], 4,
        "kaan, ali, veli, bot — not the sender"
    );

    // Every active member, Smaster and agent has it, unread.
    for who in [
        Who::Session(&kaan),
        Who::Session(&ali),
        Who::Key(&veli_key),
        Who::Key(&bot_key),
    ] {
        let (titles, unread) = inbox(&base, who).await;
        assert_eq!(titles, vec!["Water off at noon".to_string()]);
        assert_eq!(unread, 1);
    }
    let (_, v) = get(format!("{base}/notifications"), Who::Session(&ali)).await;
    let n = &v["notifications"][0];
    assert_eq!(n["from"], "Master");
    assert_eq!(n["body"], "Water off at noon body");
    assert_eq!(n["audience"]["all"], true);
    assert_eq!(n["read"], false);
    // The sender does not notify itself.
    assert_eq!(inbox(&base, Who::Session(&master)).await.0.len(), 0);

    // Read is per person.
    let (status, _) = post(
        format!("{base}/notifications/{id}/read"),
        Who::Session(&ali),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, 204);
    assert_eq!(inbox(&base, Who::Session(&ali)).await.1, 0);
    assert_eq!(inbox(&base, Who::Key(&veli_key)).await.1, 1);
    let (_, v) = get(format!("{base}/notifications"), Who::Session(&ali)).await;
    assert_eq!(v["notifications"][0]["read"], true);

    // Read-all is per person too.
    let (status, v) = post(
        format!("{base}/notifications/read-all"),
        Who::Key(&veli_key),
        serde_json::json!({}),
    )
    .await;
    assert_eq!((status, v["marked"].as_u64()), (200, Some(1)));
    assert_eq!(inbox(&base, Who::Key(&veli_key)).await.1, 0);
    assert_eq!(inbox(&base, Who::Session(&kaan)).await.1, 1);

    // The sender sees counts, never names.
    let (status, mine) = get(format!("{base}/notifications/sent"), Who::Session(&master)).await;
    assert_eq!(status, 200);
    assert_eq!(mine[0]["recipients"], 4);
    assert_eq!(mine[0]["read"], 2);
    assert!(
        !mine.to_string().contains("ali"),
        "who read it stays private"
    );

    // Nobody marks a notification they did not receive.
    for missing in [format!("{id}"), "999999".into(), "nope".into()] {
        let (status, _) = post(
            format!("{base}/notifications/{missing}/read"),
            Who::Session(&master),
            serde_json::json!({}),
        )
        .await;
        assert_eq!(status, 404, "master never received {missing}");
    }

    // `since` returns only newer ones (the browser poll).
    let (_, v) = get(
        format!("{base}/notifications?since={id}"),
        Who::Session(&kaan),
    )
    .await;
    assert_eq!(v["notifications"].as_array().unwrap().len(), 0);
    assert_eq!(v["unread"], 1);

    // Members and agents never send; a Smaster without a group cannot; only
    // the Master notifies everyone.
    let everyone = serde_json::json!({ "all": true });
    assert_eq!(
        notify(&base, Who::Session(&ali), "x", everyone.clone())
            .await
            .0,
        403
    );
    assert_eq!(
        notify(&base, Who::Key(&bot_key), "x", everyone.clone())
            .await
            .0,
        403
    );
    assert_eq!(
        notify(
            &base,
            Who::Key(&bot_key),
            "x",
            serde_json::json!({ "groups": ["grp_x"] })
        )
        .await
        .0,
        403
    );
    assert_eq!(
        notify(&base, Who::Session(&kaan), "x", everyone.clone())
            .await
            .0,
        403
    );
    let (status, v) = notify(
        &base,
        Who::Session(&kaan),
        "x",
        serde_json::json!({ "groups": ["grp_x"] }),
    )
    .await;
    assert_eq!(status, 403, "a Smaster with no group: {v}");
    assert!(v.to_string().contains("no group"));
    // Without an audience: a rank hears 400, a member still 403.
    let (status, _) = post(
        format!("{base}/notifications"),
        Who::Session(&master),
        serde_json::json!({ "title": "x" }),
    )
    .await;
    assert_eq!(status, 400);
    let (status, _) = post(
        format!("{base}/notifications"),
        Who::Session(&ali),
        serde_json::json!({ "title": "x" }),
    )
    .await;
    assert_eq!(status, 403);
    assert_eq!(
        get(format!("{base}/notifications/sent"), Who::Session(&ali))
            .await
            .0,
        403
    );
    let (_, opts) = get(
        format!("{base}/notifications/audiences"),
        Who::Session(&ali),
    )
    .await;
    assert_eq!(opts["can_send"], false);
    let (_, opts) = get(
        format!("{base}/notifications/audiences"),
        Who::Session(&kaan),
    )
    .await;
    assert_eq!(opts["can_send"], false, "{opts}");
    let (_, opts) = get(
        format!("{base}/notifications/audiences"),
        Who::Session(&master),
    )
    .await;
    assert_eq!(
        (opts["can_send"].clone(), opts["everyone"].clone()),
        (Value::Bool(true), Value::Bool(true))
    );
    // Anonymous callers get nothing.
    let (status, _) = send(
        reqwest::Method::GET,
        format!("{base}/notifications"),
        &[],
        None,
    )
    .await;
    assert_eq!(status, 401);
}

#[tokio::test]
async fn limits_are_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("ntf-limits.db");
    let (port, _guard) = building(&db).await;
    let base = format!("http://127.0.0.1:{port}");
    let master = master_session(&base).await;
    let ali_key = member(&base, "ali", "user").await;
    let all = serde_json::json!({ "all": true });

    let cases: Vec<(String, String, u16)> = vec![
        ("".into(), "b".into(), 400),
        ("   ".into(), "b".into(), 400),
        ("a".repeat(121), "b".into(), 400),
        ("two\nlines".into(), "b".into(), 400),
        ("ok".into(), "b".repeat(2001), 400),
        ("ok".into(), "bell\u{7}".into(), 400),
        ("é".repeat(120), "b".repeat(2000), 201),
        (
            "plain".into(),
            "line one\nline two\t<b>not html</b>".into(),
            201,
        ),
    ];
    for (title, body, want) in cases {
        let (status, v) = post(
            format!("{base}/notifications"),
            Who::Session(&master),
            serde_json::json!({ "title": title, "body": body, "audience": all }),
        )
        .await;
        assert_eq!(
            status,
            want,
            "title {} chars, body {} chars: {v}",
            title.chars().count(),
            body.chars().count()
        );
    }
    // Only the two valid ones were stored; plain text comes back exactly as
    // sent (no rendering, no stripping).
    let (_, v) = get(format!("{base}/notifications"), Who::Key(&ali_key)).await;
    let list = v["notifications"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["body"], "line one\nline two\t<b>not html</b>");
    assert_eq!(list[1]["title"].as_str().unwrap().chars().count(), 120);
}

#[tokio::test]
async fn a_smaster_notifies_only_its_own_groups_and_audiences_freeze() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("ntf-groups.db");
    let (port, _guard) = building(&db).await;
    let base = format!("http://127.0.0.1:{port}");

    let master = master_session(&base).await;
    let kaan = smaster(&base, "kaan").await;
    let other = smaster(&base, "other").await;
    let lone = smaster(&base, "lone").await;
    let ali_key = member(&base, "ali", "user").await;
    let veli_key = member(&base, "veli", "user").await;
    let bot_key = member(&base, "bot", "agent").await;

    let platform = found_group(&base, &kaan, "platform", &["ali", "bot"]).await;
    let design = found_group(&base, &other, "design", &["veli"]).await;

    // The composer offers a Smaster only its own groups, never "everyone".
    let (_, opts) = get(
        format!("{base}/notifications/audiences"),
        Who::Session(&kaan),
    )
    .await;
    assert_eq!(opts["everyone"], false);
    assert_eq!(opts["can_send"], true);
    let names: Vec<&str> = opts["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["platform"]);
    assert_eq!(
        opts["groups"][0]["members"], 2,
        "ali, bot — never kaan itself"
    );

    // A group holding only its founder reaches nobody: refused, not stored.
    let solo = found_group(&base, &kaan, "solo", &[]).await;
    let (status, v) = notify(
        &base,
        Who::Session(&kaan),
        "Nobody",
        serde_json::json!({ "groups": [solo] }),
    )
    .await;
    assert_eq!(status, 409, "{v}");
    let (_, mine) = get(format!("{base}/notifications/sent"), Who::Session(&kaan)).await;
    assert_eq!(mine, serde_json::json!([]));

    let (status, sent) = notify(
        &base,
        Who::Session(&kaan),
        "Deploy friday",
        serde_json::json!({ "groups": [platform] }),
    )
    .await;
    assert_eq!(status, 201, "{sent}");
    let id = sent["id"].as_i64().unwrap();
    assert_eq!(sent["recipients"], 2, "ali and bot; never the sender");

    // Only the group's members receive it.
    assert_eq!(
        inbox(&base, Who::Key(&ali_key)).await.0,
        vec!["Deploy friday"]
    );
    assert_eq!(
        inbox(&base, Who::Key(&bot_key)).await.0,
        vec!["Deploy friday"]
    );
    for outsider in [
        Who::Key(&veli_key),
        Who::Session(&other),
        Who::Session(&lone),
        Who::Session(&master),
        Who::Session(&kaan),
    ] {
        assert_eq!(inbox(&base, outsider).await, (vec![], 0));
    }
    // An outsider can neither mark it nor find it among what it sent.
    for outsider in [Who::Key(&veli_key), Who::Session(&other)] {
        let (status, _) = post(
            format!("{base}/notifications/{id}/read"),
            outsider,
            serde_json::json!({}),
        )
        .await;
        assert_eq!(status, 404);
    }
    let (_, sent_by_other) = get(format!("{base}/notifications/sent"), Who::Session(&other)).await;
    assert_eq!(sent_by_other, serde_json::json!([]));
    // A reader sees the group's name only for a group it is in.
    let (_, v) = get(format!("{base}/notifications"), Who::Key(&ali_key)).await;
    assert_eq!(
        v["notifications"][0]["audience"]["groups"],
        serde_json::json!(["platform"])
    );
    assert_eq!(v["notifications"][0]["from"], "kaan");

    // A group the Smaster is not in: refused as missing. Mixed lists too.
    for groups in [
        serde_json::json!([design]),
        serde_json::json!([platform, design]),
        serde_json::json!(["grp_missing"]),
    ] {
        let (status, v) = notify(
            &base,
            Who::Session(&kaan),
            "x",
            serde_json::json!({ "groups": groups }),
        )
        .await;
        assert_eq!(status, 404, "{v}");
    }
    let (status, _) = notify(
        &base,
        Who::Session(&kaan),
        "x",
        serde_json::json!({ "groups": [] }),
    )
    .await;
    assert_eq!(status, 400);
    // A Smaster with no group cannot send at all.
    let (status, _) = notify(
        &base,
        Who::Session(&lone),
        "x",
        serde_json::json!({ "groups": [platform] }),
    )
    .await;
    assert_eq!(status, 403);
    // Nothing was stored by the refusals.
    assert_eq!(inbox(&base, Who::Key(&veli_key)).await, (vec![], 0));

    // Someone who joins later does not get the old notification...
    add_to_group(&base, &kaan, &platform, "veli").await;
    assert_eq!(inbox(&base, Who::Key(&veli_key)).await, (vec![], 0));
    // ...but gets the next one.
    let (status, sent) = notify(
        &base,
        Who::Session(&kaan),
        "Standup moved",
        serde_json::json!({ "groups": [platform] }),
    )
    .await;
    assert_eq!(status, 201);
    assert_eq!(sent["recipients"], 3);
    assert_eq!(
        inbox(&base, Who::Key(&veli_key)).await,
        (vec!["Standup moved".to_string()], 1)
    );

    // A Smaster in two groups addresses both; one person in both gets it once.
    let (status, sent) = notify(
        &base,
        Who::Session(&other),
        "Design review",
        serde_json::json!({ "groups": [design] }),
    )
    .await;
    assert_eq!((status, sent["recipients"].as_u64()), (201, Some(1)));
    // The Master may address any group.
    let (status, sent) = notify(
        &base,
        Who::Session(&master),
        "Both teams",
        serde_json::json!({ "groups": [platform, design] }),
    )
    .await;
    assert_eq!(status, 201, "{sent}");
    assert_eq!(sent["recipients"], 5, "kaan, ali, bot, veli (once), other");
    let (titles, unread) = inbox(&base, Who::Key(&veli_key)).await;
    assert_eq!(titles, vec!["Both teams", "Design review", "Standup moved"]);
    assert_eq!(unread, 3);
    // An outsider learns nothing of a group it is not in: veli is in both,
    // but ali (platform only) never sees "design" named.
    let (_, v) = get(format!("{base}/notifications"), Who::Key(&ali_key)).await;
    assert_eq!(
        v["notifications"][0]["audience"]["groups"],
        serde_json::json!(["platform"])
    );
    assert!(!v.to_string().contains("design"), "{v}");
    // The sender's own view names every group it addressed.
    let (_, mine) = get(format!("{base}/notifications/sent"), Who::Session(&kaan)).await;
    assert_eq!(mine.as_array().unwrap().len(), 2);
    assert_eq!(mine[0]["title"], "Standup moved");
    assert_eq!(mine[0]["audience"]["groups"][0]["name"], "platform");

    // Paging: limit + before walk back without gaps.
    let (_, page) = get(format!("{base}/notifications?limit=2"), Who::Key(&veli_key)).await;
    assert_eq!(page["notifications"].as_array().unwrap().len(), 2);
    assert_eq!(page["more"], true);
    let last = page["notifications"][1]["id"].as_i64().unwrap();
    let (_, page) = get(
        format!("{base}/notifications?limit=2&before={last}"),
        Who::Key(&veli_key),
    )
    .await;
    assert_eq!(page["notifications"][0]["title"], "Standup moved");
    assert_eq!(page["more"], false);
}

#[tokio::test]
async fn agents_hear_notifications_on_their_lobby_line_only_when_addressed() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("ntf-lobby.db");
    let (port, _guard) = building(&db).await;
    let base = format!("http://127.0.0.1:{port}");

    let master = master_session(&base).await;
    let kaan = smaster(&base, "kaan").await;
    let bot_key = member(&base, "bot", "agent").await;
    let stray_key = member(&base, "stray", "agent").await;
    let platform = found_group(&base, &kaan, "platform", &["bot"]).await;

    let mut bot = lobby(port, &bot_key).await;
    let mut stray = lobby(port, &stray_key).await;
    // lobby_ready is sent after the socket subscribes, so nothing is missed.
    wait_for(&mut bot, |v| v["t"] == "lobby_ready").await;
    wait_for(&mut stray, |v| v["t"] == "lobby_ready").await;

    let (status, _) = notify(
        &base,
        Who::Session(&kaan),
        "Group only",
        serde_json::json!({ "groups": [platform] }),
    )
    .await;
    assert_eq!(status, 201);
    let frame = wait_for(&mut bot, |v| v["t"] == "notification").await;
    assert_eq!(frame["title"], "Group only");
    assert_eq!(frame["body"], "Group only body");
    assert_eq!(frame["from"], "kaan");
    assert_eq!(frame["scope"], "group");
    assert!(frame["id"].as_i64().is_some());
    let text = frame.to_string();
    assert!(
        !text.contains("mb_") && !text.contains(&bot_key),
        "no secret: {text}"
    );
    assert!(
        !text.contains("platform"),
        "no group name on the wire: {text}"
    );

    // The stray agent is not in the audience: the first notification frame it
    // ever sees is the Master's building-wide one that follows.
    let (status, _) = notify(
        &base,
        Who::Session(&master),
        "Everyone now",
        serde_json::json!({ "all": true }),
    )
    .await;
    assert_eq!(status, 201);
    let first = wait_for(&mut stray, |v| v["t"] == "notification").await;
    assert_eq!(first["title"], "Everyone now");
    assert_eq!(first["scope"], "building");
    let next = wait_for(&mut bot, |v| v["t"] == "notification").await;
    assert_eq!(next["title"], "Everyone now");

    // The agent reads the persisted list with its own key, and marks it.
    let (titles, unread) = inbox(&base, Who::Key(&bot_key)).await;
    assert_eq!((titles.len(), unread), (2, 2));
    let (status, _) = post(
        format!("{base}/notifications/read-all"),
        Who::Key(&bot_key),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(inbox(&base, Who::Key(&stray_key)).await.1, 1);
}

#[tokio::test]
async fn notifications_and_reads_survive_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("ntf-restart.db");
    let (port, guard) = building(&db).await;
    let base = format!("http://127.0.0.1:{port}");
    let master = master_session(&base).await;
    let ali_key = member(&base, "ali", "user").await;
    let veli_key = member(&base, "veli", "user").await;
    let (_, sent) = notify(
        &base,
        Who::Session(&master),
        "Kept",
        serde_json::json!({ "all": true }),
    )
    .await;
    let id = sent["id"].as_i64().unwrap();
    post(
        format!("{base}/notifications/{id}/read"),
        Who::Key(&ali_key),
        serde_json::json!({}),
    )
    .await;

    drop(guard);
    wait_for_server_shutdown(port).await;
    let (port, _guard) = building(&db).await;
    let base = format!("http://127.0.0.1:{port}");

    let (_, v) = get(format!("{base}/notifications"), Who::Key(&ali_key)).await;
    assert_eq!(v["notifications"][0]["title"], "Kept");
    assert_eq!(v["notifications"][0]["read"], true);
    assert_eq!(v["unread"], 0);
    assert_eq!(
        inbox(&base, Who::Key(&veli_key)).await,
        (vec!["Kept".to_string()], 1)
    );
    let (_, mine) = get(format!("{base}/notifications/sent"), Who::Key(MASTER)).await;
    assert_eq!(mine[0]["recipients"], 2);
    assert_eq!(mine[0]["read"], 1);
    // New ids continue after the old ones.
    let (_, next) = notify(
        &base,
        Who::Key(MASTER),
        "After",
        serde_json::json!({ "all": true }),
    )
    .await;
    assert!(next["id"].as_i64().unwrap() > id);
}
