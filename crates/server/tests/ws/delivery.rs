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
        serde_json::json!({ "t": "send", "text": "hi from ws" })
            .to_string()
            .into(),
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
    // The handshake answers before the server seats the watcher; a reply
    // posted before then is only in its history, never a live frame.
    wait_for_member(&client, &base, "general", "watcher", None).await;

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
            serde_json::json!({ "t": "typing", "on": true })
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    let is_typing = |v: &Value| v["t"] == "typing" && v["name"] == "typer" && v["on"] == true;
    wait_for(&mut watcher, is_typing).await;
}

#[tokio::test]
async fn reactions_are_whitelisted_persisted_and_directed_to_message_owner() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir
        .path()
        .join("reactions.db")
        .to_string_lossy()
        .into_owned();
    let (port, _guard) = spawn_server_env("", &[("DB_PATH", db)]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let mut owner = connect_ws(port, "general", "author", "agent").await;
    let mut reader = connect_ws(port, "general", "reader", "user").await;
    // Both must be seated (and so subscribed) before the reaction is
    // broadcast; the handshake alone does not prove it.
    wait_for_member(&client, &base, "general", "author", None).await;
    wait_for_member(&client, &base, "general", "reader", None).await;

    let message: Value = client
        .post(format!("{base}/rooms/general/messages"))
        .json(&serde_json::json!({ "sender": "author", "sender_type": "agent", "text": "landed" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let message_id = message["id"].as_u64().unwrap();

    let changed = client.post(format!("{base}/rooms/general/messages/{message_id}/reactions"))
        .json(&serde_json::json!({ "emoji": "✦", "active": true, "reactor": "reader", "reactor_type": "user" }))
        .send().await.unwrap();
    assert_eq!(changed.status(), 200);

    let is_reaction = |v: &Value| v["t"] == "reaction" && v["reaction"]["message_id"] == message_id;
    let owner_frame = wait_for(&mut owner, is_reaction).await;
    assert_eq!(owner_frame["reaction"]["owner"], "author");
    assert_eq!(owner_frame["reaction"]["actors"][0], "reader");
    wait_for(&mut reader, is_reaction).await;

    let saved: Vec<Value> = client
        .get(format!("{base}/rooms/general/reactions"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0]["emoji"], "✦");

    let bad = client
        .post(format!(
            "{base}/rooms/general/messages/{message_id}/reactions"
        ))
        .json(&serde_json::json!({ "emoji": "🔥", "active": true, "reactor": "reader" }))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);

    let own = client
        .post(format!(
            "{base}/rooms/general/messages/{message_id}/reactions"
        ))
        .json(&serde_json::json!({ "emoji": "✓", "active": true, "reactor": "author" }))
        .send()
        .await
        .unwrap();
    assert_eq!(own.status(), 409);
}

#[tokio::test]
async fn filter_msg_suppresses_noise_but_delivers_messages() {
    let (port, _guard) = spawn_server().await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // Connect an events-only listener (?filter=msg).
    let url = format!("ws://127.0.0.1:{port}/ws?room=general&name=filtered&type=agent&filter=msg");
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    wait_for_member(&client, &base, "general", "filtered", None).await;
    let memory = wait_for(&mut ws, |frame| frame["t"] == "memory").await;
    assert_eq!(memory["status"], "absent");

    // Cause noise (a typing frame from someone else) then a real message.
    use futures_util::SinkExt;
    let mut noisy = connect_ws(port, "general", "noisy", "user").await;
    noisy
        .send(WsMessage::Text(
            serde_json::json!({ "t": "typing", "on": true })
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    client
        .post(format!("{base}/rooms/general/messages"))
        .json(&serde_json::json!({ "sender": "x", "sender_type": "agent", "text": "real one" }))
        .send()
        .await
        .unwrap();

    // The filtered client's first frame is the positive memory revision
    // checkpoint immediately preceding the message. It still receives no
    // typing/history/members noise.
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
        first["t"], "memoryversion",
        "events-only clients must get the positive memory checkpoint"
    );
    let message = wait_for(&mut ws, |frame| frame["t"] == "msg").await;
    assert_eq!(message["message"]["text"], "real one");
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
    wait_for_member(&client, &base, "general", "bob", None).await;
    let memory = wait_for(&mut ws, |frame| frame["t"] == "memory").await;
    assert_eq!(memory["status"], "absent");

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

    // First frame bob receives is the positive memory checkpoint for the @all
    // wake; the earlier two messages and their checkpoints were dropped.
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
    assert_eq!(first["t"], "memoryversion");
    let addressed = wait_for(&mut ws, |frame| frame["t"] == "msg").await;
    assert_eq!(addressed["message"]["text"], "everyone listen");

    // An @bob mention in text (no target) should also arrive.
    post(None, "ping @bob you there").await;
    let is_bob = |v: &Value| v["t"] == "msg" && v["message"]["text"] == "ping @bob you there";
    wait_for(&mut ws, is_bob).await;
}

#[tokio::test]
async fn reply_wakes_owner_even_when_older_than_the_tail() {
    // A real DB is required: past the in-memory tail the author can only come
    // from Store::message_owner, which is a no-op in memory-only mode.
    // The server binds its own port (PORT=0): a port reserved here could be
    // taken by another test's server before this one bound it.
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("reply-tail.db");
    let (port, _guard) =
        spawn_server_env("", &[("DB_PATH", db.to_string_lossy().to_string())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // bob's root, then enough traffic to evict it from the 200-message in-memory
    // tail so the author can only come from the durable store.
    let root: Value = client
        .post(format!("{base}/rooms/general/messages"))
        .json(&serde_json::json!({ "sender": "bob", "sender_type": "agent", "text": "old root" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let root_id = root["id"].as_u64().unwrap();
    // Distinct senders dodge the per-sender rate limit; 205 > HISTORY_LIMIT (200)
    // guarantees the root has rolled out of the tail.
    for i in 0..205 {
        client
            .post(format!("{base}/rooms/general/messages"))
            .json(&serde_json::json!({
                "sender": format!("filler{i}"), "sender_type": "agent", "text": "noise"
            }))
            .send()
            .await
            .unwrap();
    }

    // bob connects mentions-only AFTER the tail rolled past the root.
    let url = format!(
        "ws://127.0.0.1:{port}/ws?room=general&name=bob&type=agent\
         &filter=mentions&turn_max=1"
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    wait_for_member(&client, &base, "general", "bob", None).await;
    let memory = wait_for(&mut ws, |frame| frame["t"] == "memory").await;
    assert_eq!(memory["status"], "absent");

    // A reply to the long-evicted root: only Store::message_owner can still name
    // the author, and bob must still be woken.
    client
        .post(format!("{base}/rooms/general/messages"))
        .json(&serde_json::json!({
            "sender": "x", "sender_type": "agent", "text": "late answer", "reply_to": root_id
        }))
        .send()
        .await
        .unwrap();

    let frame = wait_for(&mut ws, |v| {
        v["t"] == "msg" && v["message"]["text"] == "late answer"
    })
    .await;
    assert_eq!(frame["message"]["reply_to_sender"], "bob");
    assert_eq!(frame["message"]["reply_to"], root_id);
}

#[tokio::test]
async fn a_reply_and_an_explicit_target_wake_both() {
    let (port, _guard) = spawn_server().await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let root: Value = client
        .post(format!("{base}/rooms/general/messages"))
        .json(&serde_json::json!({ "sender": "bob", "sender_type": "agent", "text": "root" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let root_id = root["id"].as_u64().unwrap();

    // Two mentions-only listeners: the reply author (bob) and a different,
    // explicit target (carol).
    let (mut bob, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/ws?room=general&name=bob&type=agent&filter=mentions&turn_max=1"
    ))
    .await
    .unwrap();
    let (mut carol, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/ws?room=general&name=carol&type=agent&filter=mentions&turn_max=1"
    ))
    .await
    .unwrap();
    wait_for_member(&client, &base, "general", "bob", None).await;
    wait_for_member(&client, &base, "general", "carol", None).await;

    // Reply to bob's message, explicitly targeting carol. Both must wake: carol
    // via target, bob via reply_to_sender — the reply author is a SEPARATE
    // recipient, not overridden by the explicit target.
    client
        .post(format!("{base}/rooms/general/messages"))
        .json(&serde_json::json!({
            "sender": "x", "sender_type": "agent", "target": "carol",
            "text": "for both", "reply_to": root_id
        }))
        .send()
        .await
        .unwrap();

    let is_it = |v: &Value| v["t"] == "msg" && v["message"]["text"] == "for both";
    let bf = wait_for(&mut bob, is_it).await;
    let cf = wait_for(&mut carol, is_it).await;
    assert_eq!(bf["message"]["reply_to_sender"], "bob");
    assert_eq!(cf["message"]["target"], "carol");
}

#[tokio::test]
async fn a_reply_to_an_unknown_or_cross_room_id_addresses_nobody() {
    let (port, _guard) = spawn_server().await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let mut watcher = connect_ws(port, "general", "watcher", "user").await;
    // Seated before anything is posted, so the replies arrive as live frames.
    wait_for_member(&client, &base, "general", "watcher", None).await;

    // A real message, but in ANOTHER room.
    let elsewhere: Value = client
        .post(format!("{base}/rooms/sales/messages"))
        .json(&serde_json::json!({ "sender": "dave", "sender_type": "agent", "text": "over here" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let cross_id = elsewhere["id"].as_u64().unwrap();

    // A reply in `general` to a cross-room id resolves no author (message_owner
    // is room-scoped), so nobody is addressed.
    client
        .post(format!("{base}/rooms/general/messages"))
        .json(&serde_json::json!({
            "sender": "x", "sender_type": "agent", "text": "cross room", "reply_to": cross_id
        }))
        .send()
        .await
        .unwrap();
    let f1 = wait_for(&mut watcher, |v| v["message"]["text"] == "cross room").await;
    assert!(
        f1["message"]["reply_to_sender"].is_null(),
        "a cross-room reply addresses nobody"
    );

    // A wholly unknown id likewise addresses nobody.
    client
        .post(format!("{base}/rooms/general/messages"))
        .json(&serde_json::json!({
            "sender": "x", "sender_type": "agent", "text": "ghost reply", "reply_to": 999999
        }))
        .send()
        .await
        .unwrap();
    let f2 = wait_for(&mut watcher, |v| v["message"]["text"] == "ghost reply").await;
    assert!(
        f2["message"]["reply_to_sender"].is_null(),
        "a reply to an unknown id addresses nobody"
    );
}

#[tokio::test]
async fn a_self_reply_addresses_nobody() {
    let (port, _guard) = spawn_server().await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let mut watcher = connect_ws(port, "general", "watcher", "user").await;
    // Transport upgrade is not room subscription. Do not race publication
    // against the watcher joining, or the reply may be in history, not live.
    wait_for_member(&client, &base, "general", "watcher", None).await;

    let root: Value = client
        .post(format!("{base}/rooms/general/messages"))
        .json(&serde_json::json!({ "sender": "bob", "sender_type": "agent", "text": "root" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let root_id = root["id"].as_u64().unwrap();

    // bob answers bob: the author equals the sender, so nobody is addressed and
    // the sender can never wake itself.
    client
        .post(format!("{base}/rooms/general/messages"))
        .json(&serde_json::json!({
            "sender": "bob", "sender_type": "agent", "text": "talking to myself", "reply_to": root_id
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let frame = wait_for(&mut watcher, |v| {
        v["message"]["text"] == "talking to myself"
    })
    .await;
    assert!(
        frame["message"]["reply_to_sender"].is_null(),
        "a self-reply addresses nobody"
    );
}

#[tokio::test]
async fn a_message_owner_storage_error_rejects_the_reply() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("owner-error.db");
    let db = db_path.to_string_lossy().to_string();
    let (port, _guard) = spawn_server_env("", &[("DB_PATH", db)]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // bob's root, then enough traffic to evict it from the tail so the reply
    // must consult Store::message_owner rather than the in-memory history.
    let root: Value = client
        .post(format!("{base}/rooms/general/messages"))
        .json(&serde_json::json!({ "sender": "bob", "sender_type": "agent", "text": "root" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let root_id = root["id"].as_u64().unwrap();
    for i in 0..205 {
        client
            .post(format!("{base}/rooms/general/messages"))
            .json(&serde_json::json!({ "sender": format!("f{i}"), "sender_type": "agent", "text": "n" }))
            .send()
            .await
            .unwrap();
    }

    // Corrupt exactly what message_owner reads: overwrite the root's `sender`
    // with a BLOB. The column is TEXT NOT NULL, so a blob still satisfies NOT
    // NULL but fails to decode as a String — the row still EXISTS (an unknown id
    // would be a clean Ok(None)), so this isolates a storage READ error. It must
    // reject the reply, never silently post it and drop the wake.
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.busy_timeout(Duration::from_secs(5)).unwrap();
        conn.execute(
            "UPDATE messages SET sender = X'00' WHERE id = ?1",
            rusqlite::params![root_id],
        )
        .unwrap();
    }

    let resp = client
        .post(format!("{base}/rooms/general/messages"))
        .json(&serde_json::json!({
            "sender": "x", "sender_type": "agent", "text": "late answer", "reply_to": root_id
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        503,
        "a message_owner storage error must reject the reply, not silently drop the wake"
    );
}

#[tokio::test]
async fn a_named_lead_hears_the_whole_room_until_the_title_ends() {
    let (port, _guard) = spawn_server_with("adm").await;
    create_test_room(port, "adm", "general").await;
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
    wait_for_member(&client, &base, "general", "bob", None).await;
    let memory = wait_for(&mut ws, |frame| frame["t"] == "memory").await;
    assert_eq!(memory["status"], "absent");

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
    wait_for_member(&client, &base, "general", "bob", None).await;
    let memory = wait_for(&mut ws, |frame| frame["t"] == "memory").await;
    assert_eq!(memory["status"], "absent");
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
    create_test_room(port, "adm", "general").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // bob connects mentions-only.
    let url = format!(
        "ws://127.0.0.1:{port}/ws?room=general&name=bob&type=agent\
         &filter=mentions&turn_max=1"
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    // The handshake only proves the transport is up. Without waiting for the
    // room to register bob, the post below can be broadcast before he is
    // subscribed, and he never receives it: this test then timed out in CI.
    wait_for_member(&client, &base, "general", "bob", None).await;

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
    // Read every message frame up to the addressed one: the unaddressed
    // "async chatter" sent before it must not be among them. (Comparing only
    // the addressed frame's own text proved nothing.)
    let mut texts = Vec::new();
    loop {
        let frame = wait_for(&mut ws, |v: &Value| v["t"] == "msg").await;
        let text = frame["message"]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let done = text == "hey bob back to async";
        texts.push(text);
        if done {
            break;
        }
    }
    assert!(
        !texts.iter().any(|text| text == "async chatter"),
        "with live off, an unaddressed message reached a mentions-only socket: {texts:?}"
    );
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
        create_test_room(port, "MASTER", "work").await;
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
        create_test_room(port, "master", "general").await;
        let base = format!("http://127.0.0.1:{port}");
        let m: Value = client
            .post(format!("{base}/rooms/general/messages"))
            .json(&serde_json::json!({
                "sender": "loca-dev", "sender_type": "agent",
                "text": "skill updated", "kind": "announce"
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
    create_test_room(port, "MASTER", "parlor").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // Reconnect under one name: the second connection evicts the first. The old
    // code did `count += 1` and leaned on the evicted connection to run leave()
    // — but if its reader is dead it never does, so the count leaked and the
    // name stuck in the roster forever. Now join() resets the count to one, so
    // no ghost forms in the first place.
    let a = connect_ws(port, "parlor", "reconnector", "agent").await;
    // The handshake answers before the server seats the connection. Without
    // this wait the two joins can land in either order: when `a` joins last
    // it is the live seat, `b` is the evicted one, and forgetting `a` below
    // leaves the name seated for ever (the test then times out waiting for
    // it to leave). The first connection must be seated before the second
    // arrives for "the second evicts the first" to be what is tested.
    wait_for_member(&client, &base, "parlor", "reconnector", None).await;
    let mut b = connect_ws(port, "parlor", "reconnector", "agent").await;
    let _ = wait_for(&mut b, |frame| frame["t"] == "history").await;

    // The evicted first connection never sends a clean leave (forget its Drop).
    std::mem::forget(a);
    // The live second connection then closes cleanly.
    drop(b);
    // Closing the socket schedules cleanup asynchronously. Under a loaded CI
    // runner a fixed sleep races that cleanup, so poll the externally visible
    // invariant within a bounded deadline instead of asserting scheduler speed.
    wait_for_member_absent(&client, &base, "parlor", "reconnector").await;

    // And kick still removes an entry outright, even a stale one.
    let mut c = connect_ws(port, "parlor", "leftover", "agent").await;
    let _ = wait_for(&mut c, |frame| frame["t"] == "history").await;
    std::mem::forget(c); // reader dies without a clean leave
    client
        .post(format!("{base}/rooms/parlor/moderate"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "action": "kick", "name": "leftover" }))
        .send()
        .await
        .unwrap();
    wait_for_member_absent(&client, &base, "parlor", "leftover").await;
}

/// Frames posted to a loca to put a reader behind: each one is large, and
/// together they are more than the server can push into one socket whose
/// client reads nothing, yet fewer than the loca bus holds (256), so none is
/// dropped as lag.
const BACKLOG_FRAMES: usize = 200;

/// Open a loca socket whose client keeps a small receive buffer and reads
/// nothing until the test does. Once a few frames are in flight the server's
/// writes to it stall, and later frames stay queued on the loca bus for it.
async fn slow_reader(
    port: u16,
    query: &str,
    credential: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.set_recv_buffer_size(4096).unwrap();
    let stream = socket.connect(([127, 0, 0, 1], port).into()).await.unwrap();
    let mut request = format!("ws://127.0.0.1:{port}/ws?{query}")
        .into_client_request()
        .unwrap();
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_str(&format!("loca.v1, {credential}")).unwrap(),
    );
    let (ws, _) =
        tokio_tungstenite::client_async(request, tokio_tungstenite::MaybeTlsStream::Plain(stream))
            .await
            .unwrap();
    ws
}

/// Post `BACKLOG_FRAMES` large messages as the master (not rate-limited).
async fn post_backlog(client: &reqwest::Client, base: &str, room: &str, master: &str) {
    let filler = "x".repeat(40_000);
    for i in 0..BACKLOG_FRAMES {
        client
            .post(format!("{base}/rooms/{room}/messages"))
            .header("x-admin-token", master)
            .json(&serde_json::json!({
                "sender": "backlog", "sender_type": "agent", "text": format!("{i} {filler}")
            }))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
    }
}

/// A person taken out of a loca is told so, even when their socket is behind.
/// Kick, release and invite revocation end the credential and then broadcast
/// `kicked`. A socket that still had ordinary frames queued used to reach one
/// of them first, find its credential gone and close without a word: the
/// browser showed a connection error and the kit a reconnect instead of
/// "you were removed". The `kicked` frame must arrive, and be the last frame.
#[tokio::test]
async fn a_removed_reader_behind_on_frames_still_hears_kicked() {
    for action in ["kick", "release", "revoke"] {
        let master = "backlog-master";
        let (port, _guard) = spawn_server_env(
            master,
            &[
                ("REQUIRE_INVITE", "1".into()),
                ("LEGACY_WS_QUERY_AUTH", "0".into()),
            ],
        )
        .await;
        create_test_room(port, master, "backlog-room").await;
        let base = format!("http://127.0.0.1:{port}");
        let client = reqwest::Client::new();
        let invite = invite_for(&base, master, "backlog-room", "behind").await;
        let mut seat = slow_reader(
            port,
            "room=backlog-room&name=behind&type=agent",
            &format!("loca.room.{invite}"),
        )
        .await;
        wait_for(&mut seat, |v| v["t"] == "history").await;

        post_backlog(&client, &base, "backlog-room", master).await;
        let response = if action == "revoke" {
            client
                .delete(format!(
                    "{base}/rooms/backlog-room/invites/{}",
                    invite_ref(&invite)
                ))
                .header("x-admin-token", master)
                .send()
                .await
                .unwrap()
        } else {
            client
                .post(format!("{base}/rooms/backlog-room/moderate"))
                .header("x-admin-token", master)
                .json(&serde_json::json!({ "action": action, "name": "behind" }))
                .send()
                .await
                .unwrap()
        };
        assert!(
            response.status().is_success(),
            "{action}: {}",
            response.status()
        );

        let heard = read_until_close(&mut seat, action).await;
        let delivered = heard.iter().filter(|v| v["t"] == "msg").count();
        assert!(
            delivered < BACKLOG_FRAMES,
            "{action}: the socket was never behind, so nothing was tested"
        );
        let last = heard.last().map(|v| (&v["t"], &v["name"], &v["banned"]));
        assert_eq!(
            last,
            Some((
                &Value::from("kicked"),
                &Value::from("behind"),
                &Value::from(false)
            )),
            "{action}: the last frame must say the person was removed; \
             {delivered} of {BACKLOG_FRAMES} messages delivered, last frame types: {:?}",
            heard
                .iter()
                .rev()
                .take(3)
                .map(|v| v["t"].clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            heard.iter().filter(|v| v["t"] == "kicked").count(),
            1,
            "{action}: told once"
        );
    }
}

/// Every text frame a socket reads before it closes.
async fn read_until_close(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    label: &str,
) -> Vec<Value> {
    let mut heard = Vec::new();
    tokio::time::timeout(ASYNC_STATE_HANG_GUARD, async {
        loop {
            match ws.next().await {
                Some(Ok(WsMessage::Text(text))) => {
                    let value: Value = serde_json::from_str(&text).unwrap();
                    assert_listed_frame(&value);
                    heard.push(value);
                }
                Some(Ok(WsMessage::Close(_))) | None | Some(Err(_)) => return,
                Some(Ok(_)) => {}
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{label} must close the loca socket"));
    heard
}

/// The other side of the test above: a credential that ends without a
/// removal (here a session sign-out) still closes a socket that is behind,
/// before the backlog is through, and tells it of no removal.
#[tokio::test]
async fn a_signed_out_reader_behind_on_frames_closes_without_kicked() {
    let master = "backlog-master";
    let (port, _guard) = spawn_server_env(master, &[("LEGACY_WS_QUERY_AUTH", "0".into())]).await;
    create_test_room(port, master, "backlog-room").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let session = session_with(&base, ("x-admin-token", master), "operator", None).await;
    let mut seat = slow_reader(
        port,
        "room=backlog-room&name=operator&type=user",
        &format!("loca.session.{session}"),
    )
    .await;
    wait_for(&mut seat, |v| v["t"] == "history").await;

    post_backlog(&client, &base, "backlog-room", master).await;
    client
        .delete(format!("{base}/sessions"))
        .header("x-session-token", &session)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    let heard = read_until_close(&mut seat, "sign-out").await;
    let delivered = heard.iter().filter(|v| v["t"] == "msg").count();
    assert!(
        delivered < BACKLOG_FRAMES,
        "the socket was never behind, so nothing was tested"
    );
    assert!(
        !heard.iter().any(|v| v["t"] == "kicked"),
        "a sign-out is not a removal"
    );
}

/// An agent that reconnects keeps its seat when the old connection only
/// notices its end afterwards. Here the old connection is behind on frames
/// when the new one joins, so the eviction aimed at it waits in its queue; its
/// client then drops, the server's next write to it fails and it leaves
/// without ever reading the eviction. That leave used to free the seat the
/// new connection had just taken: connected and receiving, but missing from
/// the roster. When the new connection closes the seat is freed (no ghost).
#[tokio::test]
async fn a_late_leave_of_the_old_connection_keeps_the_reconnected_seat() {
    let master = "backlog-master";
    let (port, _guard) = spawn_server_env(
        master,
        &[
            ("REQUIRE_INVITE", "1".into()),
            ("LEGACY_WS_QUERY_AUTH", "0".into()),
        ],
    )
    .await;
    create_test_room(port, master, "backlog-room").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let invite = invite_for(&base, master, "backlog-room", "reconnector").await;
    let query = "room=backlog-room&name=reconnector&type=agent";
    let mut old = slow_reader(port, query, &format!("loca.room.{invite}")).await;
    wait_for(&mut old, |v| v["t"] == "history").await;
    post_backlog(&client, &base, "backlog-room", master).await;

    let mut new = connect_ws_protocols(
        format!("ws://127.0.0.1:{port}/ws?{query}"),
        &["loca.v1".into(), format!("loca.room.{invite}")],
    )
    .await;
    wait_for(&mut new, |v| v["t"] == "history").await;
    // Read the new connection past its own join: after this marker the next
    // roster it hears is the one the old connection's leave sends.
    client
        .post(format!("{base}/rooms/backlog-room/messages"))
        .header("x-admin-token", master)
        .json(&serde_json::json!({
            "sender": "backlog", "sender_type": "agent", "text": "marker"
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    wait_for(&mut new, |v| {
        v["t"] == "msg" && v["message"]["text"] == "marker"
    })
    .await;

    // The old client goes away with frames still unread: its socket is reset.
    drop(old);
    let roster = wait_for(&mut new, |v| v["t"] == "members").await;
    let seated: Vec<&str> = roster["members"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["name"].as_str())
        .collect();
    assert!(
        seated.contains(&"reconnector"),
        "the live connection lost its seat to the old one's leave: {seated:?}"
    );
    let members: Vec<Value> = client
        .get(format!("{base}/rooms/backlog-room/members"))
        .header("x-room-token", &invite)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        members.iter().any(|m| m["name"] == "reconnector"),
        "{members:?}"
    );

    drop(new);
    tokio::time::timeout(ASYNC_STATE_HANG_GUARD, async {
        loop {
            let members: Vec<Value> = client
                .get(format!("{base}/rooms/backlog-room/members"))
                .header("x-admin-token", master)
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            if !members.iter().any(|m| m["name"] == "reconnector") {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the seat is freed once its last connection closes");
}

/// A human may keep the same operator identity open on a laptop and a phone.
/// Those are two readers of one seat, not competing agent runtimes: neither
/// browser may evict the other and closing one must leave the other ONLINE.
#[tokio::test]
async fn the_same_user_can_read_from_two_web_clients_without_reconnect_ping_pong() {
    let (port, _guard) = spawn_server_env("MASTER", &[("LEGACY_WS_QUERY_AUTH", "0".into())]).await;
    create_test_room(port, "MASTER", "parlor").await;
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
    let url = format!("ws://127.0.0.1:{port}/ws?room=parlor&name=operator&type=user");
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
        .post(format!("{base}/rooms/parlor/messages"))
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
    let while_phone_remains: Value = client
        .get(format!("{base}/rooms/parlor/members"))
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
    wait_for_member_absent(&client, &base, "parlor", "operator").await;
}
