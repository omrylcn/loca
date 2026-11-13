//! Groups: a group's own locas (PRINCIPLES "Groups"). The door, the mint,
//! the listings, the copies into İye, removal, ownership, exclusions and a
//! restart — each leak path gets its own assertion.
use super::*;

const MASTER: &str = "grp-master-key";

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

fn sess(token: &str) -> [(&str, &str); 1] {
    [("x-session-token", token)]
}

async fn get(url: String, token: &str) -> (u16, Value) {
    send(reqwest::Method::GET, url, &sess(token), None).await
}

async fn post(url: String, token: &str, body: Value) -> (u16, Value) {
    send(reqwest::Method::POST, url, &sess(token), Some(body)).await
}

async fn put(url: String, token: &str, body: Option<Value>) -> (u16, Value) {
    send(reqwest::Method::PUT, url, &sess(token), body).await
}

async fn delete(url: String, token: &str) -> (u16, Value) {
    send(reqwest::Method::DELETE, url, &sess(token), None).await
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

/// A Smaster key and a session signed in with it.
async fn smaster(base: &str, name: &str) -> (String, String) {
    let (status, v) = send(
        reqwest::Method::POST,
        format!("{base}/smasters"),
        &[("x-admin-token", MASTER)],
        Some(serde_json::json!({ "name": name })),
    )
    .await;
    assert!(status < 300, "smaster {name}: {status} {v}");
    let key = v["token"].as_str().unwrap().to_string();
    let s = session(base, ("x-admin-token", &key), name, "user").await;
    (key, s)
}

/// A Building member (mb_ key).
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

fn names_room(listing: &Value, room: &str) -> bool {
    listing.to_string().contains(&format!("\"{room}\""))
}

fn texts(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|m| m["text"].as_str().unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default()
}

async fn closed_building(db: &std::path::Path) -> (u16, ServerGuard) {
    spawn_server_env(
        MASTER,
        &[
            ("DB_PATH", db.to_str().unwrap().into()),
            ("REQUIRE_SESSIONS", "1".into()),
            ("REQUIRE_INVITE", "1".into()),
            ("LOCA_AGENT_ROOM", "iye".into()),
            ("RESERVED_LOCA", "iye".into()),
            ("LOCA_CARETAKERS", "loca-dev,loca-care".into()),
        ],
    )
    .await
}

#[tokio::test]
async fn a_group_loca_opens_only_to_its_members_and_the_master() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("groups-door.db");
    let (port, _guard) = closed_building(&db).await;
    let base = format!("http://127.0.0.1:{port}");

    let master = master_session(&base).await;
    let (_, lead) = smaster(&base, "kaan").await;
    let (_, other) = smaster(&base, "other").await;
    let (_, inner) = smaster(&base, "inner").await;
    let ali_key = member(&base, "ali", "user").await;
    let veli_key = member(&base, "veli", "user").await;
    member(&base, "bot", "agent").await;
    member(&base, "stray", "agent").await;
    let ali = session(&base, ("x-room-token", &ali_key), "ali", "user").await;
    let veli = session(&base, ("x-room-token", &veli_key), "veli", "user").await;

    // Default behaviour: a loca with no group is exactly as before — every
    // rank enters it, a member needs a davet, and its summary names no group.
    create_test_room(port, MASTER, "open-table").await;
    assert_eq!(rest_can(&base, "open-table", &sess(&other)).await, 200);
    assert_eq!(rest_can(&base, "open-table", &sess(&ali)).await, 401);
    let (_, rooms) = get(format!("{base}/rooms"), &other).await;
    let open = rooms
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["room"] == "open-table")
        .unwrap();
    assert!(open.get("group").is_none(), "a default loca names no group");

    // Founding: a Smaster founds and owns; it is the first member.
    assert_eq!(
        post(
            format!("{base}/groups"),
            &ali,
            serde_json::json!({"name": "x"})
        )
        .await
        .0,
        403,
        "only a rank founds a group"
    );
    let (status, group) = post(
        format!("{base}/groups"),
        &lead,
        serde_json::json!({"name": "backend"}),
    )
    .await;
    assert_eq!(status, 201, "{group}");
    let gid = group["id"].as_str().unwrap().to_string();
    assert_eq!(group["owner"], "kaan");
    let lead_id = principal_of(&base, "kaan").await;
    let ali_id = principal_of(&base, "ali").await;
    let veli_id = principal_of(&base, "veli").await;
    let bot_id = principal_of(&base, "bot").await;
    let inner_id = principal_of(&base, "inner").await;
    let master_id = principal_of(&base, "Master").await;

    // Another Smaster does not see it and cannot add anyone to it.
    let (_, seen) = get(format!("{base}/groups"), &other).await;
    assert!(!seen.to_string().contains(&gid));
    let (status, _) = put(
        format!("{base}/groups/{gid}/members/{veli_id}"),
        &other,
        None,
    )
    .await;
    assert_eq!(status, 404, "an outsider Smaster cannot touch the group");
    let (_, seen) = get(format!("{base}/groups"), &master).await;
    assert!(
        seen.to_string().contains(&gid),
        "the Master sees every group"
    );

    // Rights: the founder adds; nobody adds themselves; the Master is never
    // a member.
    for id in [&ali_id, &bot_id, &inner_id] {
        let (status, v) = put(format!("{base}/groups/{gid}/members/{id}"), &lead, None).await;
        assert_eq!(status, 200, "{v}");
    }
    assert_eq!(
        put(
            format!("{base}/groups/{gid}/members/{lead_id}"),
            &lead,
            None
        )
        .await
        .0,
        403,
        "nobody adds themselves"
    );
    assert_eq!(
        put(
            format!("{base}/groups/{gid}/members/{master_id}"),
            &master,
            None
        )
        .await
        .0,
        403,
        "the Master is never a member"
    );
    assert_eq!(
        put(format!("{base}/groups/{gid}/members/{veli_id}"), &ali, None)
            .await
            .0,
        403,
        "a member who is not the founder manages nothing"
    );
    assert_eq!(
        put(
            format!("{base}/groups/{gid}/members/{veli_id}"),
            &inner,
            None
        )
        .await
        .0,
        403,
        "a member Smaster who is not the founder manages nothing"
    );

    // Group loca: a member Smaster or the Master opens one; nobody else.
    let (status, _) = post(
        format!("{base}/rooms"),
        &other,
        serde_json::json!({"name": "gl-other", "group": gid}),
    )
    .await;
    assert_eq!(status, 404, "an outsider Smaster cannot open a loca in it");
    let (status, v) = post(
        format!("{base}/rooms"),
        &lead,
        serde_json::json!({"name": "gl", "group": gid}),
    )
    .await;
    assert_eq!(status, 201, "{v}");
    let (status, v) = post(
        format!("{base}/rooms"),
        &inner,
        serde_json::json!({"name": "gl two", "group": gid}),
    )
    .await;
    assert_eq!(status, 201, "a member Smaster opens one: {v}");
    let (status, _) = post(
        format!("{base}/rooms/gl/messages"),
        &lead,
        serde_json::json!({"sender": "x", "sender_type": "user", "text": "group secret"}),
    )
    .await;
    assert!(status < 300);

    // Outsiders: no read, no search, no listing, no socket, no rank action —
    // and the same answer as a loca that does not exist.
    let (missing_status, missing_body) =
        get(format!("{base}/rooms/no-such-loca/messages"), &other).await;
    for (who, s) in [("smaster", &other), ("member", &veli)] {
        for room in ["gl", "gl%20two"] {
            let (status, body) = get(format!("{base}/rooms/{room}/messages"), s).await;
            assert_eq!(
                (status, &body),
                (missing_status, &missing_body),
                "{who} must get a missing loca's answer for {room}"
            );
        }
        assert_eq!(
            get(format!("{base}/rooms/gl/search?q=secret"), s).await.0,
            404
        );
        assert_eq!(get(format!("{base}/rooms/gl/settings"), s).await.0, 404);
        assert_eq!(
            put(
                format!("{base}/rooms/gl/mode"),
                s,
                Some(serde_json::json!({"mode": {"kind": "free"}}))
            )
            .await
            .0,
            404,
            "{who} must not act on the loca by rank"
        );
        let (_, rooms) = get(format!("{base}/rooms"), s).await;
        assert!(!names_room(&rooms, "gl"), "{who} must not list it");
        assert!(!names_room(&rooms, "gl two"));
        assert!(!ws_can(port, "gl", &format!("session={s}")).await);
    }
    assert_eq!(
        post(
            format!("{base}/rooms/gl/call"),
            &other,
            serde_json::json!({"name": "veli"})
        )
        .await
        .0,
        404
    );

    // Members and the Master enter; the summary names the group.
    for (who, s) in [
        ("member", &ali),
        ("member smaster", &inner),
        ("master", &master),
    ] {
        let (status, read) = get(format!("{base}/rooms/gl/messages"), s).await;
        assert_eq!(status, 200, "{who} reads the group loca");
        assert!(texts(&read).contains(&"group secret".to_string()));
        let (_, rooms) = get(format!("{base}/rooms"), s).await;
        let gl = rooms
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["room"] == "gl")
            .unwrap_or_else(|| panic!("{who} lists gl"));
        assert_eq!(gl["group"], "backend");
        assert!(ws_can(port, "gl", &format!("session={s}")).await);
    }
    assert_eq!(
        rest_can(&base, "gl", &[("x-admin-token", MASTER)]).await,
        200
    );
    assert!(ws_can(port, "gl", &format!("admin={MASTER}")).await);

    // Invites: nobody outside the group is invited or called in — not by
    // the founder, not by the Master.
    for (who, s) in [("founder", &lead), ("master", &master)] {
        let (status, _) = post(
            format!("{base}/rooms/gl/invites"),
            s,
            serde_json::json!({"name": "veli"}),
        )
        .await;
        assert_eq!(status, 403, "{who} cannot invite an outsider");
        let (status, _) = post(
            format!("{base}/rooms/gl/call"),
            s,
            serde_json::json!({"name": "stray"}),
        )
        .await;
        assert_eq!(status, 403, "{who} cannot call an outsider in");
    }
    // A member agent is called in and enters by its davet.
    let (status, called) = post(
        format!("{base}/rooms/gl/call"),
        &lead,
        serde_json::json!({"name": "bot"}),
    )
    .await;
    assert_eq!(status, 201, "{called}");
    let bot_davet = called["token"].as_str().unwrap().to_string();
    assert_eq!(
        rest_can(&base, "gl", &[("x-room-token", &bot_davet)]).await,
        200
    );

    // Listings: a non-Master viewer never sees the group loca named as where
    // someone sits; the Master does.
    let (mut inner_ws, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/ws?room=gl&name=inner&type=user&session={inner}"
    ))
    .await
    .unwrap();
    let _ = wait_for(&mut inner_ws, |v| v["t"] == "history").await;
    let (status, _) = post(
        format!("{base}/rooms/gl/moderate"),
        &lead,
        serde_json::json!({"action": "ban", "name": "veli"}),
    )
    .await;
    assert_eq!(status, 200);
    for path in ["building/ranks", "residents", "building/bans"] {
        let (status, other_view) = get(format!("{base}/{path}"), &other).await;
        assert_eq!(status, 200, "{path}");
        assert!(
            !names_room(&other_view, "gl"),
            "{path} names a group loca to an outsider: {other_view}"
        );
        let (_, master_view) = get(format!("{base}/{path}"), &master).await;
        assert!(
            names_room(&master_view, "gl"),
            "{path} shows the Master everything: {master_view}"
        );
    }
    let (_, other_people) = get(format!("{base}/residents"), &other).await;
    assert!(
        other_people.to_string().contains("\"bot\""),
        "people are still listed, only the group loca is not"
    );

    // Profile: a person sees their own groups.
    let (_, profile) = get(format!("{base}/profile"), &ali).await;
    assert_eq!(profile["groups"][0]["name"], "backend");
    let (_, profile) = get(format!("{base}/profile"), &veli).await;
    assert_eq!(profile["groups"].as_array().map(Vec::len), Some(0));

    // The record says who changed what, for the founder and the Master only.
    let (status, log) = get(format!("{base}/groups/{gid}/log"), &lead).await;
    assert_eq!(status, 200);
    let actions: Vec<&str> = log
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["action"].as_str().unwrap())
        .collect();
    assert!(actions.contains(&"created") && actions.contains(&"member_added"));
    assert!(actions.contains(&"loca_joined"));
    assert!(log
        .as_array()
        .unwrap()
        .iter()
        .all(|e| e["at"].as_u64() > Some(0)));
    assert!(log
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["action"] == "member_added" && e["subject"] == "ali" && e["by"] == "kaan"));
    assert_eq!(get(format!("{base}/groups/{gid}/log"), &ali).await.0, 403);
}

#[tokio::test]
async fn leaving_a_group_ends_access_at_once_even_on_an_open_socket() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("groups-removal.db");
    let (port, _guard) = closed_building(&db).await;
    let base = format!("http://127.0.0.1:{port}");
    let (_, lead) = smaster(&base, "kaan").await;
    let ali_key = member(&base, "ali", "user").await;
    member(&base, "bot", "agent").await;
    let ali = session(&base, ("x-room-token", &ali_key), "ali", "user").await;
    let (_, group) = post(
        format!("{base}/groups"),
        &lead,
        serde_json::json!({"name": "ops"}),
    )
    .await;
    let gid = group["id"].as_str().unwrap().to_string();
    let ali_id = principal_of(&base, "ali").await;
    let bot_id = principal_of(&base, "bot").await;
    for id in [&ali_id, &bot_id] {
        assert_eq!(
            put(format!("{base}/groups/{gid}/members/{id}"), &lead, None)
                .await
                .0,
            200
        );
    }
    let (status, _) = post(
        format!("{base}/rooms"),
        &lead,
        serde_json::json!({"name": "ops-room", "group": gid}),
    )
    .await;
    assert_eq!(status, 201);
    let (_, called) = post(
        format!("{base}/rooms/ops-room/call"),
        &lead,
        serde_json::json!({"name": "bot"}),
    )
    .await;
    let davet = called["token"].as_str().unwrap().to_string();
    let bot_session = session(&base, ("x-room-token", &davet), "bot", "agent").await;

    let (mut ali_ws, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/ws?room=ops-room&name=ali&type=user&session={ali}"
    ))
    .await
    .unwrap();
    let _ = wait_for(&mut ali_ws, |v| v["t"] == "history").await;
    let (mut bot_ws, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/ws?room=ops-room&name=bot&type=agent&token={davet}"
    ))
    .await
    .unwrap();
    let _ = wait_for(&mut bot_ws, |v| v["t"] == "history").await;

    // Removing a person closes their open socket and their door at once.
    let (status, _) = delete(format!("{base}/groups/{gid}/members/{ali_id}"), &lead).await;
    assert_eq!(status, 200);
    let closed = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match ali_ws.next().await {
                None | Some(Err(_)) => return,
                Some(Ok(WsMessage::Close(_))) => return,
                Some(Ok(WsMessage::Text(t))) => {
                    let v: Value = serde_json::from_str(&t).unwrap();
                    assert_ne!(v["t"], "msg", "nothing more reaches a removed member");
                    if v["t"] == "kicked" {
                        return;
                    }
                }
                Some(Ok(_)) => {}
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "the removed member's socket must close");
    assert_eq!(rest_can(&base, "ops-room", &sess(&ali)).await, 404);
    assert!(!ws_can(port, "ops-room", &format!("session={ali}")).await);

    // Removing an agent revokes its davet, the session minted from it, and
    // its socket.
    let (status, _) = delete(format!("{base}/groups/{gid}/members/{bot_id}"), &lead).await;
    assert_eq!(status, 200);
    let closed = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match bot_ws.next().await {
                None | Some(Err(_)) | Some(Ok(WsMessage::Close(_))) => return,
                Some(Ok(WsMessage::Text(t))) => {
                    let v: Value = serde_json::from_str(&t).unwrap();
                    if v["t"] == "kicked" {
                        return;
                    }
                }
                Some(Ok(_)) => {}
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "the removed agent's socket must close");
    assert_ne!(
        rest_can(&base, "ops-room", &[("x-room-token", &davet)]).await,
        200
    );
    assert_ne!(rest_can(&base, "ops-room", &sess(&bot_session)).await, 200);
    assert!(!ws_can(port, "ops-room", &format!("token={davet}")).await);
    // The founder still reads; a message posted now reaches nobody removed.
    assert_eq!(rest_can(&base, "ops-room", &sess(&lead)).await, 200);
}

#[tokio::test]
async fn a_group_loca_is_never_copied_into_iye() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("groups-iye.db");
    let (port, _guard) = closed_building(&db).await;
    let base = format!("http://127.0.0.1:{port}");
    let (_, lead) = smaster(&base, "kaan").await;
    member(&base, "loca-dev", "agent").await;
    let (status, iye_davet) = send(
        reqwest::Method::POST,
        format!("{base}/rooms/iye/invites"),
        &[("x-admin-token", MASTER)],
        Some(serde_json::json!({"name": "loca-dev"})),
    )
    .await;
    assert_eq!(status, 200, "{iye_davet}");
    let iye_davet = iye_davet["token"].as_str().unwrap().to_string();
    let (mut caretaker, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/ws?room=iye&name=loca-dev&type=agent&filter=mentions&turn_max=1&token={iye_davet}"
    ))
    .await
    .unwrap();

    let (_, group) = post(
        format!("{base}/groups"),
        &lead,
        serde_json::json!({"name": "secretive"}),
    )
    .await;
    let gid = group["id"].as_str().unwrap().to_string();
    create_test_room(port, MASTER, "plain").await;
    let (status, _) = post(
        format!("{base}/rooms"),
        &lead,
        serde_json::json!({"name": "hidden", "group": gid}),
    )
    .await;
    assert_eq!(status, 201);

    // First the group loca summons the caretaker, then a default loca does.
    for room in ["hidden", "plain"] {
        let (status, v) = post(
            format!("{base}/rooms/{room}/messages"),
            &lead,
            serde_json::json!({
                "sender": "x", "sender_type": "user", "target": "loca-dev",
                "text": format!("@loca-dev look at {room}")
            }),
        )
        .await;
        assert!(status < 300, "{v}");
    }
    // Frames arrive in order: the default loca's summons arrives, and the
    // group loca's never came before it.
    loop {
        let care = wait_for(&mut caretaker, |v| v["t"] == "care").await;
        let source = care["signal"]["source_room"]
            .as_str()
            .unwrap_or("")
            .to_string();
        assert_ne!(
            source, "hidden",
            "a group loca's words were copied into İye"
        );
        if source == "plain" {
            break;
        }
    }
    let (_, iye_messages) = send(
        reqwest::Method::GET,
        format!("{base}/rooms/iye/messages"),
        &[("x-admin-token", MASTER)],
        None,
    )
    .await;
    assert!(!iye_messages.to_string().contains("look at hidden"));
}

#[tokio::test]
async fn ownership_exclusions_and_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("groups-restart.db");
    let (port, guard) = closed_building(&db).await;
    let base = format!("http://127.0.0.1:{port}");
    let master = master_session(&base).await;
    let (_, lead) = smaster(&base, "kaan").await;
    let (_, other) = smaster(&base, "other").await;
    let ali_key = member(&base, "ali", "user").await;
    let ali = session(&base, ("x-room-token", &ali_key), "ali", "user").await;
    let ali_id = principal_of(&base, "ali").await;

    let (_, group) = post(
        format!("{base}/groups"),
        &lead,
        serde_json::json!({"name": "platform"}),
    )
    .await;
    let gid = group["id"].as_str().unwrap().to_string();
    put(format!("{base}/groups/{gid}/members/{ali_id}"), &lead, None).await;
    let (status, _) = post(
        format!("{base}/rooms"),
        &lead,
        serde_json::json!({"name": "plat", "group": gid}),
    )
    .await;
    assert_eq!(status, 201);
    post(
        format!("{base}/rooms/plat/messages"),
        &ali,
        serde_json::json!({"sender": "x", "sender_type": "user", "text": "kept across restart"}),
    )
    .await;

    // Exclusions: İye and a birebir never get a group; only the Master moves
    // a loca between groups.
    for room in ["iye", "bb-0000squat"] {
        let (status, _) = put(
            format!("{base}/rooms/{room}/group"),
            &master,
            Some(serde_json::json!({"group": gid})),
        )
        .await;
        assert!(status >= 400, "{room} must not join a group");
    }
    let (status, opened) = post(
        format!("{base}/birebirs"),
        &master,
        serde_json::json!({"name": "ali"}),
    )
    .await;
    assert_eq!(status, 201, "{opened}");
    let bb = opened["birebir"]["room"].as_str().unwrap().to_string();
    let (status, _) = put(
        format!("{base}/rooms/{bb}/group"),
        &master,
        Some(serde_json::json!({"group": gid})),
    )
    .await;
    assert!(status >= 400, "a birebir must not join a group");
    let (status, _) = post(
        format!("{base}/rooms"),
        &master,
        serde_json::json!({"name": "iye", "group": gid}),
    )
    .await;
    assert!(status >= 400);
    create_test_room(port, MASTER, "later").await;
    assert_eq!(
        put(
            format!("{base}/rooms/later/group"),
            &lead,
            Some(serde_json::json!({"group": gid}))
        )
        .await
        .0,
        403,
        "only the Master moves a loca"
    );
    let (status, moved) = put(
        format!("{base}/rooms/later/group"),
        &master,
        Some(serde_json::json!({"group": gid})),
    )
    .await;
    assert_eq!(status, 200, "{moved}");
    assert_eq!(rest_can(&base, "later", &sess(&other)).await, 404);

    // Deleting needs every loca closed; archiving closes them.
    assert_eq!(delete(format!("{base}/groups/{gid}"), &lead).await.0, 409);

    // Restart: groups, members, founders and group locas all come back.
    drop(guard);
    wait_for_server_shutdown(port).await;
    let (port, guard) = closed_building(&db).await;
    let base = format!("http://127.0.0.1:{port}");
    let ali = session(&base, ("x-room-token", &ali_key), "ali", "user").await;
    let master = master_session(&base).await;
    let (status, read) = get(format!("{base}/rooms/plat/messages"), &ali).await;
    assert_eq!(status, 200, "a member still enters after restart");
    assert!(texts(&read).contains(&"kept across restart".to_string()));
    assert_eq!(rest_can(&base, "plat", &sess(&other)).await, 404);
    assert_eq!(rest_can(&base, "later", &sess(&other)).await, 404);
    let (_, view) = get(format!("{base}/groups/{gid}"), &lead).await;
    assert_eq!(view["owner"], "kaan");
    assert!(view["locas"].to_string().contains("plat"));
    assert!(view["members"].to_string().contains("ali"));

    // The founder is revoked: the group passes to the Master, recorded.
    let (_, smasters) = send(
        reqwest::Method::GET,
        format!("{base}/smasters"),
        &[("x-admin-token", MASTER)],
        None,
    )
    .await;
    let lead_ref = smasters
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "kaan")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, _) = send(
        reqwest::Method::DELETE,
        format!("{base}/smasters/{lead_ref}"),
        &[("x-admin-token", MASTER)],
        None,
    )
    .await;
    assert!(status < 300);
    let (_, view) = get(format!("{base}/groups/{gid}"), &master).await;
    assert_eq!(view["owned_by_master"], true, "{view}");
    assert!(!view["members"].to_string().contains("\"kaan\""));
    let (_, log) = get(format!("{base}/groups/{gid}/log"), &master).await;
    assert!(log.to_string().contains("owner_passed"));
    assert_eq!(rest_can(&base, "plat", &sess(&ali)).await, 200);

    // Archive closes its locas; then it may be deleted, and its closed
    // locas stay out of reach for everyone but the Master.
    let (status, archived) = send(
        reqwest::Method::PATCH,
        format!("{base}/groups/{gid}"),
        &sess(&master),
        Some(serde_json::json!({"archived": true})),
    )
    .await;
    assert_eq!(status, 200, "{archived}");
    let (_, rooms) = get(format!("{base}/rooms"), &ali).await;
    let plat = rooms
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["room"] == "plat")
        .unwrap();
    assert_eq!(plat["archived"], true);
    assert_eq!(delete(format!("{base}/groups/{gid}"), &master).await.0, 204);
    assert_eq!(rest_can(&base, "plat", &sess(&ali)).await, 404);
    assert_eq!(rest_can(&base, "plat", &sess(&master)).await, 200);

    // ... and that survives a restart too.
    drop(guard);
    wait_for_server_shutdown(port).await;
    let (port, _guard) = closed_building(&db).await;
    let base = format!("http://127.0.0.1:{port}");
    let ali = session(&base, ("x-room-token", &ali_key), "ali", "user").await;
    let master = master_session(&base).await;
    assert_eq!(rest_can(&base, "plat", &sess(&ali)).await, 404);
    assert_eq!(rest_can(&base, "plat", &sess(&master)).await, 200);
    let (_, groups) = get(format!("{base}/groups"), &master).await;
    assert!(
        !groups.to_string().contains(&gid),
        "a deleted group is gone"
    );
}

#[tokio::test]
async fn a_database_from_before_groups_keeps_every_loca_default() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("groups-upgrade.db");
    let (port, guard) = closed_building(&db).await;
    let base = format!("http://127.0.0.1:{port}");
    create_test_room(port, MASTER, "legacy").await;
    let davet = davet_for(&base, MASTER, "legacy", "old-agent").await;
    drop(guard);
    wait_for_server_shutdown(port).await;

    // Make it an older database: no group tables at all.
    {
        let c = rusqlite::Connection::open(&db).unwrap();
        c.execute_batch(
            "DROP TABLE group_log; DROP TABLE group_rooms; DROP TABLE group_members; DROP TABLE groups;",
        )
        .unwrap();
    }

    let (port, _guard) = closed_building(&db).await;
    let base = format!("http://127.0.0.1:{port}");
    let (_, other) = smaster(&base, "other").await;
    assert_eq!(
        rest_can(&base, "legacy", &[("x-room-token", &davet)]).await,
        200
    );
    assert_eq!(rest_can(&base, "legacy", &sess(&other)).await, 200);
    let (_, rooms) = get(format!("{base}/rooms"), &other).await;
    let legacy = rooms
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["room"] == "legacy")
        .unwrap();
    assert!(legacy.get("group").is_none());
    let (status, _) = post(
        format!("{base}/groups"),
        &other,
        serde_json::json!({"name": "fresh"}),
    )
    .await;
    assert_eq!(status, 201, "groups work on an upgraded database");
}

/// One actor at the door: how it presents itself, and its name.
struct Actor {
    label: &'static str,
    headers: Vec<(&'static str, String)>,
}

async fn can_read(base: &str, room: &str, actor: &Actor) -> bool {
    let headers: Vec<(&str, &str)> = actor
        .headers
        .iter()
        .map(|(k, v)| (*k, v.as_str()))
        .collect();
    let (status, _) = send(
        reqwest::Method::GET,
        format!("{base}/rooms/{room}/messages"),
        &headers,
        None,
    )
    .await;
    status == 200
}

async fn lists(base: &str, room: &str, actor: &Actor) -> bool {
    let headers: Vec<(&str, &str)> = actor
        .headers
        .iter()
        .map(|(k, v)| (*k, v.as_str()))
        .collect();
    let (_, rooms) = send(
        reqwest::Method::GET,
        format!("{base}/rooms"),
        &headers,
        None,
    )
    .await;
    names_room(&rooms, room)
}

#[tokio::test]
async fn moving_a_loca_between_groups_matrix_for_every_kind_of_person() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _guard) = closed_building(&dir.path().join("move-matrix.db")).await;
    let base = format!("http://127.0.0.1:{port}");
    let master = master_session(&base).await;
    let (_, s_in) = smaster(&base, "mv-s-in").await;
    let (_, s_out) = smaster(&base, "mv-s-out").await;
    let h_in_key = member(&base, "mv-h-in", "user").await;
    let h_out_key = member(&base, "mv-h-out", "user").await;
    let h_new_key = member(&base, "mv-h-new", "user").await;
    let a_in_key = member(&base, "mv-a-in", "agent").await;
    let a_out_key = member(&base, "mv-a-out", "agent").await;

    // A default loca, founded by the Master; four people invited.
    let (status, _) = post(
        format!("{base}/rooms"),
        &master,
        serde_json::json!({ "name": "mv" }),
    )
    .await;
    assert!(status < 300);
    let mut davets = std::collections::HashMap::new();
    for name in ["mv-h-in", "mv-h-out", "mv-a-in", "mv-a-out"] {
        let (status, v) = post(
            format!("{base}/rooms/mv/invites"),
            &master,
            serde_json::json!({ "name": name }),
        )
        .await;
        assert!(status < 300, "invite {name}: {status} {v}");
        davets.insert(name, v["token"].as_str().unwrap().to_string());
    }
    let (status, _) = post(
        format!("{base}/rooms/mv/lead"),
        &master,
        serde_json::json!({ "lead": "mv-h-out" }),
    )
    .await;
    assert!(status < 300, "lead");

    // Two groups: backend (s-in founds it; h-in, h-new, a-in join) and ops (empty).
    let (_, backend) = post(
        format!("{base}/groups"),
        &s_in,
        serde_json::json!({ "name": "backend" }),
    )
    .await;
    let backend = backend["id"].as_str().unwrap().to_string();
    for name in ["mv-h-in", "mv-h-new", "mv-a-in"] {
        let p = principal_of(&base, name).await;
        let (status, v) = put(format!("{base}/groups/{backend}/members/{p}"), &s_in, None).await;
        assert!(status < 300, "add {name}: {status} {v}");
    }
    let (_, ops) = post(
        format!("{base}/groups"),
        &master,
        serde_json::json!({ "name": "ops" }),
    )
    .await;
    let ops = ops["id"].as_str().unwrap().to_string();

    // How each kind of person presents itself: ranks and humans by session,
    // agents by their davet (or their membership key once it is gone).
    let h_in = session(&base, ("x-room-token", &h_in_key), "mv-h-in", "user").await;
    let h_out = session(&base, ("x-room-token", &h_out_key), "mv-h-out", "user").await;
    let h_new = session(&base, ("x-room-token", &h_new_key), "mv-h-new", "user").await;
    let actors = |davets: &std::collections::HashMap<&str, String>| {
        vec![
            Actor {
                label: "Master",
                headers: vec![("x-session-token", master.clone())],
            },
            Actor {
                label: "Smaster in backend",
                headers: vec![("x-session-token", s_in.clone())],
            },
            Actor {
                label: "Smaster in no group",
                headers: vec![("x-session-token", s_out.clone())],
            },
            Actor {
                label: "human in backend, invited",
                headers: vec![("x-session-token", h_in.clone())],
            },
            Actor {
                label: "human in no group, invited",
                headers: vec![("x-session-token", h_out.clone())],
            },
            Actor {
                label: "human in backend, never invited",
                headers: vec![("x-session-token", h_new.clone())],
            },
            Actor {
                label: "agent in backend, davet",
                headers: vec![("x-room-token", davets["mv-a-in"].clone())],
            },
            Actor {
                label: "agent in no group, davet",
                headers: vec![("x-room-token", davets["mv-a-out"].clone())],
            },
        ]
    };
    let _ = (&a_in_key, &a_out_key);
    let snapshot = |stage: &'static str| {
        let base = base.clone();
        let davets = davets.clone();
        async move {
            let mut row = Vec::new();
            for actor in actors(&davets) {
                row.push((
                    actor.label,
                    can_read(&base, "mv", &actor).await,
                    lists(&base, "mv", &actor).await,
                ));
            }
            println!("MATRIX {stage}: {row:?}");
            row
        }
    };
    // (read, listed) per actor, in the order of `actors`.
    let expect = |row: Vec<(&str, bool, bool)>, want: [bool; 8], stage: &str| {
        for ((label, read, listed), w) in row.into_iter().zip(want) {
            assert_eq!(read, w, "{stage}: {label} read");
            assert_eq!(listed, w, "{stage}: {label} listed");
        }
    };
    // Davet-only building: a person reaches a default loca by davet, also
    // when signed in with their own key (the davet is their seat).
    expect(
        snapshot("default").await,
        [true, true, true, true, true, false, true, true],
        "default",
    );

    // Only the Master previews or moves; İye/unknown are refused.
    assert_eq!(
        get(
            format!("{base}/rooms/mv/group/preview?group={backend}"),
            &s_in
        )
        .await
        .0,
        403
    );
    assert_eq!(
        put(
            format!("{base}/rooms/mv/group"),
            &s_in,
            Some(serde_json::json!({ "group": backend }))
        )
        .await
        .0,
        403
    );
    assert_eq!(
        put(
            format!("{base}/rooms/mv/group"),
            &s_out,
            Some(serde_json::json!({ "group": backend }))
        )
        .await
        .0,
        403
    );
    assert!(
        put(
            format!("{base}/rooms/iye/group"),
            &master,
            Some(serde_json::json!({ "group": backend }))
        )
        .await
        .0 >= 400
    );
    assert_eq!(
        put(
            format!("{base}/rooms/mv/group"),
            &master,
            Some(serde_json::json!({ "group": "grp_nope" }))
        )
        .await
        .0,
        404
    );

    // Preview to the empty ops group: every non-Master loses access.
    let (_, empty) = get(
        format!("{base}/rooms/mv/group/preview?group={ops}"),
        &master,
    )
    .await;
    assert_eq!(
        empty["losing"],
        serde_json::json!(["mv-a-in", "mv-a-out", "mv-h-in", "mv-h-out"])
    );
    assert_eq!(empty["lead_ends"], "mv-h-out");
    // Preview to backend: exactly the outsiders lose; the lead (outside) ends.
    let (_, pv) = get(
        format!("{base}/rooms/mv/group/preview?group={backend}"),
        &master,
    )
    .await;
    assert_eq!(pv["losing"], serde_json::json!(["mv-a-out", "mv-h-out"]));
    assert_eq!(pv["keeping"], serde_json::json!(["mv-a-in", "mv-h-in"]));
    assert_eq!(pv["lead_ends"], "mv-h-out");
    assert_eq!(
        pv["operator_ends"],
        Value::Null,
        "the Master's own appointment never ends"
    );

    // default → backend: members (any credential) in, every outsider out.
    assert_eq!(
        put(
            format!("{base}/rooms/mv/group"),
            &master,
            Some(serde_json::json!({ "group": backend }))
        )
        .await
        .0,
        200
    );
    expect(
        snapshot("backend").await,
        [true, true, false, true, false, true, true, false],
        "backend",
    );
    let (_, settings) = get(format!("{base}/rooms/mv/settings"), &master).await;
    assert_eq!(
        settings["lead"],
        Value::Null,
        "an outside lead's title ends"
    );
    let (_, journal) = get(format!("{base}/rooms/mv/journal"), &master).await;
    assert!(journal.to_string().contains("moved this loca to backend"));
    let (_, ops_owner) = get(format!("{base}/rooms/mv/operators"), &master).await;
    assert!(
        !ops_owner["appointed"].is_null(),
        "the Master stays operator"
    );
    // The outsiders' davets are gone, not only blocked.
    assert!(!ws_can(port, "mv", &format!("token={}", davets["mv-a-out"])).await);

    // Preview backend → default: the members are carried over by davet.
    let (_, back) = get(format!("{base}/rooms/mv/group/preview"), &master).await;
    assert_eq!(back["losing"], serde_json::json!([]));
    assert_eq!(
        back["carried_by_davet"],
        serde_json::json!(["mv-a-in", "mv-h-in", "mv-h-new"])
    );
    assert_eq!(
        put(
            format!("{base}/rooms/mv/group"),
            &master,
            Some(serde_json::json!({ "group": null }))
        )
        .await
        .0,
        200
    );
    let (_, invites) = get(format!("{base}/rooms/mv/invites"), &master).await;
    let invited: Vec<&str> = invites
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|i| i["name"].as_str())
        .collect();
    for name in ["mv-a-in", "mv-h-in", "mv-h-new"] {
        assert!(
            invited.contains(&name),
            "{name} keeps a seat by davet: {invites}"
        );
    }
    assert!(
        !invited.contains(&"mv-h-out") && !invited.contains(&"mv-a-out"),
        "outsiders do not come back"
    );
    // Back in the default group: ranks by rank, the agent by its kept davet.
    expect(
        // h-new was carried over by davet and now reaches it with its own key.
        snapshot("back to default").await,
        [true, true, true, true, false, true, true, false],
        "back to default",
    );

    // default → the empty ops group: only the Master remains.
    assert_eq!(
        put(
            format!("{base}/rooms/mv/group"),
            &master,
            Some(serde_json::json!({ "group": ops }))
        )
        .await
        .0,
        200
    );
    expect(
        snapshot("ops").await,
        [true, false, false, false, false, false, false, false],
        "ops",
    );

    // An archived group takes no loca.
    let (status, _) = send(
        reqwest::Method::PATCH,
        format!("{base}/groups/{backend}"),
        &[("x-session-token", master.as_str())],
        Some(serde_json::json!({ "archived": true })),
    )
    .await;
    assert!(status < 300);
    assert_eq!(
        put(
            format!("{base}/rooms/mv/group"),
            &master,
            Some(serde_json::json!({ "group": backend }))
        )
        .await
        .0,
        409
    );
}

#[tokio::test]
async fn a_person_signed_in_with_their_own_key_enters_where_they_hold_a_davet() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _guard) = closed_building(&dir.path().join("member-session.db")).await;
    let base = format!("http://127.0.0.1:{port}");
    let master = master_session(&base).await;
    let key = member(&base, "ms-ayse", "user").await;
    let other_key = member(&base, "ms-mehmet", "user").await;
    for room in ["ms-a", "ms-b"] {
        assert!(
            post(
                format!("{base}/rooms"),
                &master,
                serde_json::json!({ "name": room })
            )
            .await
            .0 < 300
        );
    }
    let (_, inv) = post(
        format!("{base}/rooms/ms-a/invites"),
        &master,
        serde_json::json!({ "name": "ms-ayse" }),
    )
    .await;
    let ayse = session(&base, ("x-room-token", &key), "ms-ayse", "user").await;
    let mehmet = session(&base, ("x-room-token", &other_key), "ms-mehmet", "user").await;
    // Her davet is her seat: in ms-a, not in ms-b.
    assert_eq!(
        get(format!("{base}/rooms/ms-a/messages"), &ayse).await.0,
        200
    );
    assert_ne!(
        get(format!("{base}/rooms/ms-b/messages"), &ayse).await.0,
        200
    );
    let (_, rooms) = get(format!("{base}/rooms"), &ayse).await;
    assert!(names_room(&rooms, "ms-a") && !names_room(&rooms, "ms-b"));
    // Someone without a davet stays out.
    assert_ne!(
        get(format!("{base}/rooms/ms-a/messages"), &mehmet).await.0,
        200
    );
    // A session does not carry a revoked davet.
    let token = inv["token"].as_str().unwrap();
    assert!(
        delete(format!("{base}/rooms/ms-a/invites/{token}"), &master)
            .await
            .0
            < 300
    );
    assert_ne!(
        get(format!("{base}/rooms/ms-a/messages"), &ayse).await.0,
        200
    );
}

#[tokio::test]
async fn leaving_a_group_ends_the_titles_held_in_its_locas() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _guard) = closed_building(&dir.path().join("titles.db")).await;
    let base = format!("http://127.0.0.1:{port}");
    let master = master_session(&base).await;
    let (_, founder) = smaster(&base, "tt-founder").await;
    let (_, deputy) = smaster(&base, "tt-deputy").await;
    let (_, g) = post(
        format!("{base}/groups"),
        &founder,
        serde_json::json!({ "name": "tt-group" }),
    )
    .await;
    let g = g["id"].as_str().unwrap().to_string();
    let deputy_id = principal_of(&base, "tt-deputy").await;
    assert!(
        put(
            format!("{base}/groups/{g}/members/{deputy_id}"),
            &founder,
            None
        )
        .await
        .0 < 300
    );
    // The deputy opens a loca in the group: it is that loca's operator; it leads.
    let (status, v) = post(
        format!("{base}/rooms"),
        &deputy,
        serde_json::json!({ "name": "tt-room", "group": g }),
    )
    .await;
    assert!(status < 300, "{status} {v}");
    assert!(
        post(
            format!("{base}/rooms/tt-room/lead"),
            &master,
            serde_json::json!({ "lead": "tt-deputy" })
        )
        .await
        .0 < 300
    );
    let (_, ops) = get(format!("{base}/rooms/tt-room/operators"), &master).await;
    assert_eq!(ops["appointed"]["principal_id"], deputy_id.as_str());
    // Removed from the group: out of the loca, and its titles end with it.
    assert!(
        delete(format!("{base}/groups/{g}/members/{deputy_id}"), &founder)
            .await
            .0
            < 300
    );
    assert_ne!(
        get(format!("{base}/rooms/tt-room/messages"), &deputy)
            .await
            .0,
        200
    );
    let (_, ops) = get(format!("{base}/rooms/tt-room/operators"), &master).await;
    assert!(ops["appointed"].is_null(), "appointment ended: {ops}");
    let (_, settings) = get(format!("{base}/rooms/tt-room/settings"), &master).await;
    assert!(settings["lead"].is_null(), "lead ended: {settings}");
}
