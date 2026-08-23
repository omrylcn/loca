//! Room authorization, sessions, and delegated authority.

use super::*;

#[tokio::test]
async fn room_token_gates_access() {
    let (port, _guard) = spawn_server_env("adm", &[("ROOM_TOKEN", "join".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // health advertises that a token is needed.
    let h: Value = client
        .get(format!("{base}/health"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(h["needs_token"], true);

    let msg = serde_json::json!({ "sender": "x", "sender_type": "agent", "text": "hi" });

    // No token -> 401.
    let no = client
        .post(format!("{base}/rooms/general/messages"))
        .json(&msg)
        .send()
        .await
        .unwrap();
    assert_eq!(no.status(), 401);

    // Room token -> ok.
    let ok = client
        .post(format!("{base}/rooms/general/messages"))
        .header("x-room-token", "join")
        .json(&msg)
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 201);

    // Admin token also counts as a member.
    let adm = client
        .post(format!("{base}/rooms/general/messages"))
        .header("x-admin-token", "adm")
        .json(&msg)
        .send()
        .await
        .unwrap();
    assert_eq!(adm.status(), 201);

    // Note create is gated too.
    let note = client
        .post(format!("{base}/rooms/general/notes"))
        .json(&serde_json::json!({ "key": "k", "by": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(note.status(), 401);
}

/// Control frames and note `can_write` reassignment require admin authority:
/// a non-admin WS control is dropped (not broadcast), and a note update can
/// only change `can_write` when the request carries the admin token — the
/// body itself carries no authority.
#[tokio::test]
async fn control_and_can_write_require_admin_authority() {
    let (port, _guard) = spawn_server_with("sekrit").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let mut watcher = connect_ws(port, "general", "watcher", "user").await;
    wait_for(&mut watcher, |v| v["t"] == "history").await;

    // Non-admin control frame: silently dropped.
    let mut plain = connect_ws(port, "general", "plain", "user").await;
    wait_for(&mut plain, |v| v["t"] == "history").await;
    plain
        .send(WsMessage::Text(r#"{"t":"control","cmd":"stop"}"#.into()))
        .await
        .unwrap();

    // Admin control frame (?admin= on the WS URL): broadcast.
    let url = format!("ws://127.0.0.1:{port}/ws?room=general&name=op&type=user&admin=sekrit");
    let (mut admin_ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    wait_for(&mut admin_ws, |v| v["t"] == "history").await;
    admin_ws
        .send(WsMessage::Text(r#"{"t":"control","cmd":"halt"}"#.into()))
        .await
        .unwrap();

    // The watcher sees ONLY the admin's control — the non-admin "stop" never
    // arrived (frames are delivered in order, so seeing "halt" first proves it).
    let ctl = wait_for(&mut watcher, |v| v["t"] == "control").await;
    assert_eq!(
        ctl["cmd"], "halt",
        "non-admin control must not be broadcast"
    );

    // ---- note can_write: only the admin token grants reassignment ----
    let r = client
        .post(format!("{base}/rooms/general/notes"))
        .json(&serde_json::json!({
            "key": "plan", "title": "Plan", "body": "v1",
            "by": "operator", "can_write": ["backend"]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);

    // Without the admin token: body/title update fine, can_write untouched.
    let r = client
        .put(format!("{base}/rooms/general/notes/plan"))
        .json(&serde_json::json!({ "by": "mallory", "body": "v2", "can_write": ["mallory"] }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let note: Value = r.json().await.unwrap();
    assert_eq!(note["body"], "v2");
    assert_eq!(
        note["can_write"],
        serde_json::json!(["backend"]),
        "can_write must not change without admin authority"
    );

    // With the admin token: reassignment succeeds.
    let r = client
        .put(format!("{base}/rooms/general/notes/plan"))
        .header("x-admin-token", "sekrit")
        .json(&serde_json::json!({ "by": "operator", "can_write": ["web"] }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let note: Value = r.json().await.unwrap();
    assert_eq!(note["can_write"], serde_json::json!(["web"]));
}

/// Session-bound identity: with REQUIRE_SESSIONS the server refuses posts
/// without a session token, and a valid token's identity OVERRIDES whatever
/// the body claims — so `sender` can no longer be spoofed.
#[tokio::test]
async fn sessions_bind_identity_and_can_be_required() {
    let (port, _guard) = spawn_server_env("", &[("REQUIRE_SESSIONS", "1".to_string())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // No session token -> refused.
    let r = client
        .post(format!("{base}/rooms/general/messages"))
        .json(&serde_json::json!({ "sender": "x", "sender_type": "user", "text": "hi" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);

    // Open a session bound to backend/agent.
    let r = client
        .post(format!("{base}/sessions"))
        .json(&serde_json::json!({ "name": "backend", "kind": "agent", "runtime": "codex" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    let sess: Value = r.json().await.unwrap();
    let token = sess["session_token"].as_str().unwrap().to_string();
    assert!(token.starts_with("st_"));

    // Post with a SPOOFED body identity: the session's identity wins.
    let r = client
        .post(format!("{base}/rooms/general/messages"))
        .header("x-session-token", &token)
        .json(&serde_json::json!({ "sender": "operator", "sender_type": "user", "text": "done" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    let msg: Value = r.json().await.unwrap();
    assert_eq!(
        msg["sender"], "backend",
        "sender must come from the session, not the body"
    );
    assert_eq!(msg["sender_type"], "agent");

    // Garbage token -> refused outright.
    let r = client
        .post(format!("{base}/rooms/general/messages"))
        .header("x-session-token", "st_bogus")
        .json(&serde_json::json!({ "sender": "backend", "sender_type": "agent", "text": "hi" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
}

/// A davet opens ONE loca. This is the rule the whole invitation model rests
/// on: the master lets you into a room, not into the building's every room.
#[tokio::test]
async fn davet_opens_one_loca_only() {
    let (port, _guard) = spawn_server_env("master", &[("ROOM_TOKEN", "building".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // Something worth reading in each loca.
    for room in ["mobile", "general"] {
        client
            .post(format!("{base}/rooms/{room}/messages"))
            .header("x-admin-token", "master")
            .json(
                &serde_json::json!({ "sender": "master", "sender_type": "user", "text": "inside" }),
            )
            .send()
            .await
            .unwrap();
    }

    // The master issues a davet for `mobile` — and only the master can.
    let refused = client
        .post(format!("{base}/rooms/mobile/invites"))
        .json(&serde_json::json!({ "name": "sb-feature" }))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 401, "only the master issues a davet");

    // A davet seats a member — admit first, then invite.
    admit(&base, "master", "sb-feature", "agent").await;
    let inv: Value = client
        .post(format!("{base}/rooms/mobile/invites"))
        .header("x-admin-token", "master")
        .json(&serde_json::json!({ "name": "sb-feature", "kind": "agent" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let davet = inv["token"].as_str().unwrap().to_string();
    assert_eq!(inv["room"], "mobile");

    // Its own loca: open, for reading and for speaking.
    let r = client
        .get(format!("{base}/rooms/mobile/messages"))
        .header("x-room-token", &davet)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "a davet opens its own loca");
    let w = client
        .post(format!("{base}/rooms/mobile/messages"))
        .header("x-room-token", &davet)
        .json(
            &serde_json::json!({ "sender": "sb-feature", "sender_type": "agent", "text": "hello" }),
        )
        .send()
        .await
        .unwrap();
    assert!(
        w.status().is_success(),
        "a davet lets you speak in your loca"
    );

    // The loca next door: shut. Reading included — a room's history is the
    // group's, and this is exactly the leak that reads-are-public would be.
    for path in [
        "/rooms/general/messages",
        "/rooms/general/members",
        "/rooms/general/notes",
        "/rooms/general/search?q=inside",
    ] {
        let r = client
            .get(format!("{base}{path}"))
            .header("x-room-token", &davet)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 401, "a mobile davet must not open {path}");
    }
    let w = client
        .post(format!("{base}/rooms/general/messages"))
        .header("x-room-token", &davet)
        .json(
            &serde_json::json!({ "sender": "sb-feature", "sender_type": "agent", "text": "leak" }),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(w.status(), 401, "a mobile davet must not speak in general");

    // Revoking ends it — the davet is the invitation, so taking it back is
    // taking the invitation back.
    let del = client
        .delete(format!("{base}/rooms/mobile/invites/{davet}"))
        .header("x-admin-token", "master")
        .send()
        .await
        .unwrap();
    assert!(del.status().is_success());
    let after = client
        .get(format!("{base}/rooms/mobile/messages"))
        .header("x-room-token", &davet)
        .send()
        .await
        .unwrap();
    assert_eq!(after.status(), 401, "a revoked davet opens nothing");
}

/// Taking a session must not widen a davet. The invited need an identity to
/// speak at all, so `/sessions` accepts a davet — but the session it hands
/// back reaches exactly one loca, otherwise the davet would mean nothing.
#[tokio::test]
async fn a_session_never_widens_a_davet() {
    let (port, _guard) = spawn_server_env("master", &[("ROOM_TOKEN", "building".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let davet_owned = davet_for(&base, "master", "mobile", "sb-feature").await;
    let davet = davet_owned.as_str();

    // The invited can take an identity — refusing this would leave them mute.
    let sess: Value = client
        .post(format!("{base}/sessions"))
        .header("x-room-token", davet)
        .json(&serde_json::json!({ "name": "sb-feature", "kind": "agent" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let st = sess["session_token"].as_str().unwrap();

    let own = client
        .get(format!("{base}/rooms/mobile/messages"))
        .header("x-session-token", st)
        .send()
        .await
        .unwrap();
    assert_eq!(own.status(), 200, "the session opens the davet's loca");

    let other = client
        .get(format!("{base}/rooms/general/messages"))
        .header("x-session-token", st)
        .send()
        .await
        .unwrap();
    assert_eq!(
        other.status(),
        401,
        "and no other — a session is not a skeleton key"
    );

    // The building key's session keeps its old reach (nothing regressed).
    let wide: Value = client
        .post(format!("{base}/sessions"))
        .header("x-room-token", "building")
        .json(&serde_json::json!({ "name": "browser", "kind": "user" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let r = client
        .get(format!("{base}/rooms/general/messages"))
        .header("x-session-token", wide["session_token"].as_str().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        200,
        "a building-key session still reaches the building"
    );
}

/// The browser connects with a session and no ?token=. If the WS door judges
/// the davet before reading the session, every seated operator is turned away
/// — which is exactly what happened on prod: join/leave twice a second, the
/// page reloading forever.
#[tokio::test]
async fn a_session_alone_opens_the_ws_door() {
    let (port, _guard) = spawn_server_env("master", &[("ROOM_TOKEN", "building".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let sess: Value = client
        .post(format!("{base}/sessions"))
        .header("x-room-token", "building")
        .json(&serde_json::json!({ "name": "operator", "kind": "user" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let st = sess["session_token"].as_str().unwrap();

    // No token in the URL — the session is the whole credential.
    let url = format!("ws://127.0.0.1:{port}/ws?room=general&name=operator&type=user&session={st}");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("a session alone must open the door");

    // And it stays open: history arrives rather than an immediate close.
    let frame = wait_for(&mut ws, |v| v["t"] == "history").await;
    assert_eq!(frame["t"], "history");
}

/// A smaster is a second master: everything a master does — issue davets, run
/// a loca — except that the master has the last word. So a smaster cannot undo
/// what the master decided, and cannot appoint another smaster.
#[tokio::test]
async fn a_smaster_does_everything_but_the_master_has_the_last_word() {
    let (port, _guard) = spawn_server_env("MASTER", &[("ROOM_TOKEN", "building".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // Only the master appoints one.
    let refused = client
        .post(format!("{base}/smasters"))
        .header("x-admin-token", "building")
        .json(&serde_json::json!({ "name": "murat" }))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 401);

    let sm: Value = client
        .post(format!("{base}/smasters"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "murat" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let key = sm["token"].as_str().unwrap().to_string();

    // Everything a master does. (The members exist first — a davet seats a
    // member, for smasters exactly as for the master.)
    admit(&base, "MASTER", "veli", "agent").await;
    admit(&base, "MASTER", "ali", "agent").await;
    let own: Value = client
        .post(format!("{base}/rooms/general/invites"))
        .header("x-admin-token", &key)
        .json(&serde_json::json!({ "name": "veli" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        own["token"].as_str().unwrap().starts_with("dv_"),
        "a smaster issues davets"
    );

    let mine = client
        .delete(format!(
            "{base}/rooms/general/invites/{}",
            own["token"].as_str().unwrap()
        ))
        .header("x-admin-token", &key)
        .send()
        .await
        .unwrap();
    assert!(mine.status().is_success(), "and ends the ones they issued");

    // …except undoing the master.
    let masters: Value = client
        .post(format!("{base}/rooms/general/invites"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "ali" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mtoken = masters["token"].as_str().unwrap();

    let blocked = client
        .delete(format!("{base}/rooms/general/invites/{mtoken}"))
        .header("x-admin-token", &key)
        .send()
        .await
        .unwrap();
    assert_eq!(
        blocked.status(),
        403,
        "the master's davet is the master's to end"
    );

    let allowed = client
        .delete(format!("{base}/rooms/general/invites/{mtoken}"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap();
    assert!(
        allowed.status().is_success(),
        "and the master may end anything"
    );

    // A smaster cannot grow the circle of authority.
    let no = client
        .post(format!("{base}/smasters"))
        .header("x-admin-token", &key)
        .json(&serde_json::json!({ "name": "someone" }))
        .send()
        .await
        .unwrap();
    assert_eq!(no.status(), 401, "authority flows from the master only");

    // The master can take it back, and then the key is just a string.
    let gone = client
        .delete(format!("{base}/smasters/{key}"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap();
    assert!(gone.status().is_success());
    let after = client
        .post(format!("{base}/rooms/general/invites"))
        .header("x-admin-token", &key)
        .json(&serde_json::json!({ "name": "nobody" }))
        .send()
        .await
        .unwrap();
    assert_eq!(after.status(), 401, "a revoked smaster acts no more");
}

/// The hierarchy, checked where it actually bites. Three layers, and the rule
/// is vertical: the one above contains the one below, and the one below cannot
/// undo the one above.
#[tokio::test]
async fn the_hierarchy_holds_at_every_door() {
    let (port, _guard) = spawn_server_env("MASTER", &[("ROOM_TOKEN", "building".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let sm: Value = client
        .post(format!("{base}/smasters"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "murat" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let smaster = sm["token"].as_str().unwrap().to_string();

    // A smaster reaches every loca without a davet — they are meant to run any
    // room, so being stopped at the door would make the title hollow.
    let reach = client
        .get(format!("{base}/rooms/oda/messages"))
        .header("x-admin-token", &smaster)
        .send()
        .await
        .unwrap();
    assert_eq!(
        reach.status(),
        200,
        "a smaster is not a stranger to any loca"
    );

    // A loca operator's authority stops at their own door: no membership, no
    // davets, no smasters, and no writing themselves further powers.
    for (method, path, body) in [
        ("POST", "/members", serde_json::json!({ "name": "x" })),
        (
            "POST",
            "/rooms/oda/invites",
            serde_json::json!({ "name": "x" }),
        ),
        ("POST", "/smasters", serde_json::json!({ "name": "x" })),
    ] {
        let r = client
            .request(method.parse().unwrap(), format!("{base}{path}"))
            .header("x-room-token", "building")
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            401,
            "{path} belongs to the building layer, not a loca"
        );
    }

    // Naming a lead is an explicit action: a smaster may, and the master's
    // word lands on top.
    let set_lead = |token: String, who: &'static str| {
        let (c, b) = (client.clone(), base.clone());
        async move {
            c.post(format!("{b}/rooms/oda/lead"))
                .header("x-admin-token", token)
                .json(&serde_json::json!({ "lead": who }))
                .send()
                .await
                .unwrap()
        }
    };
    let lead = || {
        let (c, b) = (client.clone(), base.clone());
        async move {
            let s: Value = c
                .get(format!("{b}/rooms/oda/settings"))
                .header("x-admin-token", "MASTER")
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            s["lead"].as_str().map(str::to_string)
        }
    };

    set_lead(smaster.clone(), "sb-feature").await;
    assert_eq!(
        lead().await.as_deref(),
        Some("sb-feature"),
        "a smaster runs a loca"
    );

    set_lead("MASTER".into(), "debug").await;
    assert_eq!(
        lead().await.as_deref(),
        Some("debug"),
        "and the master has the last word"
    );
}

/// THE core smoke test: davet-only mode (no building key). This is prod.
/// Every combination of who × where × how, checked at once.
#[tokio::test]
async fn door_matrix_davet_only_mode() {
    let (port, _g) = spawn_server_env("MASTER", &[("REQUIRE_INVITE", "1".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // Two locas exist (a message from the master creates them).
    for room in ["general", "sb-dev"] {
        client
            .post(format!("{base}/rooms/{room}/messages"))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({ "sender": "m", "sender_type": "user", "text": "hi" }))
            .send()
            .await
            .unwrap();
    }

    // cyber holds a davet for general only.
    let cyber_davet = davet_for(&base, "MASTER", "general", "cyber").await;
    let cyber_session = session_with(
        &base,
        ("x-room-token", &cyber_davet),
        "cyber",
        Some("general"),
    )
    .await;

    // --- MASTER: every loca, no davet, both channels ---
    assert_eq!(
        rest_can(&base, "general", &[("x-admin-token", "MASTER")]).await,
        200,
        "master reads general"
    );
    assert_eq!(
        rest_can(&base, "sb-dev", &[("x-admin-token", "MASTER")]).await,
        200,
        "master reads sb-dev uninvited"
    );
    assert!(
        ws_can(port, "sb-dev", "admin=MASTER").await,
        "master WS into sb-dev uninvited"
    );

    // --- cyber: their own loca opens, the neighbour's does not (THE bug) ---
    assert_eq!(
        rest_can(&base, "general", &[("x-room-token", &cyber_davet)]).await,
        200,
        "cyber davet -> general"
    );
    assert_eq!(
        rest_can(&base, "sb-dev", &[("x-room-token", &cyber_davet)]).await,
        401,
        "cyber davet -> sb-dev shut"
    );
    // The exact hole: a building-key-less session must NOT open another loca.
    assert_eq!(
        rest_can(&base, "sb-dev", &[("x-session-token", &cyber_session)]).await,
        401,
        "cyber SESSION -> sb-dev shut (the hole, now closed)"
    );
    assert!(
        !ws_can(port, "sb-dev", &format!("session={cyber_session}")).await,
        "cyber SESSION WS -> sb-dev shut"
    );
    // But their own loca still opens over both channels.
    assert_eq!(
        rest_can(&base, "general", &[("x-session-token", &cyber_session)]).await,
        200,
        "cyber session -> general"
    );
    assert!(
        ws_can(port, "general", &format!("token={cyber_davet}")).await,
        "cyber davet WS -> general"
    );

    // --- stranger: nothing ---
    assert_eq!(
        rest_can(&base, "general", &[]).await,
        401,
        "stranger reads nothing"
    );
    assert!(!ws_can(port, "general", "").await, "stranger WS nothing");

    // --- smaster: rank, every loca ---
    let sm: Value = client
        .post(format!("{base}/smasters"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "murat" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let smk = sm["token"].as_str().unwrap();
    assert_eq!(
        rest_can(&base, "sb-dev", &[("x-admin-token", smk)]).await,
        200,
        "smaster reads sb-dev uninvited"
    );
    assert!(
        ws_can(port, "sb-dev", &format!("admin={smk}")).await,
        "smaster WS into sb-dev"
    );
}

/// The other mode: a building key is set (dev / legacy). The browser takes a
/// session once and stops sending the key, so a loca-less session MUST still
/// reach the building — otherwise the web client breaks.
#[tokio::test]
async fn door_matrix_building_key_mode() {
    let (port, _g) = spawn_server_env("MASTER", &[("ROOM_TOKEN", "building".into())]).await;
    let base = format!("http://127.0.0.1:{port}");

    // The browser's path: session taken with the building key, no loca scope.
    let sess = session_with(&base, ("x-room-token", "building"), "browser", None).await;
    assert_eq!(
        rest_can(&base, "general", &[("x-session-token", &sess)]).await,
        200,
        "building-key session reaches general"
    );
    assert_eq!(
        rest_can(&base, "sb-dev", &[("x-session-token", &sess)]).await,
        200,
        "building-key session reaches sb-dev too (browser relies on this)"
    );
    assert!(
        ws_can(port, "sb-dev", &format!("session={sess}")).await,
        "building-key session WS reaches any loca"
    );

    // A davet still scopes to one loca even here.
    let davet = davet_for(&base, "MASTER", "general", "cyber").await;
    let scoped = session_with(&base, ("x-room-token", &davet), "cyber", Some("general")).await;
    assert_eq!(
        rest_can(&base, "sb-dev", &[("x-session-token", &scoped)]).await,
        401,
        "a davet-scoped session stays in its loca even in key mode"
    );
}

/// A banned name must stay out even holding a valid session token — the ban is
/// judged against the session's own name (enter_decision resolves it there),
/// not only against a davet or a ?name= query. In davet-only prod a session is
/// the common credential, so this is the path that matters.
#[tokio::test]
async fn a_ban_holds_against_a_valid_session() {
    let (port, _g) = spawn_server_env("MASTER", &[("REQUIRE_INVITE", "1".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    client
        .post(format!("{base}/rooms/oda/messages"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "sender": "m", "sender_type": "user", "text": "hi" }))
        .send()
        .await
        .unwrap();

    // kotu holds a davet and a session scoped to oda.
    let davet = davet_for(&base, "MASTER", "oda", "kotu").await;
    let sess = session_with(&base, ("x-room-token", &davet), "kotu", Some("oda")).await;
    assert_eq!(
        rest_can(&base, "oda", &[("x-session-token", &sess)]).await,
        200,
        "before ban: session works"
    );

    // Ban the name. The session token is still valid, but the name is barred.
    client
        .post(format!("{base}/rooms/oda/moderate"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "action": "ban", "name": "kotu" }))
        .send()
        .await
        .unwrap();

    assert_eq!(
        rest_can(&base, "oda", &[("x-session-token", &sess)]).await,
        401,
        "a ban holds against the session's own name, not just davet/?name="
    );
    assert!(
        !ws_can(port, "oda", &format!("session={sess}")).await,
        "and the same over WS"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// FAZ 1 — anayasa uyumu: izolasyon, watcher, ban kalıcılığı, dirilme,
// kick-durdurur-daveti, release-hayalet-bırakmaz.
// ─────────────────────────────────────────────────────────────────────────────

/// A davet for one loca cannot read another loca's internals — mode, settings,
/// moderation lists, a note by key, or the task list. This is the isolation
/// leak the RoomAccess extractor closes: any of these used to answer to any
/// davet holder regardless of which loca it opened.
#[tokio::test]
async fn a_davet_for_one_loca_cannot_read_another() {
    let (port, _g) = spawn_server_env("MASTER", &[("REQUIRE_INVITE", "1".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    // Two locas exist (master's messages create them).
    for room in ["mine", "yours"] {
        client
            .post(format!("{base}/rooms/{room}/messages"))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({ "sender": "master", "sender_type": "user", "text": "hi" }))
            .send()
            .await
            .unwrap();
    }
    let d = davet_for(&base, "MASTER", "mine", "cyber").await;
    // Its own loca: readable.
    let own = client
        .get(format!("{base}/rooms/mine/mode"))
        .header("x-room-token", &d)
        .send()
        .await
        .unwrap();
    assert_eq!(own.status(), 200, "a davet reads its own loca's mode");
    // The loca next door: every member-level read protected by RoomAccess is
    // shut. Keep this list broad so a future route cannot silently fall back
    // to the building-wide middleware and leak another loca.
    for path in [
        "messages",
        "members",
        "journal",
        "mode",
        "settings",
        "moderate",
        "notes",
        "notes/anything",
        "notes/anything/history",
        "search?q=secret",
        "tasks",
        "goals",
        "waits",
    ] {
        let r = client
            .get(format!("{base}/rooms/yours/{path}"))
            .header("x-room-token", &d)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 401, "a mine-davet must not read yours/{path}");
    }
    // The same invariant covers every ordinary member mutation. Bodies are
    // intentionally minimal: RoomAccess must reject the request before a
    // handler can parse or act on untrusted content.
    for (method, path) in [
        (reqwest::Method::POST, "messages"),
        (reqwest::Method::POST, "journal"),
        (reqwest::Method::POST, "notes"),
        (reqwest::Method::PUT, "notes/anything"),
        (reqwest::Method::DELETE, "notes/anything"),
        (reqwest::Method::POST, "tasks"),
        (reqwest::Method::PATCH, "tasks/1"),
        (reqwest::Method::POST, "goals"),
        (reqwest::Method::PATCH, "goals/1"),
        (reqwest::Method::POST, "waits"),
        (reqwest::Method::DELETE, "waits/worker"),
        (reqwest::Method::POST, "care/signal/ack"),
    ] {
        let r = client
            .request(method.clone(), format!("{base}/rooms/yours/{path}"))
            .header("x-room-token", &d)
            .json(&serde_json::json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            401,
            "a mine-davet must not {} yours/{path}",
            method.as_str()
        );
    }
    // And the loca list shows only what the davet opens.
    let rooms: Vec<Value> = client
        .get(format!("{base}/rooms"))
        .header("x-room-token", &d)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let names: Vec<&str> = rooms.iter().map(|r| r["room"].as_str().unwrap()).collect();
    assert!(
        names.contains(&"mine") && !names.contains(&"yours"),
        "the list shows the locas you may enter, not the whole building"
    );
}

/// A watcher (?watch=1) reads the stream but cannot speak — no seat, no voice.
#[tokio::test]
async fn a_watcher_cannot_speak() {
    let (port, _g) = spawn_server_env("MASTER", &[]).await;
    let base = format!("http://127.0.0.1:{port}");
    // A real member is seated to receive.
    let mut member = connect_ws(port, "oda", "member", "agent").await;
    let _ = wait_for(&mut member, |v| v["t"] == "history").await;
    // A watcher joins and tries to speak.
    let (mut watcher, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/ws?room=oda&name=ghost&type=agent&watch=1"
    ))
    .await
    .unwrap();
    use futures_util::SinkExt;
    watcher
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::json!({ "t": "send", "text": "can you hear me?" }).to_string(),
        ))
        .await
        .unwrap();
    // The member must NOT receive it — the watcher is read-only.
    let heard = tokio::time::timeout(std::time::Duration::from_millis(600), async {
        loop {
            use futures_util::StreamExt;
            if let Some(Ok(tokio_tungstenite::tungstenite::Message::Text(t))) = member.next().await
            {
                let v: Value = serde_json::from_str(&t).unwrap();
                if v["t"] == "msg" && v["message"]["text"] == "can you hear me?" {
                    break true;
                }
            }
        }
    })
    .await
    .unwrap_or(false);
    assert!(!heard, "a watcher's words never reach the table");
    // And it never took a seat.
    let members: Vec<Value> = reqwest::Client::new()
        .get(format!("{base}/rooms/oda/members"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        !members.iter().any(|m| m["name"] == "ghost"),
        "a watcher holds no seat"
    );
}

/// Revoking a DAVET kills the session minted from it: the old session token no
/// longer opens the loca. The door is shut in fact, not just on paper.
#[tokio::test]
async fn revoking_a_davet_kills_its_session() {
    let (port, _g) = spawn_server_env("MASTER", &[("REQUIRE_INVITE", "1".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let d = davet_for(&base, "MASTER", "oda", "cyber").await;
    let st = session_with(&base, ("x-room-token", &d), "cyber", None).await;
    // The session opens the loca.
    assert_eq!(
        client
            .get(format!("{base}/rooms/oda/messages"))
            .header("x-session-token", &st)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    // Revoke the davet.
    client
        .delete(format!("{base}/rooms/oda/invites/{d}"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap();
    // The session is dead too — cascade, not just the davet.
    assert_eq!(
        client
            .get(format!("{base}/rooms/oda/messages"))
            .header("x-session-token", &st)
            .send()
            .await
            .unwrap()
            .status(),
        401,
        "a revoked davet's session stops working"
    );
}

/// A loca operator (not the master) runs their own loca's mode and moderation,
/// and only their own (PRINCIPLES: operator manages mod/moderasyon).
#[tokio::test]
async fn operator_manages_own_loca_not_others() {
    let (port, _g) = spawn_server_env("MASTER", &[("REQUIRE_SESSIONS", "1".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    // Two locas; "op" is operator of oda1 only.
    for r in ["oda1", "oda2"] {
        client
            .post(format!("{base}/rooms/{r}/messages"))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({ "sender": "m", "sender_type": "user", "text": "hi" }))
            .send()
            .await
            .unwrap();
    }
    client
        .put(format!("{base}/rooms/oda1/settings"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "operators": ["op"] }))
        .send()
        .await
        .unwrap();
    // op is a real member with a davet — its session is its OWN identity, NOT an
    // admin session (that would make it master everywhere, which is the bug the
    // admin-session work must not introduce). op is operator of oda1 only.
    let d1 = davet_for(&base, "MASTER", "oda1", "op").await;
    let st = session_with(&base, ("x-room-token", &d1), "op", None).await;
    // op sets the mode in their own loca.
    let own = client
        .put(format!("{base}/rooms/oda1/mode"))
        .header("x-session-token", &st)
        .json(&serde_json::json!({ "mode": { "mode": "paused" } }))
        .send()
        .await
        .unwrap();
    assert!(
        own.status().is_success(),
        "an operator runs their own loca's mode"
    );
    // But not in a loca they don't operate.
    let other = client
        .put(format!("{base}/rooms/oda2/mode"))
        .header("x-session-token", &st)
        .json(&serde_json::json!({ "mode": { "mode": "paused" } }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        other.status(),
        401,
        "an operator's power ends at their loca's door"
    );
}

/// The browser pairs with a one-use stand-in: the root key never enters the
/// browser, the issued token is cryptographically sized and role/expiry are
/// explicit, and logout revokes it server-side.
#[tokio::test]
async fn master_pairing_is_one_use_and_logout_revokes_session() {
    let (port, _g) = spawn_server_env("MASTER", &[("REQUIRE_INVITE", "1".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // The duration is bounded at the server, not only in the panel.
    assert_eq!(
        client
            .post(format!("{base}/pairings?ttl_hours=0"))
            .header("x-admin-token", "MASTER")
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        client
            .post(format!("{base}/pairings?ttl_hours=8761"))
            .header("x-admin-token", "MASTER")
            .send()
            .await
            .unwrap()
            .status(),
        400
    );

    // The terminal/admin console asks the server for a one-use browser code.
    let pairing_created_after = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let pairing: Value = client
        .post(format!("{base}/pairings?ttl_hours=168"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let code = pairing["pairing_code"].as_str().unwrap();
    assert!(code.starts_with("pair_"));
    assert_eq!(pairing["session_ttl_hours"], 168);
    let pairing_expires_at = pairing["pairing_expires_at"].as_u64().unwrap();
    assert!(
        (pairing_created_after + 5 * 60 * 1000..=pairing_created_after + 5 * 60 * 1000 + 5_000)
            .contains(&pairing_expires_at),
        "the one-use code itself expires after five minutes"
    );

    // The browser sends only the pairing code, never the root key.
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let response = client
        .post(format!("{base}/sessions"))
        .header("x-pairing-code", code)
        // A stale davet field cannot override the explicit master pairing.
        .header("x-room-token", "stale-davet")
        .json(&serde_json::json!({ "name": "master", "kind": "user" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let session: Value = response.json().await.unwrap();
    assert_eq!(session["admin"], true);
    let expires_at = session["expires_at"].as_u64().unwrap();
    let seven_days_ms = 168 * 60 * 60 * 1000;
    assert!(
        (before + seven_days_ms..=before + seven_days_ms + 5_000).contains(&expires_at),
        "the one-use code carries its selected seven-day lifetime"
    );
    let token = session["session_token"].as_str().unwrap();
    assert!(token.starts_with("st_"));
    assert_eq!(token.len(), 67, "32 random bytes encoded as hex");

    let replay = client
        .post(format!("{base}/sessions"))
        .header("x-pairing-code", code)
        .json(&serde_json::json!({ "name": "intruder", "kind": "user" }))
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 401, "a pairing code works exactly once");

    let admin_act = client
        .put(format!("{base}/rooms/general/mode"))
        .header("x-session-token", token)
        .json(&serde_json::json!({ "mode": { "mode": "paused" } }))
        .send()
        .await
        .unwrap();
    assert!(admin_act.status().is_success());

    let logout = client
        .delete(format!("{base}/sessions"))
        .header("x-session-token", token)
        .send()
        .await
        .unwrap();
    assert_eq!(logout.status(), 204);
    let after_logout = client
        .put(format!("{base}/rooms/general/mode"))
        .header("x-session-token", token)
        .json(&serde_json::json!({ "mode": { "mode": "free" } }))
        .send()
        .await
        .unwrap();
    assert_eq!(after_logout.status(), 401);
}

/// The browser can hold a short-lived ADMIN SESSION instead of the raw master
/// key: it exchanges the key once, then every admin act is proven by the
/// session — the raw key never has to travel again (PRINCIPLES: the master key
/// never leaves .env; the browser holds a stand-in).
#[tokio::test]
async fn admin_session_grants_admin_without_raw_key() {
    let (port, _g) = spawn_server_env("MASTER", &[]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    // Exchange the master key for an admin session ONCE.
    let sess: Value = client
        .post(format!("{base}/sessions"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "master", "kind": "user" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let st = sess["session_token"].as_str().unwrap();
    // Now an admin-only act (set mode) succeeds with ONLY the session — no key.
    let ok = client
        .put(format!("{base}/rooms/general/mode"))
        .header("x-session-token", st)
        .json(&serde_json::json!({ "mode": { "mode": "paused" } }))
        .send()
        .await
        .unwrap();
    assert!(
        ok.status().is_success(),
        "an admin session grants admin without the raw key"
    );
    // A NON-admin session (davet-scoped) does NOT grant admin.
    let d = davet_for(&base, "MASTER", "oda", "cyber").await;
    let dsess = session_with(&base, ("x-room-token", &d), "cyber", None).await;
    let denied = client
        .put(format!("{base}/rooms/oda/mode"))
        .header("x-session-token", &dsess)
        .json(&serde_json::json!({ "mode": { "mode": "paused" } }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        denied.status(),
        401,
        "a davet session is not an admin session"
    );
    // The raw key still works too (the old path is not broken).
    let key_ok = client
        .put(format!("{base}/rooms/general/mode"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "mode": { "mode": "free" } }))
        .send()
        .await
        .unwrap();
    assert!(key_ok.status().is_success(), "the raw key path still works");
}

/// A deploy restarts the process, not the master's working day. The browser's
/// expiring admin stand-in survives through SQLite, while explicit logout is
/// still final and stays revoked after another restart.
#[tokio::test]
async fn admin_session_survives_restart_but_logout_survives_too() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir
        .path()
        .join("admin-session.db")
        .to_string_lossy()
        .to_string();
    let client = reqwest::Client::new();
    let token;
    {
        let (port, _g) = spawn_server_env(
            "MASTER",
            &[("DB_PATH", db.clone()), ("REQUIRE_INVITE", "1".into())],
        )
        .await;
        let base = format!("http://127.0.0.1:{port}");
        let session: Value = client
            .post(format!("{base}/sessions"))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({ "name": "operator", "kind": "user" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        token = session["session_token"].as_str().unwrap().to_string();
    }
    {
        let (port, _g) = spawn_server_env(
            "MASTER",
            &[("DB_PATH", db.clone()), ("REQUIRE_INVITE", "1".into())],
        )
        .await;
        let base = format!("http://127.0.0.1:{port}");
        assert_eq!(
            client
                .get(format!("{base}/rooms"))
                .header("x-session-token", &token)
                .send()
                .await
                .unwrap()
                .status(),
            200,
            "deploy must not empty the master's sidebar"
        );
        assert_eq!(
            client
                .delete(format!("{base}/sessions"))
                .header("x-session-token", &token)
                .send()
                .await
                .unwrap()
                .status(),
            204
        );
    }
    let (port, _g) =
        spawn_server_env("MASTER", &[("DB_PATH", db), ("REQUIRE_INVITE", "1".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    assert_eq!(
        client
            .get(format!("{base}/rooms"))
            .header("x-session-token", &token)
            .send()
            .await
            .unwrap()
            .status(),
        401,
        "logout must remain revoked after restart"
    );
}

/// The prod scenario the first admin-session test MISSED: in davet-only mode
/// (REQUIRE_INVITE=1) an admin session must open the door too — read a loca's
/// messages, list rooms, connect over WS. Before the fix `enter_decision`
/// ignored `SessionIdentity.admin`, so the master's session 401'd at every room
/// gate while non-door acts (mode) still worked — the browser saw no locas and
/// the WS handshake was refused.
#[tokio::test]
async fn admin_session_opens_the_door_in_davet_mode() {
    let (port, _g) = spawn_server_env(
        "MASTER",
        &[
            ("REQUIRE_INVITE", "1".into()),
            ("REQUIRE_SESSIONS", "1".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    // Exchange the key for an admin session (pure admin, no davet).
    let sess: Value = client
        .post(format!("{base}/sessions"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "master", "kind": "user" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let st = sess["session_token"].as_str().unwrap();
    // The door: room list, message read, mode — all with ONLY the session.
    assert_eq!(
        client
            .get(format!("{base}/rooms"))
            .header("x-session-token", st)
            .send()
            .await
            .unwrap()
            .status(),
        200,
        "admin session lists rooms"
    );
    assert_eq!(
        client
            .get(format!("{base}/rooms/general/messages"))
            .header("x-session-token", st)
            .send()
            .await
            .unwrap()
            .status(),
        200,
        "admin session reads a loca (the door bug)"
    );
    assert!(
        client
            .put(format!("{base}/rooms/general/mode"))
            .header("x-session-token", st)
            .json(&serde_json::json!({ "mode": { "mode": "free" } }))
            .send()
            .await
            .unwrap()
            .status()
            .is_success(),
        "admin session sets mode"
    );
    // The WS door opens with the admin session (no ?admin= key on the URL).
    let ok = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/ws?room=general&name=master&type=user&session={st}"
    ))
    .await;
    assert!(ok.is_ok(), "admin session opens the WS door in davet mode");
    // Revoking a davet and moderation also work by session alone.
    let d = davet_for(&base, "MASTER", "oda", "cyber").await;
    assert_eq!(
        client
            .delete(format!("{base}/rooms/oda/invites/{d}"))
            .header("x-session-token", st)
            .send()
            .await
            .unwrap()
            .status(),
        200,
        "admin session revokes a davet"
    );
}

/// Public clients keep bearer credentials out of URLs. A davet presented in
/// the protocol header also owns its authoritative server-side name, so an
/// alias cannot survive revocation under a label the revoke broadcast misses.
#[tokio::test]
async fn websocket_header_auth_hides_bearer_and_revoke_closes_alias() {
    let (port, _guard) = spawn_server_env(
        "MASTER",
        &[
            ("REQUIRE_INVITE", "1".into()),
            ("LEGACY_WS_QUERY_AUTH", "0".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let davet = davet_for(&base, "MASTER", "secret", "real-agent").await;

    let legacy_url =
        format!("ws://127.0.0.1:{port}/ws?room=secret&name=alias&type=agent&token={davet}");
    assert!(
        tokio_tungstenite::connect_async(legacy_url).await.is_err(),
        "query-string bearer auth must be refused by default"
    );

    let url = format!("ws://127.0.0.1:{port}/ws?room=secret&name=alias&type=agent");
    let mut ws = connect_ws_protocols(url, &["loca.v1".into(), format!("loca.room.{davet}")]).await;
    let roster = wait_for(&mut ws, |value| value["t"] == "members").await;
    assert!(roster["members"]
        .as_array()
        .unwrap()
        .iter()
        .any(|member| member["name"] == "real-agent"));
    assert!(!roster["members"]
        .as_array()
        .unwrap()
        .iter()
        .any(|member| member["name"] == "alias"));

    client
        .delete(format!("{base}/rooms/secret/invites/{davet}"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let closed = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match ws.next().await {
                Some(Ok(WsMessage::Text(text))) => {
                    let value: Value = serde_json::from_str(&text).unwrap();
                    if value["t"] == "kicked" {
                        assert_eq!(value["name"], "real-agent");
                    }
                }
                Some(Ok(WsMessage::Close(_))) | None => return,
                Some(Ok(_)) => {}
                Some(Err(_)) => return,
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "revocation must close the live socket");
}

/// WebSocket authority is live, not a handshake snapshot. Logging out an
/// admin session must immediately prevent that already-open socket from
/// broadcasting a control frame.
#[tokio::test]
async fn admin_session_logout_revokes_open_websocket_control() {
    let (port, _guard) = spawn_server_env("MASTER", &[("LEGACY_WS_QUERY_AUTH", "0".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let session = session_with(&base, ("x-admin-token", "MASTER"), "operator", None).await;
    let url = format!("ws://127.0.0.1:{port}/ws?room=general&name=operator&type=user");
    let mut admin_ws =
        connect_ws_protocols(url, &["loca.v1".into(), format!("loca.session.{session}")]).await;
    wait_for(&mut admin_ws, |value| value["t"] == "history").await;

    let observer_url =
        format!("ws://127.0.0.1:{port}/ws?room=general&name=observer&type=user&watch=1");
    let mut observer = connect_ws_protocols(
        observer_url,
        &["loca.v1".into(), "loca.admin.MASTER".into()],
    )
    .await;
    wait_for(&mut observer, |value| value["t"] == "history").await;

    client
        .delete(format!("{base}/sessions"))
        .header("x-session-token", &session)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    admin_ws
        .send(WsMessage::Text(
            r#"{"t":"control","cmd":"must-not-arrive"}"#.into(),
        ))
        .await
        .unwrap();

    let control = tokio::time::timeout(Duration::from_millis(500), async {
        loop {
            let Some(frame) = observer.next().await else {
                return false;
            };
            let Ok(WsMessage::Text(text)) = frame else {
                continue;
            };
            let value: Value = serde_json::from_str(&text).unwrap();
            if value["t"] == "control" && value["cmd"] == "must-not-arrive" {
                return true;
            }
        }
    })
    .await;
    assert!(
        control.is_err(),
        "a logged-out admin socket must not retain control authority"
    );
}
