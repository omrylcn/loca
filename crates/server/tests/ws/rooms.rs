//! Room lifecycle, capacity, moderation, release, and lead state.

use super::*;

#[tokio::test]
async fn moderation_mute_kick_ban() {
    let (port, _guard) = spawn_server_with("adm").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let post = |sender: &'static str| {
        let (client, base) = (client.clone(), base.clone());
        async move {
            client
                .post(format!("{base}/rooms/general/messages"))
                .json(
                    &serde_json::json!({ "sender": sender, "sender_type": "agent", "text": "hi" }),
                )
                .send()
                .await
                .unwrap()
                .status()
                .as_u16()
        }
    };
    let mod_act = |action: &'static str, name: &'static str| {
        let (client, base) = (client.clone(), base.clone());
        async move {
            client
                .post(format!("{base}/rooms/general/moderate"))
                .header("x-admin-token", "adm")
                .json(&serde_json::json!({ "action": action, "name": name }))
                .send()
                .await
                .unwrap()
                .status()
                .as_u16()
        }
    };

    // Moderate without admin token -> 401.
    let no = client
        .post(format!("{base}/rooms/general/moderate"))
        .json(&serde_json::json!({ "action": "mute", "name": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(no.status(), 401);

    // Mute freezes posting; unmute restores.
    assert_eq!(post("victim").await, 201);
    assert_eq!(mod_act("mute", "victim").await, 200);
    assert_eq!(post("victim").await, 403);
    assert_eq!(mod_act("unmute", "victim").await, 200);
    assert_eq!(post("victim").await, 201);

    // Ban blocks posting AND rejoining (WS handshake rejected).
    assert_eq!(mod_act("ban", "victim").await, 200);
    assert_eq!(post("victim").await, 403);
    let joined = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/ws?room=general&name=victim&type=agent"
    ))
    .await
    .is_ok();
    assert!(!joined, "banned name must not connect");

    // Unban lets them back.
    assert_eq!(mod_act("unban", "victim").await, 200);
    assert_eq!(post("victim").await, 201);
}

#[tokio::test]
async fn closing_a_room_evicts_and_wipes_it() {
    let (port, _guard) = spawn_server_with("adm").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // Create a room by posting, and put a note in it.
    client
        .post(format!("{base}/rooms/doomed/messages"))
        .json(&serde_json::json!({ "sender": "x", "sender_type": "user", "text": "hi" }))
        .send()
        .await
        .unwrap();
    client
        .post(format!("{base}/rooms/doomed/notes"))
        .json(&serde_json::json!({ "key": "k", "by": "x", "body": "v" }))
        .send()
        .await
        .unwrap();

    // A watcher inside the room should be told it closed.
    let mut ws = connect_ws(port, "doomed", "watcher", "user").await;

    // Without the admin token -> 401, room survives.
    let unauth = client
        .delete(format!("{base}/rooms/doomed"))
        .send()
        .await
        .unwrap();
    assert_eq!(unauth.status(), 401);

    // Deletion requires the room to be closed (archived) first.
    client
        .put(format!("{base}/rooms/doomed/settings"))
        .header("x-admin-token", "adm")
        .json(&serde_json::json!({ "archived": true }))
        .send()
        .await
        .unwrap();

    // With the token -> 204 and a room-closed control reaches the watcher.
    let del = client
        .delete(format!("{base}/rooms/doomed"))
        .header("x-admin-token", "adm")
        .send()
        .await
        .unwrap();
    assert_eq!(del.status(), 204);
    let closed = |v: &Value| v["t"] == "control" && v["cmd"] == "room-closed";
    wait_for(&mut ws, closed).await;

    // Its content is gone (the room list no longer has it; the note 404s).
    let rooms: Vec<Value> = client
        .get(format!("{base}/rooms"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        !rooms.iter().any(|r| r["room"] == "doomed"),
        "closed room must not be listed"
    );
    let note = client
        .get(format!("{base}/rooms/doomed/notes/k"))
        .send()
        .await
        .unwrap();
    assert_eq!(note.status(), 404);

    // Deleting a room that doesn't exist -> 404.
    let missing = client
        .delete(format!("{base}/rooms/never-existed"))
        .header("x-admin-token", "adm")
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
}

#[tokio::test]
async fn archive_is_reversible_and_gates_delete() {
    let (port, _guard) = spawn_server_with("adm").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let post = || {
        let (client, base) = (client.clone(), base.clone());
        async move {
            client
                .post(format!("{base}/rooms/attic/messages"))
                .json(&serde_json::json!({ "sender": "x", "sender_type": "user", "text": "hi" }))
                .send()
                .await
                .unwrap()
                .status()
                .as_u16()
        }
    };
    let set_archived = |on: bool| {
        let (client, base) = (client.clone(), base.clone());
        async move {
            client
                .put(format!("{base}/rooms/attic/settings"))
                .header("x-admin-token", "adm")
                .json(&serde_json::json!({ "archived": on }))
                .send()
                .await
                .unwrap()
                .status()
                .as_u16()
        }
    };

    assert_eq!(post().await, 201);

    // An ACTIVE room refuses deletion — close it first (409).
    let early = client
        .delete(format!("{base}/rooms/attic"))
        .header("x-admin-token", "adm")
        .send()
        .await
        .unwrap();
    assert_eq!(early.status(), 409);

    // Close it: read-only, but still listed and its history is intact.
    assert_eq!(set_archived(true).await, 200);
    assert_eq!(post().await, 403, "archived rooms are read-only");
    let rooms: Vec<Value> = client
        .get(format!("{base}/rooms"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let row = rooms
        .iter()
        .find(|r| r["room"] == "attic")
        .expect("still listed");
    assert_eq!(row["archived"], true);
    let msgs: Vec<Value> = client
        .get(format!("{base}/rooms/attic/messages"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(msgs.len(), 1, "history kept while archived");

    // Reopening restores posting.
    assert_eq!(set_archived(false).await, 200);
    assert_eq!(post().await, 201);

    // Close again, then delete for good.
    assert_eq!(set_archived(true).await, 200);
    let del = client
        .delete(format!("{base}/rooms/attic"))
        .header("x-admin-token", "adm")
        .send()
        .await
        .unwrap();
    assert_eq!(del.status(), 204);
}

/// Seal is the irreversible lifecycle boundary and belongs to the single
/// Building Master. A Smaster may manage/archive a Loca, but cannot seal it.
#[tokio::test]
async fn only_the_master_can_seal_an_archived_loca() {
    let (port, _guard) = spawn_server_with("MASTER").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    client
        .post(format!("{base}/rooms/attic/messages"))
        .json(&serde_json::json!({
            "sender": "operator",
            "sender_type": "user",
            "text": "keep this history"
        }))
        .send()
        .await
        .unwrap();

    let smaster: Value = client
        .post(format!("{base}/smasters"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "murat" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let smaster_token = smaster["token"].as_str().unwrap();

    let archived = client
        .put(format!("{base}/rooms/attic/settings"))
        .header("x-admin-token", smaster_token)
        .json(&serde_json::json!({ "archived": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(archived.status(), 200, "Smaster may archive a Loca");

    let refused = client
        .delete(format!("{base}/rooms/attic"))
        .header("x-admin-token", smaster_token)
        .send()
        .await
        .unwrap();
    assert_eq!(
        refused.status(),
        401,
        "Smaster must not cross the Seal boundary"
    );

    let sealed = client
        .delete(format!("{base}/rooms/attic"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap();
    assert_eq!(sealed.status(), 204, "Master may seal the archived Loca");
}

/// A loca seats seven. More than that is a salon — a different kind of place.
#[tokio::test]
async fn loca_seats_seven() {
    let (port, _guard) = spawn_server_env("master", &[]).await;
    let mut seated = Vec::new();
    for i in 1..=7 {
        seated.push(connect_ws(port, "mobile", &format!("agent{i}"), "agent").await);
    }
    let members: Value = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/rooms/mobile/members"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(members.as_array().unwrap().len(), 7, "seven fit");

    // The eighth is turned away with a word, not dropped in silence.
    let mut eighth = connect_ws(port, "mobile", "agent8", "agent").await;
    let frame = wait_for(&mut eighth, |v| v["t"] == "control").await;
    assert!(
        frame["cmd"].as_str().unwrap().contains("full"),
        "a full loca says so"
    );

    let members: Value = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/rooms/mobile/members"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        members.as_array().unwrap().len(),
        7,
        "the eighth took no seat"
    );
    drop(seated);
}

/// The four ways somebody stops being in a loca are four different things, and
/// the difference has to survive contact with the code.
///
///   mute    — stays, reads, cannot speak
///   kick    — connection closed, davet stands, may walk back in
///   ban     — the door is shut, reading included
///   release — the work is done; the seat goes back, the building does not
#[tokio::test]
async fn leaving_a_loca_has_four_distinct_meanings() {
    let (port, _guard) = spawn_server_env("MASTER", &[("ROOM_TOKEN", "building".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let seat = |name: &'static str| {
        let (c, b) = (client.clone(), base.clone());
        async move {
            c.post(format!("{b}/members"))
                .header("x-admin-token", "MASTER")
                .json(&serde_json::json!({ "name": name, "kind": "agent" }))
                .send()
                .await
                .unwrap();
            let r: Value = c
                .post(format!("{b}/rooms/oda/call"))
                .header("x-admin-token", "MASTER")
                .json(&serde_json::json!({ "name": name }))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            r["token"].as_str().unwrap().to_string()
        }
    };
    let act = |action: &'static str, name: &'static str| {
        let (c, b) = (client.clone(), base.clone());
        async move {
            c.post(format!("{b}/rooms/oda/moderate"))
                .header("x-admin-token", "MASTER")
                .json(&serde_json::json!({ "action": action, "name": name }))
                .send()
                .await
                .unwrap();
        }
    };
    let can_read = |token: String| {
        let (c, b) = (client.clone(), base.clone());
        async move {
            c.get(format!("{b}/rooms/oda/messages"))
                .header("x-room-token", token)
                .send()
                .await
                .unwrap()
                .status()
                == 200
        }
    };

    // kick (çıkar): the connection closes AND the davet stops — otherwise the
    // same token walks right back in. PRINCIPLES: "çıkar — bağlantı kapanır,
    // daveti durur." The membership stays (they can be called back in), but the
    // loca door is shut until a fresh davet.
    let kicked = seat("kicked").await;
    act("kick", "kicked").await;
    assert!(
        !can_read(kicked).await,
        "a kick stops the davet — the token no longer reads"
    );

    // ban: the door is shut, and a shut door does not let you read.
    let banned = seat("banned").await;
    act("ban", "banned").await;
    assert!(
        !can_read(banned).await,
        "a ban closes reading too, or it is not a ban"
    );

    // release: the work is done. Seat gone, building kept, callable again.
    let released = seat("released").await;
    act("release", "released").await;
    assert!(!can_read(released).await, "the seat is taken back");

    let residents: Value = client
        .get(format!("{base}/residents"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let them = residents
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "released")
        .expect("still a member of the building");
    assert!(
        them["locas"].as_array().unwrap().is_empty(),
        "in the building, in no room"
    );

    let again = client
        .post(format!("{base}/rooms/oda/call"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "released" }))
        .send()
        .await
        .unwrap();
    assert!(
        again.status().is_success(),
        "calling them back is one step, not a fresh setup"
    );
}

/// A lead is named out loud, by an operator, and everyone in the room learns of
/// it at that moment. It advises; it does not command — so naming one changes
/// what people know, not what anybody is allowed to do.
#[tokio::test]
async fn a_lead_is_named_in_the_open_and_only_by_an_operator() {
    let (port, _guard) = spawn_server_env("MASTER", &[]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let lead_now = || {
        let (c, b) = (client.clone(), base.clone());
        async move {
            let s: Value = c
                .get(format!("{b}/rooms/genel/settings"))
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
    // Naming a lead is an EXPLICIT action, not a parsed chat message
    // (PRINCIPLES: "konuşmak yan etki üretmez"). The operator posts to the
    // lead endpoint; the master token is the operator here.
    let set_lead = |lead: Option<&'static str>, admin: bool| {
        let (c, b) = (client.clone(), base.clone());
        async move {
            let mut req = c.post(format!("{b}/rooms/genel/lead"));
            if admin {
                req = req.header("x-admin-token", "MASTER");
            }
            req.json(&serde_json::json!({ "lead": lead }))
                .send()
                .await
                .unwrap()
        }
    };

    assert_eq!(lead_now().await, None, "a loca starts with no lead");

    set_lead(Some("debug"), true).await;
    assert_eq!(
        lead_now().await.as_deref(),
        Some("debug"),
        "the operator's action names one"
    );

    // A non-operator (no admin token, no operator session) cannot name a lead.
    let refused = set_lead(Some("someone-else"), false).await;
    assert_eq!(refused.status(), 401, "only an operator names a lead");
    assert_eq!(
        lead_now().await.as_deref(),
        Some("debug"),
        "the lead is unchanged"
    );

    // Naming another replaces the first — a loca has one lead.
    set_lead(Some("sb-feature"), true).await;
    assert_eq!(lead_now().await.as_deref(), Some("sb-feature"));

    let goal: Value = client
        .post(format!("{base}/rooms/genel/goals"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({
            "outcome": "release is verified",
            "completion": "manual",
            "task_ids": [],
            "by": "operator"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    // The invariant has two mutation doors: activating a Goal and clearing
    // its Lead. Closing only the first left ACTIVE + no Lead reachable.
    let clear_active = set_lead(None, true).await;
    assert_eq!(clear_active.status(), 409);
    assert_eq!(lead_now().await.as_deref(), Some("sb-feature"));

    let transfer = set_lead(Some("release-lead"), true).await;
    assert_eq!(
        transfer.status(),
        200,
        "an active Goal permits Lead transfer"
    );
    assert_eq!(lead_now().await.as_deref(), Some("release-lead"));

    // Typing "@lead x" in CHAT no longer mutates anything — talking is talking.
    client.post(format!("{base}/rooms/genel/messages")).header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "sender": "operator", "sender_type": "user", "text": "@lead debug" }))
        .send().await.unwrap();
    assert_eq!(
        lead_now().await.as_deref(),
        Some("release-lead"),
        "chat does not set the lead"
    );

    client
        .patch(format!("{base}/rooms/genel/goals/{}", goal["id"]))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({
            "status": "cancelled",
            "by": "operator"
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    // Once no Goal is active, the operator can end the title explicitly.
    let cleared = set_lead(None, true).await;
    assert_eq!(cleared.status(), 200);
    assert_eq!(
        lead_now().await,
        None,
        "the operator can take the title back"
    );
}

/// A banned name is shut out, and the roster reflects it immediately.
#[tokio::test]
async fn ban_shuts_the_door_and_updates_the_roster() {
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
    let davet = davet_for(&base, "MASTER", "oda", "kotu").await;

    // Reads with the davet, then gets banned.
    assert_eq!(
        rest_can(&base, "oda", &[("x-room-token", &davet)]).await,
        200,
        "before ban: in"
    );
    client
        .post(format!("{base}/rooms/oda/moderate"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "action": "ban", "name": "kotu" }))
        .send()
        .await
        .unwrap();
    // A ban closes reading too — a door that still lets you read is not shut.
    assert_eq!(
        rest_can(&base, "oda", &[("x-room-token", &davet)]).await,
        401,
        "after ban: shut, reads included"
    );
    assert!(
        !ws_can(port, "oda", &format!("token={davet}")).await,
        "after ban: WS shut"
    );
}

/// A deleted loca stays deleted — a watch=1 listener touching it must not
/// bring it back.
#[tokio::test]
async fn a_deleted_loca_does_not_come_back() {
    let (port, _g) = spawn_server_env("MASTER", &[("REQUIRE_INVITE", "1".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // Create, then archive, then delete a loca.
    client
        .post(format!("{base}/rooms/gecici/messages"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "sender": "m", "sender_type": "user", "text": "hi" }))
        .send()
        .await
        .unwrap();
    client
        .put(format!("{base}/rooms/gecici/settings"))
        .header("x-admin-token", "MASTER")
        .json(
            &serde_json::json!({ "rate_limit": 10, "rate_window_secs": 30, "live": false,
            "archived": true, "live_timeout_secs": 120, "operators": [] }),
        )
        .send()
        .await
        .unwrap();
    let del = client
        .delete(format!("{base}/rooms/gecici"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap();
    assert!(del.status().is_success(), "delete succeeds");

    // A watch=1 listener connects to the deleted loca. It must NOT resurrect it.
    let _ = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/ws?room=gecici&name=watcher&type=agent&admin=MASTER&watch=1"
    ))
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let rooms: Value = client
        .get(format!("{base}/rooms"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let alive = rooms
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["room"] == "gecici");
    assert!(
        !alive,
        "the deleted loca stayed deleted despite a watcher touching it"
    );

    // But the master can bring it back deliberately: issuing a davet lifts the
    // tombstone, and then the loca lives again on the next real touch (a
    // message, a join) — just like any loca is born on first use.
    davet_for(&base, "MASTER", "gecici", "someone").await;
    client
        .post(format!("{base}/rooms/gecici/messages"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "sender": "m", "sender_type": "user", "text": "reopened" }))
        .send()
        .await
        .unwrap();
    let rooms: Value = client
        .get(format!("{base}/rooms"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        rooms
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["room"] == "gecici"),
        "after a davet lifts the tombstone, a message reopens the loca"
    );
}

/// Ban must reach the roster on the wire, not just the door. The fix this
/// covers is the extra ServerFrame::Members broadcast in the Ban branch: a
/// watcher connected when someone is banned should receive a fresh roster with
/// that name gone. Without the broadcast the door still shuts (other tests
/// catch that) but the banned name lingers on every screen — the asymmetry the
/// operator hit. This asserts the live frame, so deleting the broadcast fails.
#[tokio::test]
async fn ban_broadcasts_the_new_roster_live() {
    let (port, _g) = spawn_server_env("MASTER", &[]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // Two members seated: a watcher (stays) and a victim (to be banned).
    let mut watcher = connect_ws(port, "oda", "watcher", "agent").await;
    let _victim = connect_ws(port, "oda", "victim", "agent").await;
    // Drain until both are present in a members frame.
    let seen_both = wait_for(&mut watcher, |v| {
        v["t"] == "members"
            && v["members"].as_array().is_some_and(|a| {
                a.iter().any(|m| m["name"] == "watcher") && a.iter().any(|m| m["name"] == "victim")
            })
    })
    .await;
    assert_eq!(seen_both["t"], "members");

    // Ban the victim.
    client
        .post(format!("{base}/rooms/oda/moderate"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "action": "ban", "name": "victim" }))
        .send()
        .await
        .unwrap();

    // The watcher must receive a members frame WITHOUT victim — live, on the wire.
    let after = wait_for(&mut watcher, |v| {
        v["t"] == "members"
            && v["members"]
                .as_array()
                .is_some_and(|a| a.iter().all(|m| m["name"] != "victim"))
    })
    .await;
    assert!(
        after["members"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["name"] != "victim"),
        "the ban's roster broadcast reached the watcher with victim removed"
    );
}

/// A loca seats seven — the seat IS the davet (Model A). The 8th davet is
/// refused AT MINT, before anyone connects: the cap is on active davets, not
/// on live sockets. No phantom davet can exist for a full loca.
#[tokio::test]
async fn a_full_loca_refuses_the_eighth_even_with_a_davet() {
    let (port, _g) = spawn_server_env("MASTER", &[("REQUIRE_INVITE", "1".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // Seven davets fill the seven seats — minting each succeeds.
    for i in 0..7 {
        let name = format!("a{i}");
        admit(&base, "MASTER", &name, "agent").await;
        let r = client
            .post(format!("{base}/rooms/dolu/invites"))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({ "name": name }))
            .send()
            .await
            .unwrap();
        assert!(r.status().is_success(), "seat {i} mints");
    }

    // The eighth davet is refused at mint — the loca is full.
    admit(&base, "MASTER", "a8", "agent").await;
    let eighth = client
        .post(format!("{base}/rooms/dolu/invites"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "a8" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        eighth.status(),
        409,
        "the eighth davet is refused — the seat is the davet"
    );

    // Call-in (the UI path) is refused the same way — no phantom davet.
    let call8 = client
        .post(format!("{base}/rooms/dolu/call"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "a8" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        call8.status(),
        409,
        "call-in to a full loca mints no phantom davet"
    );

    // Only seven davets exist.
    let invs: Vec<Value> = client
        .get(format!("{base}/rooms/dolu/invites"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(invs.len(), 7, "seven davets, no more");
}

/// Round-robin advances the turn as each speaker posts, wraps around the order,
/// and survives a restart. The happy path was covered; the wrap and the
/// out-of-order refusal were not.
#[tokio::test]
async fn round_robin_advances_wraps_and_refuses_out_of_turn() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("rr.db").to_string_lossy().to_string();
    let client = reqwest::Client::new();

    // Non-admin posts: the admin token bypasses mode gating, so round-robin can
    // only be exercised by ordinary members. No REQUIRE_INVITE here so a member
    // posts with just their name (the mode, not the door, is what we test).
    let post =
        |port: u16, sender: &'static str| {
            let c = client.clone();
            async move {
                c.post(format!("http://127.0.0.1:{port}/rooms/rr/messages"))
                .json(&serde_json::json!({ "sender": sender, "sender_type": "agent", "text": "x" }))
                .send().await.unwrap().status().as_u16()
            }
        };

    let (port, guard) = spawn_server_env("MASTER", &[("DB_PATH", db.clone())]).await;
    client
        .post(format!("http://127.0.0.1:{port}/rooms/rr/messages"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "sender": "m", "sender_type": "user", "text": "hi" }))
        .send()
        .await
        .unwrap();
    // Order: alice, bob. alice's turn first.
    client.put(format!("http://127.0.0.1:{port}/rooms/rr/mode")).header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "mode": { "mode": "roundrobin", "order": ["alice", "bob"], "turn": 0 } }))
        .send().await.unwrap();

    // bob out of turn -> refused; alice -> ok, turn moves to bob.
    assert_eq!(post(port, "bob").await, 403, "bob out of turn");
    assert_eq!(post(port, "alice").await, 201, "alice's turn");
    assert_eq!(
        post(port, "alice").await,
        403,
        "now it is bob's turn, alice refused"
    );
    assert_eq!(post(port, "bob").await, 201, "bob's turn");
    // Wrap: after bob, back to alice.
    assert_eq!(post(port, "alice").await, 201, "wraps back to alice");
    drop(guard);

    // Restart: the round-robin turn is restored, not reset.
    let (port2, _g2) = spawn_server_env("MASTER", &[("DB_PATH", db)]).await;
    // After the wrap it is bob's turn again.
    assert_eq!(
        post(port2, "alice").await,
        403,
        "after restart alice is still out of turn"
    );
    assert_eq!(
        post(port2, "bob").await,
        201,
        "the turn survived the restart"
    );
}

/// An archived loca is read-only, not sealed: you may connect and read its
/// history, but posting is refused. This is distinct from a deleted loca (which
/// refuses the connection entirely).
#[tokio::test]
async fn an_archived_loca_reads_but_refuses_posts() {
    let (port, _g) = spawn_server_env("MASTER", &[]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    client
        .post(format!("{base}/rooms/eski/messages"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "sender": "m", "sender_type": "user", "text": "kept" }))
        .send()
        .await
        .unwrap();
    client
        .put(format!("{base}/rooms/eski/settings"))
        .header("x-admin-token", "MASTER")
        .json(
            &serde_json::json!({ "rate_limit": 10, "rate_window_secs": 30, "live": false,
            "archived": true, "live_timeout_secs": 120, "operators": [] }),
        )
        .send()
        .await
        .unwrap();

    // Reading still works — the history is kept.
    assert_eq!(
        rest_can(&base, "eski", &[("x-admin-token", "MASTER")]).await,
        200,
        "an archived loca is still readable"
    );
    // A WS connection still opens (you can watch a closed room).
    assert!(
        ws_can(port, "eski", "admin=MASTER").await,
        "you can connect to an archived loca to read"
    );
    // But posting is refused.
    let post = client
        .post(format!("{base}/rooms/eski/messages"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "sender": "m", "sender_type": "user", "text": "no" }))
        .send()
        .await
        .unwrap();
    assert_eq!(post.status(), 403, "posting to an archived loca is refused");
}

/// The four endpoints the audit found untested: whoami, GET /smasters, get_mod,
/// DELETE /members. Each at least success + the auth boundary.
#[tokio::test]
async fn the_untested_endpoints_answer_correctly() {
    let (port, _g) = spawn_server_env("MASTER", &[("REQUIRE_INVITE", "1".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // whoami: a davet says which loca; the master's key says "building"; nothing says no.
    let d = davet_for(&base, "MASTER", "general", "someone").await;
    let who: Value = client
        .get(format!("{base}/whoami"))
        .header("x-room-token", &d)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(who["kind"], "davet");
    assert_eq!(who["loca"], "general");
    let master_who: Value = client
        .get(format!("{base}/whoami"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(master_who["kind"], "building");

    // GET /smasters: master only.
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
    assert!(sm["token"].as_str().is_some());
    let list: Value = client
        .get(format!("{base}/smasters"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        list.as_array()
            .unwrap()
            .iter()
            .any(|s| s["name"] == "murat"),
        "smaster is listed"
    );
    let listed_smaster = list
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "murat")
        .unwrap();
    assert!(listed_smaster["id"].as_str().unwrap().starts_with("smid_"));
    assert!(listed_smaster["token"].is_null(), "list must redact secret");
    let refused = client
        .get(format!("{base}/smasters"))
        .header("x-room-token", &d)
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 401, "GET /smasters is master-only");

    // get_mod (GET /moderate): the mute/ban state.
    client
        .post(format!("{base}/rooms/general/moderate"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "action": "mute", "name": "loud" }))
        .send()
        .await
        .unwrap();
    let mod_state: Value = client
        .get(format!("{base}/rooms/general/moderate"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        mod_state["muted"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n == "loud"),
        "get_mod shows the mute"
    );

    // DELETE /members: revoke a membership. Admit one first.
    client
        .post(format!("{base}/members"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "gecici", "kind": "agent" }))
        .send()
        .await
        .unwrap();
    let members: Value = client
        .get(format!("{base}/members"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let tok = members
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "gecici")
        .and_then(|m| m["id"].as_str())
        .expect("member has a management id")
        .to_string();
    let del = client
        .delete(format!("{base}/members/{tok}"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap();
    assert!(del.status().is_success(), "a membership can be revoked");
    // Master-only.
    let no = client
        .delete(format!("{base}/members/whatever"))
        .header("x-room-token", &d)
        .send()
        .await
        .unwrap();
    assert_eq!(no.status(), 401, "DELETE /members is master-only");
}

/// A ban survives a restart. Without persistence the ban set empties on boot
/// while davets reload, so a banned name walks right back in.
#[tokio::test]
async fn a_ban_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("bans.db").to_string_lossy().to_string();
    let client = reqwest::Client::new();
    // First boot: seat cyber via davet, then ban them.
    let d = {
        let (port, _g) = spawn_server_env(
            "MASTER",
            &[("DB_PATH", db.clone()), ("REQUIRE_INVITE", "1".into())],
        )
        .await;
        let base = format!("http://127.0.0.1:{port}");
        let d = davet_for(&base, "MASTER", "oda", "cyber").await;
        // Read works before the ban.
        assert_eq!(
            client
                .get(format!("{base}/rooms/oda/messages"))
                .header("x-room-token", &d)
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
        client
            .post(format!("{base}/rooms/oda/moderate"))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({ "action": "ban", "name": "cyber" }))
            .send()
            .await
            .unwrap();
        d
    };
    // Second boot on the same DB: the ban must still hold.
    let (port, _g) =
        spawn_server_env("MASTER", &[("DB_PATH", db), ("REQUIRE_INVITE", "1".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let after = client
        .get(format!("{base}/rooms/oda/messages"))
        .header("x-room-token", &d)
        .send()
        .await
        .unwrap();
    assert_eq!(after.status(), 401, "a ban is not forgotten by a restart");
    // And the mod state still lists them.
    let modst: Value = client
        .get(format!("{base}/rooms/oda/moderate"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(modst["banned"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n == "cyber"));
}

/// A sealed (deleted) loca does not come back on a post/journal/task — the
/// tombstone holds against the front door, not just subscribe/join.
#[tokio::test]
async fn a_sealed_loca_stays_sealed_against_posts() {
    let (port, _g) = spawn_server_env("MASTER", &[]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    // Create, archive, delete a loca.
    client
        .post(format!("{base}/rooms/temp/messages"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "sender": "master", "sender_type": "user", "text": "hi" }))
        .send()
        .await
        .unwrap();
    client
        .put(format!("{base}/rooms/temp/settings"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "archived": true }))
        .send()
        .await
        .unwrap();
    let del = client
        .delete(format!("{base}/rooms/temp"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap();
    assert!(del.status().is_success(), "an archived loca can be sealed");
    // A post must not resurrect it.
    let post = client
        .post(format!("{base}/rooms/temp/messages"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "sender": "master", "sender_type": "user", "text": "back?" }))
        .send()
        .await
        .unwrap();
    assert!(
        !post.status().is_success(),
        "a sealed loca does not reopen on a post"
    );
    // It is absent from the loca list.
    let rooms: Vec<Value> = client
        .get(format!("{base}/rooms"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        !rooms.iter().any(|r| r["room"] == "temp"),
        "sealed locas are gone from the floor plan"
    );
}

/// Release drops the roster entry directly (like kick/ban) so a dead socket
/// leaves no ghost — "işi bitti" means the seat is truly empty.
#[tokio::test]
async fn release_leaves_no_ghost() {
    let (port, _g) = spawn_server_env("MASTER", &[]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    admit(&base, "MASTER", "worker", "agent").await;
    client
        .post(format!("{base}/rooms/oda/call"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "worker" }))
        .send()
        .await
        .unwrap();
    let mut ws = connect_ws(port, "oda", "worker", "agent").await;
    let _ = wait_for(&mut ws, |v| v["t"] == "history").await;
    // Release them.
    client
        .post(format!("{base}/rooms/oda/moderate"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "action": "release", "name": "worker" }))
        .send()
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let members: Vec<Value> = client
        .get(format!("{base}/rooms/oda/members"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        !members.iter().any(|m| m["name"] == "worker"),
        "release leaves no ghost in the roster"
    );
}

/// The call is not merely a new row in the database: an agent keeps a
/// membership-only lobby socket after release, receives the fresh davet there,
/// and can immediately open the called loca. A reconnect also replays a call
/// that happened while the socket was down.
#[tokio::test]
async fn release_to_lobby_and_one_click_recall_is_end_to_end() {
    let (port, _g) = spawn_server_env("MASTER", &[("REQUIRE_INVITE", "1".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let first_davet = davet_for(&base, "MASTER", "oda", "worker").await;

    let claim: Value = client
        .post(format!("{base}/membership/claim"))
        .header("x-room-token", &first_davet)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let membership = claim["membership_token"].as_str().unwrap().to_string();
    assert!(membership.starts_with("mb_"));

    // The permanent membership is a lobby credential, not a skeleton key.
    let private_door = client
        .get(format!("{base}/rooms/oda/messages"))
        .header("x-room-token", &membership)
        .send()
        .await
        .unwrap();
    assert_eq!(
        private_door.status(),
        401,
        "membership alone must never open a private loca"
    );

    let lobby_url = format!("ws://127.0.0.1:{port}/lobby/ws?membership={membership}");
    let (mut lobby, _) = tokio_tungstenite::connect_async(&lobby_url).await.unwrap();
    let ready = wait_for(&mut lobby, |v| v["t"] == "lobby_ready").await;
    assert_eq!(ready["invites"][0]["room"], "oda");
    assert_eq!(ready["invites"][0]["token"], first_davet);
    let initial = wait_for(&mut lobby, |v| v["t"] == "called").await;
    assert_eq!(initial["token"], first_davet);

    client
        .post(format!("{base}/rooms/oda/release"))
        .header("x-room-token", &first_davet)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    let residents: Vec<Value> = client
        .get(format!("{base}/residents"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let worker = residents.iter().find(|r| r["name"] == "worker").unwrap();
    assert!(worker["locas"].as_array().unwrap().is_empty());
    assert_eq!(
        worker["online"], true,
        "the lobby socket keeps the released agent present"
    );

    client
        .post(format!("{base}/rooms/oda/call"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "worker" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let called = wait_for(&mut lobby, |v| v["t"] == "called").await;
    assert_eq!(called["room"], "oda");
    let second_davet = called["token"].as_str().unwrap().to_string();
    assert_ne!(second_davet, first_davet);
    assert_eq!(
        client
            .get(format!("{base}/rooms/oda/messages"))
            .header("x-room-token", &second_davet)
            .send()
            .await
            .unwrap()
            .status(),
        200,
        "the privately delivered call opens exactly its loca"
    );

    // Calls made during a brief lobby disconnect are not lost: the invite is
    // persisted and replayed on the next lobby handshake.
    lobby.send(WsMessage::Close(None)).await.unwrap();
    client
        .post(format!("{base}/rooms/oda/release"))
        .header("x-room-token", &second_davet)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    client
        .post(format!("{base}/rooms/oda/call"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "worker" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    let (mut reconnected, _) = tokio_tungstenite::connect_async(&lobby_url).await.unwrap();
    wait_for(&mut reconnected, |v| v["t"] == "lobby_ready").await;
    let replayed = wait_for(&mut reconnected, |v| v["t"] == "called").await;
    assert_eq!(replayed["room"], "oda");
    assert_ne!(replayed["token"], second_davet);
}

/// `iye` is not a cosmetic alias. The migration moves the complete durable
/// record atomically, marks the loca special in the API, and removes ordinary
/// legacy seats. Rank (master/smaster) still enters; only the two caretakers
/// may receive a davet.
#[tokio::test]
async fn iye_migration_preserves_history_and_reserves_the_loca() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("iye.db").to_string_lossy().to_string();
    let client = reqwest::Client::new();

    {
        let (port, _g) = spawn_server_env("MASTER", &[("DB_PATH", db.clone())]).await;
        let base = format!("http://127.0.0.1:{port}");
        client
            .post(format!("{base}/rooms/general/messages"))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({
                "sender": "master",
                "sender_type": "user",
                "text": "the building remembers"
            }))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        davet_for(&base, "MASTER", "general", "loca-dev").await;
        davet_for(&base, "MASTER", "general", "cyber").await;
    }

    let (port, _g) = spawn_server_env(
        "MASTER",
        &[
            ("DB_PATH", db),
            ("ROOM_RENAME", "general:iye".into()),
            ("LOCA_AGENT_ROOM", "iye".into()),
            ("RESERVED_LOCA", "iye".into()),
            ("LOCA_CARETAKERS", "loca-dev,loca-care".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");

    let rooms: Vec<Value> = client
        .get(format!("{base}/rooms"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!rooms.iter().any(|r| r["room"] == "general"));
    let iye = rooms.iter().find(|r| r["room"] == "iye").unwrap();
    assert_eq!(iye["special"], true);

    let history: Vec<Value> = client
        .get(format!("{base}/rooms/iye/messages"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(history[0]["text"], "the building remembers");

    let invites: Vec<Value> = client
        .get(format!("{base}/rooms/iye/invites"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(invites.iter().any(|i| i["name"] == "loca-dev"));
    assert!(
        !invites.iter().any(|i| i["name"] == "cyber"),
        "ordinary legacy seats are not carried into iye"
    );

    let denied = client
        .post(format!("{base}/rooms/iye/call"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "cyber" }))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 403);

    let smaster: Value = client
        .post(format!("{base}/smasters"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "second" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        client
            .get(format!("{base}/rooms/iye/messages"))
            .header("x-admin-token", smaster["token"].as_str().unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        200,
        "smaster enters iye by rank, not by ordinary invitation"
    );
}

/// A mute also survives a restart (same door-state persistence as ban).
#[tokio::test]
async fn a_mute_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("mute.db").to_string_lossy().to_string();
    let client = reqwest::Client::new();
    {
        let (port, _g) = spawn_server_env("MASTER", &[("DB_PATH", db.clone())]).await;
        let base = format!("http://127.0.0.1:{port}");
        client
            .post(format!("{base}/rooms/oda/moderate"))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({ "action": "mute", "name": "cyber" }))
            .send()
            .await
            .unwrap();
    }
    let (port, _g) = spawn_server_env("MASTER", &[("DB_PATH", db)]).await;
    let base = format!("http://127.0.0.1:{port}");
    let modst: Value = client
        .get(format!("{base}/rooms/oda/moderate"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        modst["muted"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n == "cyber"),
        "a mute is not forgotten by a restart"
    );
}

/// Unban after a restart clears the persisted row — the door opens again.
#[tokio::test]
async fn unban_after_restart_clears_the_row() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("unban.db").to_string_lossy().to_string();
    let client = reqwest::Client::new();
    {
        let (port, _g) = spawn_server_env("MASTER", &[("DB_PATH", db.clone())]).await;
        let base = format!("http://127.0.0.1:{port}");
        client
            .post(format!("{base}/rooms/oda/moderate"))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({ "action": "ban", "name": "cyber" }))
            .send()
            .await
            .unwrap();
    }
    let (port, _g) = spawn_server_env("MASTER", &[("DB_PATH", db)]).await;
    let base = format!("http://127.0.0.1:{port}");
    // Still banned after restart, then lift it.
    client
        .post(format!("{base}/rooms/oda/moderate"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "action": "unban", "name": "cyber" }))
        .send()
        .await
        .unwrap();
    let modst: Value = client
        .get(format!("{base}/rooms/oda/moderate"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        !modst["banned"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n == "cyber"),
        "unban removes the persisted ban"
    );
}

/// A sealed loca is not resurrected by a watch=1 connection either — the
/// tombstone holds against every door, not just posts.
#[tokio::test]
async fn a_sealed_loca_is_not_revived_by_a_watcher() {
    let (port, _g) = spawn_server_env("MASTER", &[]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    client
        .post(format!("{base}/rooms/temp/messages"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "sender": "master", "sender_type": "user", "text": "hi" }))
        .send()
        .await
        .unwrap();
    client
        .put(format!("{base}/rooms/temp/settings"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "archived": true }))
        .send()
        .await
        .unwrap();
    client
        .delete(format!("{base}/rooms/temp"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap();
    // A watcher connects — must not bring the room back.
    let _ = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/ws?room=temp&name=nosy&type=agent&watch=1&admin=MASTER"
    ))
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let rooms: Vec<Value> = client
        .get(format!("{base}/rooms"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        !rooms.iter().any(|r| r["room"] == "temp"),
        "a watcher does not revive a sealed loca"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// FAZ 3 — lifecycle + roller: arşiv read-only, operatör yetkisi, seal, @lead.
// ─────────────────────────────────────────────────────────────────────────────

/// An archived loca is read-only for EVERYONE — no note, task or journal write
/// (PRINCIPLES: archived = read-only, hiçbir mutation).
#[tokio::test]
async fn archived_blocks_all_mutations() {
    let (port, _g) = spawn_server_env("MASTER", &[]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    client
        .post(format!("{base}/rooms/oda/messages"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "sender": "m", "sender_type": "user", "text": "hi" }))
        .send()
        .await
        .unwrap();
    let attention: Value = client
        .post(format!("{base}/rooms/oda/attentions"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({
            "subject": "review", "audience": {"kind": "person", "name": "m"},
            "by": "master"
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    client
        .post(format!("{base}/rooms/oda/waits"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({
            "by": "m", "waiting_for": "reviewer", "reason": "review"
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    client
        .put(format!("{base}/rooms/oda/settings"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "archived": true }))
        .send()
        .await
        .unwrap();
    // Every write path refuses.
    let note = client
        .post(format!("{base}/rooms/oda/notes"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "key": "k", "title": "t", "body": "b", "by": "master" }))
        .send()
        .await
        .unwrap();
    assert_eq!(note.status(), 409, "no note in an archived loca");
    let task = client
        .post(format!("{base}/rooms/oda/tasks"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "title": "do", "by": "master" }))
        .send()
        .await
        .unwrap();
    assert_eq!(task.status(), 409, "no task in an archived loca");
    let jrnl = client
        .post(format!("{base}/rooms/oda/journal"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "text": "did", "by": "master" }))
        .send()
        .await
        .unwrap();
    assert_eq!(jrnl.status(), 409, "no journal in an archived loca");
    let msg = client
        .post(format!("{base}/rooms/oda/messages"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "sender": "m", "sender_type": "user", "text": "more" }))
        .send()
        .await
        .unwrap();
    assert!(!msg.status().is_success(), "no message in an archived loca");
    for action in ["claim", "resolve"] {
        let response = client
            .post(format!(
                "{base}/rooms/oda/attentions/{}/{action}",
                attention["id"].as_str().unwrap()
            ))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({ "by": "master" }))
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            409,
            "no attention mutation while archived"
        );
    }
    let clear_wait = client
        .delete(format!("{base}/rooms/oda/waits/m"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "by": "master" }))
        .send()
        .await
        .unwrap();
    assert_eq!(clear_wait.status(), 409, "no wait mutation while archived");
    let attentions: Vec<Value> = client
        .get(format!("{base}/rooms/oda/attentions"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        attentions[0]["status"], "open",
        "archive pauses work; it does not complete it"
    );
    // Un-archiving is still allowed (it is the way back).
    let un = client
        .put(format!("{base}/rooms/oda/settings"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "archived": false }))
        .send()
        .await
        .unwrap();
    assert!(
        un.status().is_success(),
        "settings stays writable to reopen"
    );
}

/// A sealed loca survives a restart still sealed: its history stays on disk but
/// it does not reopen (PRINCIPLES: seal not destroy + restart does not undo it).
#[tokio::test]
async fn a_sealed_loca_survives_restart_sealed() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("seal.db").to_string_lossy().to_string();
    let client = reqwest::Client::new();
    {
        let (port, _g) = spawn_server_env("MASTER", &[("DB_PATH", db.clone())]).await;
        let base = format!("http://127.0.0.1:{port}");
        client
            .post(format!("{base}/rooms/temp/messages"))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({ "sender": "m", "sender_type": "user", "text": "a record" }))
            .send()
            .await
            .unwrap();
        client
            .put(format!("{base}/rooms/temp/settings"))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({ "archived": true }))
            .send()
            .await
            .unwrap();
        client
            .delete(format!("{base}/rooms/temp"))
            .header("x-admin-token", "MASTER")
            .send()
            .await
            .unwrap();
    }
    // Restart on the same DB: the sealed loca must not reopen.
    let (port, _g) = spawn_server_env("MASTER", &[("DB_PATH", db)]).await;
    let base = format!("http://127.0.0.1:{port}");
    let rooms: Vec<Value> = client
        .get(format!("{base}/rooms"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        !rooms.iter().any(|r| r["room"] == "temp"),
        "a sealed loca stays sealed across restart"
    );
    // A post does not resurrect it either.
    let post = client
        .post(format!("{base}/rooms/temp/messages"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "sender": "m", "sender_type": "user", "text": "back?" }))
        .send()
        .await
        .unwrap();
    assert!(!post.status().is_success(), "sealed stays sealed");
}

/// Naming a lead is an explicit endpoint action, and it announces itself — a
/// chat "@lead x" no longer mutates anything (PRINCIPLES: konuşmak yan etki
/// üretmez). Covered by a_lead_is_named_in_the_open_and_only_by_an_operator;
/// here we assert the announcement is emitted and directly wakes the named
/// mentions-only agent without waking everybody else.
#[tokio::test]
async fn lead_endpoint_announces() {
    let (port, _g) = spawn_server_env("MASTER", &[]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let mut ws = connect_ws(port, "oda", "watcher", "user").await;
    let _ = wait_for(&mut ws, |v| v["t"] == "history").await;
    let lead_url = format!(
        "ws://127.0.0.1:{port}/ws?room=oda&name=debug&type=agent&filter=mentions\
         &turn_max=3&turn_wait_ms=4000"
    );
    let other_url = format!(
        "ws://127.0.0.1:{port}/ws?room=oda&name=other&type=agent&filter=mentions\
         &turn_max=3&turn_wait_ms=4000"
    );
    let (mut lead, _) = tokio_tungstenite::connect_async(lead_url).await.unwrap();
    let (mut other, _) = tokio_tungstenite::connect_async(other_url).await.unwrap();

    client
        .post(format!("{base}/rooms/oda/lead"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "lead": "debug" }))
        .send()
        .await
        .unwrap();
    // The whole room learns the result through the public announcement.
    let ann = wait_for(&mut ws, |v| {
        v["t"] == "msg" && v["message"]["kind"] == "announce"
    })
    .await;
    assert!(
        ann["message"]["text"].as_str().unwrap().contains("debug"),
        "the room learns who leads"
    );
    assert_eq!(ann["message"]["target"], "debug");

    // The named lead receives that same public record as an immediate direct
    // runtime wake even though announcements normally bypass mention nudges.
    let wake = wait_for(&mut lead, |v| {
        v["t"] == "msg" && v["message"]["kind"] == "announce" && v["message"]["target"] == "debug"
    })
    .await;
    assert!(wake["message"]["text"]
        .as_str()
        .unwrap()
        .contains("set @lead debug"));

    // Naming one lead must not burn a turn for every other filtered agent.
    assert!(
        tokio::time::timeout(Duration::from_millis(150), other.next())
            .await
            .is_err(),
        "an unrelated agent was woken by the lead assignment"
    );
}

/// Reconnect recovery reads the durable archive rather than the Hub's
/// 200-message hot tail. A deploy may happen while an agent is offline, so the
/// regression crosses a real process restart and pages more than 1,000 rows.
#[tokio::test]
async fn durable_backfill_pages_the_complete_archive_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("backfill.db").to_string_lossy().to_string();
    let client = reqwest::Client::new();
    let total = 1_005u64;

    {
        let (port, _guard) = spawn_server_env("MASTER", &[("DB_PATH", db.clone())]).await;
        let base = format!("http://127.0.0.1:{port}");
        for sequence in 1..=total {
            let response = client
                .post(format!("{base}/rooms/archive/messages"))
                .header("x-admin-token", "MASTER")
                .json(&serde_json::json!({
                    "sender": "operator",
                    "sender_type": "user",
                    "text": format!("durable-{sequence}")
                }))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 201);
        }

        let hot_tail: Vec<Value> = client
            .get(format!("{base}/rooms/archive/messages"))
            .header("x-admin-token", "MASTER")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(hot_tail.len(), 200, "normal room-open stays bounded");
        assert_eq!(hot_tail[0]["text"], "durable-806");
    }

    let (port, _guard) = spawn_server_env("MASTER", &[("DB_PATH", db)]).await;
    let base = format!("http://127.0.0.1:{port}");
    let mut cursor = 0u64;
    let mut recovered = Vec::new();
    loop {
        let page: Vec<Value> = client
            .get(format!(
                "{base}/rooms/archive/messages?since={cursor}&limit=137"
            ))
            .header("x-admin-token", "MASTER")
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        if page.is_empty() {
            break;
        }
        cursor = page.last().unwrap()["id"].as_u64().unwrap();
        recovered.extend(page);
    }

    assert_eq!(recovered.len(), total as usize);
    assert_eq!(recovered.first().unwrap()["text"], "durable-1");
    assert_eq!(recovered.last().unwrap()["text"], "durable-1005");
    assert!(recovered
        .windows(2)
        .all(|pair| pair[0]["id"].as_u64() < pair[1]["id"].as_u64()));
}
