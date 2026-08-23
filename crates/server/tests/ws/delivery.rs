//! WebSocket delivery, filtering, packets, replay, and idempotency.

use super::*;

#[tokio::test]
async fn broadcast_reaches_both_clients_and_roster_updates() {
    let (port, _guard) = spawn_server().await;

    let mut agent = connect_ws(port, "general", "backend", "agent").await;
    let mut user = connect_ws(port, "general", "operator", "user").await;

    // Both should observe a members frame that includes both names.
    let both_here = |v: &Value| {
        v["t"] == "members"
            && v["members"].as_array().is_some_and(|a| {
                let names: Vec<&str> = a.iter().filter_map(|m| m["name"].as_str()).collect();
                names.contains(&"backend") && names.contains(&"operator")
            })
    };
    wait_for(&mut user, both_here).await;

    // POST a message over REST as the operator addressing @all.
    let client = reqwest::Client::new();
    client
        .post(format!("http://127.0.0.1:{port}/rooms/general/messages"))
        .json(&serde_json::json!({
            "sender": "operator", "sender_type": "user", "target": "all", "text": "ping"
        }))
        .send()
        .await
        .unwrap();

    // The agent (a different client) must receive it live.
    let got = |v: &Value| v["t"] == "msg" && v["message"]["text"] == "ping";
    let frame = wait_for(&mut agent, got).await;
    assert_eq!(frame["message"]["target"], "all");
    assert_eq!(frame["message"]["sender_type"], "user");

    // A user's WS `send` frame should also broadcast.
    user.send(WsMessage::Text(
        serde_json::json!({ "t": "send", "text": "hi from ws" }).to_string(),
    ))
    .await
    .unwrap();
    let got2 = |v: &Value| v["t"] == "msg" && v["message"]["text"] == "hi from ws";
    wait_for(&mut agent, got2).await;

    // REST backlog reflects both messages.
    let msgs: Vec<Value> = client
        .get(format!("http://127.0.0.1:{port}/rooms/general/messages"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(msgs.len(), 2);

    // Sidebar polling gets a cheap cursor first; it only fetches the room tail
    // when this changes, then counts messages since its local read cursor.
    let rooms: Vec<Value> = client
        .get(format!("http://127.0.0.1:{port}/rooms"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let general = rooms.iter().find(|r| r["room"] == "general").unwrap();
    assert_eq!(general["last_id"], msgs.last().unwrap()["id"]);
}

#[tokio::test]
async fn reply_to_and_typing_flow() {
    let (port, _guard) = spawn_server().await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let mut watcher = connect_ws(port, "general", "watcher", "user").await;

    // Post a root message, then a reply pointing at it.
    let root: Value = client
        .post(format!("{base}/rooms/general/messages"))
        .json(&serde_json::json!({ "sender": "a", "sender_type": "agent", "text": "root" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let root_id = root["id"].as_u64().unwrap();
    client.post(format!("{base}/rooms/general/messages"))
        .json(&serde_json::json!({ "sender": "b", "sender_type": "agent", "text": "re", "reply_to": root_id }))
        .send().await.unwrap();

    // The reply carries reply_to over the wire.
    let is_reply = |v: &Value| v["t"] == "msg" && v["message"]["text"] == "re";
    let frame = wait_for(&mut watcher, is_reply).await;
    assert_eq!(frame["message"]["reply_to"], root_id);

    // A typing frame from one client reaches another.
    use futures_util::SinkExt;
    let mut agent = connect_ws(port, "general", "typer", "agent").await;
    agent
        .send(WsMessage::Text(
            serde_json::json!({ "t": "typing", "on": true }).to_string(),
        ))
        .await
        .unwrap();
    let is_typing = |v: &Value| v["t"] == "typing" && v["name"] == "typer" && v["on"] == true;
    wait_for(&mut watcher, is_typing).await;
}

#[tokio::test]
async fn filter_msg_suppresses_noise_but_delivers_messages() {
    let (port, _guard) = spawn_server().await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // Connect an events-only listener (?filter=msg).
    let url = format!("ws://127.0.0.1:{port}/ws?room=general&name=filtered&type=agent&filter=msg");
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();

    // Cause noise (a typing frame from someone else) then a real message.
    use futures_util::SinkExt;
    let mut noisy = connect_ws(port, "general", "noisy", "user").await;
    noisy
        .send(WsMessage::Text(
            serde_json::json!({ "t": "typing", "on": true }).to_string(),
        ))
        .await
        .unwrap();
    client
        .post(format!("{base}/rooms/general/messages"))
        .json(&serde_json::json!({ "sender": "x", "sender_type": "agent", "text": "real one" }))
        .send()
        .await
        .unwrap();

    // The filtered client's FIRST frame must be the message, not typing/history/members.
    let first = {
        let deadline = tokio::time::sleep(Duration::from_secs(3));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = &mut deadline => panic!("no frame"),
                item = ws.next() => {
                    if let Some(Ok(WsMessage::Text(t))) = item {
                        break serde_json::from_str::<Value>(&t).unwrap();
                    }
                }
            }
        }
    };
    assert_eq!(
        first["t"], "msg",
        "events-only client should only get msg frames"
    );
    assert_eq!(first["message"]["text"], "real one");
}

#[tokio::test]
async fn filter_mentions_only_delivers_addressed_messages() {
    let (port, _guard) = spawn_server().await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // A client that only wants messages addressing "bob".
    let url = format!(
        "ws://127.0.0.1:{port}/ws?room=general&name=bob&type=agent\
         &filter=mentions&turn_max=1"
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();

    let post = |target: Option<&'static str>, text: &'static str| {
        let (client, base) = (client.clone(), base.clone());
        async move {
            let mut body =
                serde_json::json!({ "sender": "x", "sender_type": "agent", "text": text });
            if let Some(t) = target {
                body["target"] = serde_json::json!(t);
            }
            client
                .post(format!("{base}/rooms/general/messages"))
                .json(&body)
                .send()
                .await
                .unwrap();
        }
    };

    // Not for bob (no target, no @bob) -> must NOT arrive.
    post(None, "just chatter").await;
    // Directly to alice -> must NOT arrive.
    post(Some("alice"), "hey alice").await;
    // @all -> must arrive.
    post(Some("all"), "everyone listen").await;

    // First frame bob receives should be the @all one (the earlier two dropped).
    let first = {
        let deadline = tokio::time::sleep(Duration::from_secs(3));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = &mut deadline => panic!("no addressed frame"),
                item = ws.next() => {
                    if let Some(Ok(WsMessage::Text(t))) = item {
                        break serde_json::from_str::<Value>(&t).unwrap();
                    }
                }
            }
        }
    };
    assert_eq!(first["message"]["text"], "everyone listen");

    // An @bob mention in text (no target) should also arrive.
    post(None, "ping @bob you there").await;
    let is_bob = |v: &Value| v["t"] == "msg" && v["message"]["text"] == "ping @bob you there";
    wait_for(&mut ws, is_bob).await;
}

#[tokio::test]
async fn a_named_lead_hears_the_whole_room_until_the_title_ends() {
    let (port, _guard) = spawn_server_with("adm").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let mut observer = connect_ws(port, "general", "operator", "user").await;

    // Lead candidates still use the token-saving mentions stream. Naming the
    // lead, rather than changing the URL or enabling room-wide live mode,
    // widens this one participant's delivery.
    let url = format!(
        "ws://127.0.0.1:{port}/ws?room=general&name=bob&type=agent\
         &filter=mentions&turn_max=1&admin=adm"
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    // The WS upgrade can complete before its room task has subscribed. Use the
    // ordinary roster as a barrier so the assignment broadcast cannot race
    // the lead's join.
    wait_for(&mut observer, |v| {
        v["t"] == "members"
            && v["members"]
                .as_array()
                .is_some_and(|rows| rows.iter().any(|m| m["name"] == "bob"))
    })
    .await;

    let named = client
        .post(format!("{base}/rooms/general/lead"))
        .header("x-admin-token", "adm")
        .json(&serde_json::json!({ "lead": "bob" }))
        .send()
        .await
        .unwrap();
    assert!(named.status().is_success());
    // The assignment itself is an immediate direct wake.
    wait_for(&mut ws, |v| {
        v["t"] == "msg" && v["message"]["target"] == "bob"
    })
    .await;

    client
        .post(format!("{base}/rooms/general/messages"))
        .json(&serde_json::json!({
            "sender": "alice",
            "sender_type": "agent",
            "text": "plain room conversation"
        }))
        .send()
        .await
        .unwrap();
    wait_for(&mut ws, |v| {
        v["t"] == "msg" && v["message"]["text"] == "plain room conversation"
    })
    .await;

    let ended = client
        .post(format!("{base}/rooms/general/lead"))
        .header("x-admin-token", "adm")
        .json(&serde_json::json!({ "lead": null }))
        .send()
        .await
        .unwrap();
    assert!(ended.status().is_success());
    // The HTTP response is the state barrier. The public end announcement is
    // intentionally not a runtime call, so a mentions-only former lead does
    // not spend a model turn on it.

    client
        .post(format!("{base}/rooms/general/messages"))
        .json(&serde_json::json!({
            "sender": "alice",
            "sender_type": "agent",
            "text": "ordinary chatter after lead"
        }))
        .send()
        .await
        .unwrap();
    client
        .post(format!("{base}/rooms/general/messages"))
        .json(&serde_json::json!({
            "sender": "alice",
            "sender_type": "agent",
            "target": "bob",
            "text": "direct barrier"
        }))
        .send()
        .await
        .unwrap();
    let direct = wait_for(&mut ws, |v| {
        v["t"] == "msg" && v["message"]["text"] == "direct barrier"
    })
    .await;
    assert_ne!(direct["message"]["text"], "ordinary chatter after lead");
}

#[tokio::test]
async fn mention_turn_queue_flushes_on_max_or_quiet_window() {
    let (port, _guard) = spawn_server().await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let url = format!(
        "ws://127.0.0.1:{port}/ws?room=general&name=bob&type=agent\
         &filter=mentions&turn_max=3&turn_idle_ms=250&turn_max_wait_ms=700"
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();

    let post = |text: &'static str| {
        let (client, base) = (client.clone(), base.clone());
        async move {
            client
                .post(format!("{base}/rooms/general/messages"))
                .json(&serde_json::json!({
                    "sender": "operator",
                    "sender_type": "user",
                    "target": "bob",
                    "text": text,
                }))
                .send()
                .await
                .unwrap();
        }
    };

    post("one").await;
    post("two").await;
    // Two quick Enters stay queued; they must not cause two model turns.
    assert!(
        tokio::time::timeout(Duration::from_millis(100), ws.next())
            .await
            .is_err(),
        "turn queue flushed before max/deadline"
    );

    post("three").await;
    let batch = wait_for(&mut ws, |v| v["t"] == "turn").await;
    let texts: Vec<&str> = batch["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["text"].as_str())
        .collect();
    assert_eq!(texts, ["one", "two", "three"]);

    // A single complete thought moves after the quiet window and retains the
    // backwards-compatible msg shape.
    post("alone").await;
    let single = wait_for(&mut ws, |v| {
        v["t"] == "msg" && v["message"]["text"] == "alone"
    })
    .await;
    assert_eq!(single["message"]["sender"], "operator");
}

#[tokio::test]
async fn turn_quiet_window_slides_but_hard_deadline_does_not() {
    let (port, _guard) = spawn_server().await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let url = format!(
        "ws://127.0.0.1:{port}/ws?room=general&name=bob&type=agent\
         &filter=mentions&turn_max=4&turn_idle_ms=220&turn_max_wait_ms=500"
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let post = |text: &'static str| {
        let (client, base) = (client.clone(), base.clone());
        async move {
            client
                .post(format!("{base}/rooms/general/messages"))
                .json(&serde_json::json!({
                    "sender": "operator", "sender_type": "user",
                    "target": "bob", "text": text
                }))
                .send()
                .await
                .unwrap();
        }
    };

    post("fragment one").await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    post("fragment two").await;
    // 250ms from the first, but only 100ms from the latest: a fixed-first
    // timer would flush here and create the extra model call this feature
    // exists to prevent.
    assert!(
        tokio::time::timeout(Duration::from_millis(100), ws.next())
            .await
            .is_err(),
        "latest fragment did not restart the quiet window"
    );
    let quiet_batch = wait_for(&mut ws, |value| value["t"] == "turn").await;
    assert_eq!(quiet_batch["messages"].as_array().unwrap().len(), 2);

    post("one").await;
    for text in ["two", "three"] {
        tokio::time::sleep(Duration::from_millis(180)).await;
        post(text).await;
    }
    let hard_batch = wait_for(&mut ws, |value| value["t"] == "turn").await;
    assert_eq!(
        hard_batch["messages"].as_array().unwrap().len(),
        3,
        "hard first-message deadline must flush continuous typing"
    );
}

#[tokio::test]
async fn turn_packet_defaults_come_from_loca_settings() {
    let (port, _guard) = spawn_server().await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let saved: Value = client
        .put(format!("{base}/rooms/general/settings"))
        .json(&serde_json::json!({
                "turn_max_messages": 2,
                "turn_idle_ms": 1000,
                "turn_max_wait_ms": 2000
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(saved["turn_max_messages"], 2);
    let mut observer = connect_ws(port, "general", "operator", "user").await;
    let _ = wait_for(&mut observer, |value| value["t"] == "history").await;
    let url = format!("ws://127.0.0.1:{port}/ws?room=general&name=bob&type=agent&filter=mentions");
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    // Mentions streams intentionally receive no history/members frames. Use
    // an ordinary room observer as the subscription barrier instead.
    let _ = wait_for(&mut observer, |value| {
        value["t"] == "members"
            && value["members"]
                .as_array()
                .is_some_and(|rows| rows.iter().any(|member| member["name"] == "bob"))
    })
    .await;
    for text in ["one", "two"] {
        client
            .post(format!("{base}/rooms/general/messages"))
            .json(&serde_json::json!({
                "sender": "operator", "sender_type": "user",
                "target": "bob", "text": text
            }))
            .send()
            .await
            .unwrap();
    }
    let turn = wait_for(&mut ws, |value| value["t"] == "turn").await;
    assert_eq!(turn["messages"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn live_mode_overrides_mention_filter() {
    let (port, _guard) = spawn_server_with("adm").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // bob connects mentions-only.
    let url = format!(
        "ws://127.0.0.1:{port}/ws?room=general&name=bob&type=agent\
         &filter=mentions&turn_max=1"
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();

    // Operator flips the room to live mode.
    let s = client
        .put(format!("{base}/rooms/general/settings"))
        .header("x-admin-token", "adm")
        .json(&serde_json::json!({ "live": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(s.status(), 200);

    // A plain wall post (NOT addressing bob) should now reach bob anyway.
    client.post(format!("{base}/rooms/general/messages"))
        .json(&serde_json::json!({ "sender": "x", "sender_type": "agent", "text": "live chatter no mention" }))
        .send().await.unwrap();
    let got = |v: &Value| v["t"] == "msg" && v["message"]["text"] == "live chatter no mention";
    wait_for(&mut ws, got).await;

    // Turn live off: an unaddressed message should be dropped again.
    client
        .put(format!("{base}/rooms/general/settings"))
        .header("x-admin-token", "adm")
        .json(&serde_json::json!({ "live": false }))
        .send()
        .await
        .unwrap();
    client
        .post(format!("{base}/rooms/general/messages"))
        .json(
            &serde_json::json!({ "sender": "x", "sender_type": "agent", "text": "async chatter" }),
        )
        .send()
        .await
        .unwrap();
    // But an addressed one still arrives — use it as a barrier proving the
    // previous unaddressed one was skipped.
    client.post(format!("{base}/rooms/general/messages"))
        .json(&serde_json::json!({ "sender": "x", "sender_type": "agent", "target": "bob", "text": "hey bob back to async" }))
        .send().await.unwrap();
    let addressed = |v: &Value| v["t"] == "msg" && v["message"]["text"] == "hey bob back to async";
    let f = wait_for(&mut ws, addressed).await;
    // The frame right before must not have been "async chatter".
    assert_ne!(f["message"]["text"], "async chatter");
}

/// The exactly-once promise is not a production-DB accident: local and test
/// servers without SQLite must also absorb a response-lost retry.
#[tokio::test]
async fn message_op_id_is_idempotent_without_a_database() {
    let (port, _guard) = spawn_server().await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "sender": "operator",
        "sender_type": "user",
        "text": "one word, one row",
        "op_id": "web-response-lost"
    });

    let first: Value = client
        .post(format!("{base}/rooms/work/messages"))
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let retry: Value = client
        .post(format!("{base}/rooms/work/messages"))
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(retry["id"], first["id"]);

    let messages: Value = client
        .get(format!("{base}/rooms/work/messages"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(messages.as_array().map(Vec::len), Some(1));
}

/// Retrying one accepted operation returns the original message. It must not
/// broadcast twice, consume the next round-robin turn or create a second row,
/// even after a process restart.
#[tokio::test]
async fn message_op_id_is_idempotent_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir
        .path()
        .join("idempotency.db")
        .to_string_lossy()
        .to_string();
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "sender": "alice",
        "sender_type": "agent",
        "text": "exactly once",
        "op_id": "alice-build-42"
    });
    let first_id;

    {
        let (port, _guard) = spawn_server_env("MASTER", &[("DB_PATH", db.clone())]).await;
        let base = format!("http://127.0.0.1:{port}");
        let mode = client
            .put(format!("{base}/rooms/work/mode"))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({
                "mode": { "mode": "roundrobin", "order": ["alice", "bob"], "turn": 0 }
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(mode.status(), 200);

        let first: Value = client
            .post(format!("{base}/rooms/work/messages"))
            .json(&body)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        first_id = first["id"].as_u64().unwrap();

        let retry: Value = client
            .post(format!("{base}/rooms/work/messages"))
            .json(&body)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(retry["id"], first["id"]);

        let messages: Value = client
            .get(format!("{base}/rooms/work/messages"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(messages.as_array().map(Vec::len), Some(1));
        let mode: Value = client
            .get(format!("{base}/rooms/work/mode"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(mode["turn"], 1, "retry did not consume bob's turn");
    }

    let (port, _guard) = spawn_server_env("MASTER", &[("DB_PATH", db)]).await;
    let base = format!("http://127.0.0.1:{port}");
    let retry: Value = client
        .post(format!("{base}/rooms/work/messages"))
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(retry["id"], first_id);

    let bob: Value = client
        .post(format!("{base}/rooms/work/messages"))
        .json(&serde_json::json!({
            "sender": "bob",
            "sender_type": "agent",
            "text": "different principal",
            "op_id": "alice-build-42"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_ne!(bob["id"], first_id);
    let messages: Value = client
        .get(format!("{base}/rooms/work/messages"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(messages.as_array().map(Vec::len), Some(2));
}

/// An announcement is a different kind of utterance, and it stays one — a
/// restart used to quietly turn it back into small talk.
#[tokio::test]
async fn an_announcement_stays_an_announcement() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("a.db").to_string_lossy().to_string();
    let client = reqwest::Client::new();

    {
        let (port, _g) = spawn_server_env("master", &[("DB_PATH", db.clone())]).await;
        let base = format!("http://127.0.0.1:{port}");
        let m: Value = client
            .post(format!("{base}/rooms/general/messages"))
            .json(&serde_json::json!({
                "sender": "loca-dev", "sender_type": "agent",
                "text": "skill güncellendi", "kind": "announce"
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(m["kind"], "announce");
    }

    let (port, _g) = spawn_server_env("master", &[("DB_PATH", db)]).await;
    let base = format!("http://127.0.0.1:{port}");
    let msgs: Value = client
        .get(format!("{base}/rooms/general/messages"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let announcements: Vec<_> = msgs
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["kind"] == "announce")
        .collect();
    assert_eq!(
        announcements.len(),
        1,
        "it is still an announcement after a restart"
    );
}

/// A connection that dies without a clean leave keeps its count above zero, so
/// the name lingers in the roster with nobody behind it — a ghost. The
/// operator sees somebody who is not there and no ordinary tool removes them,
/// because they all signal a live socket. Kick must remove the entry outright.
#[tokio::test]
async fn a_reconnect_leaves_no_ghost() {
    let (port, _guard) = spawn_server_env("MASTER", &[]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // Reconnect under one name: the second connection evicts the first. The old
    // code did `count += 1` and leaned on the evicted connection to run leave()
    // — but if its reader is dead it never does, so the count leaked and the
    // name stuck in the roster forever. Now join() resets the count to one, so
    // no ghost forms in the first place.
    let a = connect_ws(port, "oda", "reconnector", "agent").await;
    let b = connect_ws(port, "oda", "reconnector", "agent").await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // The evicted first connection never sends a clean leave (forget its Drop).
    std::mem::forget(a);
    // The live second connection then closes cleanly.
    drop(b);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let members: Value = client
        .get(format!("{base}/rooms/oda/members"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        !members
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["name"] == "reconnector"),
        "no ghost: the count was reset on reconnect, so a clean leave empties it"
    );

    // And kick still removes an entry outright, even a stale one.
    let c = connect_ws(port, "oda", "leftover", "agent").await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    std::mem::forget(c); // reader dies without a clean leave
    client
        .post(format!("{base}/rooms/oda/moderate"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "action": "kick", "name": "leftover" }))
        .send()
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let after: Value = client
        .get(format!("{base}/rooms/oda/members"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        !after
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["name"] == "leftover"),
        "kick removes the entry whether or not a socket is still listening"
    );
}

/// A human may keep the same operator identity open on a laptop and a phone.
/// Those are two readers of one seat, not competing agent runtimes: neither
/// browser may evict the other and closing one must leave the other ONLINE.
#[tokio::test]
async fn the_same_user_can_read_from_two_web_clients_without_reconnect_ping_pong() {
    let (port, _guard) = spawn_server_env("MASTER", &[("LEGACY_WS_QUERY_AUTH", "0".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let new_browser_session = || {
        let client = client.clone();
        let base = base.clone();
        async move {
            let value: Value = client
                .post(format!("{base}/sessions"))
                .header("x-admin-token", "MASTER")
                .json(&serde_json::json!({ "name": "operator", "kind": "user" }))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            value["session_token"].as_str().unwrap().to_string()
        }
    };
    let laptop_session = new_browser_session().await;
    let phone_session = new_browser_session().await;
    let url = format!("ws://127.0.0.1:{port}/ws?room=oda&name=operator&type=user");
    let mut laptop = connect_ws_protocols(
        url.clone(),
        &["loca.v1".into(), format!("loca.session.{laptop_session}")],
    )
    .await;
    let mut phone = connect_ws_protocols(
        url,
        &["loca.v1".into(), format!("loca.session.{phone_session}")],
    )
    .await;
    // An HTTP 101 only proves the upgrade completed. Wait until each server
    // task has subscribed to the room broadcast before posting; otherwise a
    // heavily loaded CI runner can publish in the handshake→subscribe gap and
    // make this concurrency test flaky for a reason unrelated to takeover.
    for ws in [&mut laptop, &mut phone] {
        let _ = wait_for(ws, |frame| frame["t"] == "history").await;
    }

    let posted: Value = client
        .post(format!("{base}/rooms/oda/messages"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({
            "sender": "debug",
            "sender_type": "agent",
            "text": "both screens must receive this"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let message_id = posted["id"].as_u64().unwrap();

    for ws in [&mut laptop, &mut phone] {
        let delivered = wait_for(ws, |frame| {
            frame["t"] == "msg" && frame["message"]["id"] == message_id
        })
        .await;
        assert_eq!(
            delivered["message"]["text"],
            "both screens must receive this"
        );
    }

    drop(laptop);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let while_phone_remains: Value = client
        .get(format!("{base}/rooms/oda/members"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        while_phone_remains
            .as_array()
            .unwrap()
            .iter()
            .any(|member| member["name"] == "operator"),
        "closing one browser must not remove the other from the roster"
    );

    drop(phone);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let after_both_close: Value = client
        .get(format!("{base}/rooms/oda/members"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        !after_both_close
            .as_array()
            .unwrap()
            .iter()
            .any(|member| member["name"] == "operator"),
        "one identity still occupies only one seat and leaves cleanly"
    );
}
