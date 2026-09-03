//! Attention, care, reminders, and durable wait wake-up.

use super::*;

#[tokio::test]
async fn exact_cross_loca_caretaker_call_reaches_iye_without_opening_source_history() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("summon.db").to_string_lossy().to_string();
    let (port, _guard) = spawn_server_env(
        "",
        &[
            ("LOCA_AGENT_ROOM", "iye".into()),
            ("RESERVED_LOCA", "iye".into()),
            ("LOCA_CARETAKERS", "loca-dev,loca-care".into()),
            ("DB_PATH", db),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let url = format!(
        "ws://127.0.0.1:{port}/ws?room=iye&name=loca-dev&type=agent&filter=mentions&turn_max=1"
    );
    let (mut caretaker, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let observer_url = format!("ws://127.0.0.1:{port}/ws?room=iye&name=loca-care&type=agent");
    let (mut observer, _) = tokio_tungstenite::connect_async(observer_url)
        .await
        .unwrap();
    let _ = wait_for(&mut observer, |value| value["t"] == "history").await;
    let client = reqwest::Client::new();

    client
        .post(format!("{base}/rooms/reviewer/messages"))
        .json(&serde_json::json!({
            "sender": "operator",
            "sender_type": "user",
            "target": "loca-dev",
            "text": "@loca-dev monitoring hattına bak"
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    let summoned = wait_for(&mut caretaker, |v| {
        v["t"] == "care"
            && v["signal"]["reason"] == "direct_summon"
            && v["signal"]["context"][0]["text"] == "@loca-dev monitoring hattına bak"
    })
    .await;
    // The envelope is re-homed onto Iye (the delivery socket's room); the true
    // origin travels in source_room. A socket in Iye must never see a Care whose
    // room is another loca.
    assert_eq!(summoned["signal"]["room"], "iye");
    assert_eq!(summoned["signal"]["source_room"], "reviewer");
    assert_eq!(summoned["signal"]["owner"], "loca-dev");
    assert_eq!(summoned["signal"]["context"].as_array().unwrap().len(), 1);
    let leaked = tokio::time::timeout(Duration::from_millis(300), async {
        loop {
            let Some(Ok(message)) = observer.next().await else {
                return false;
            };
            if let WsMessage::Text(text) = message {
                let value: Value = serde_json::from_str(&text).unwrap();
                if value["t"] == "care" {
                    return true;
                }
            }
        }
    })
    .await;
    assert!(
        leaked.is_err(),
        "an all-filter Iye client must not see another caretaker's source envelope"
    );
    let summon_id = summoned["signal"]["id"].as_str().unwrap().to_string();
    let attention_id = summoned["signal"]["attention_id"]
        .as_str()
        .unwrap()
        .to_string();

    let reviewer_attentions: Vec<Value> = client
        .get(format!("{base}/rooms/reviewer/attentions"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(reviewer_attentions.len(), 1);
    assert_eq!(reviewer_attentions[0]["id"], attention_id);
    assert_eq!(reviewer_attentions[0]["status"], "open");

    let iye_history: Vec<Value> = client
        .get(format!("{base}/rooms/iye/messages"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        iye_history.is_empty(),
        "a summon belongs in the durable outbox, not Iye chat history"
    );

    // No transport ACK: reconnect replays the exact durable summon.
    drop(caretaker);
    let replay_url = format!(
        "ws://127.0.0.1:{port}/ws?room=iye&name=loca-dev&type=agent&filter=mentions&turn_max=1"
    );
    let (mut caretaker, _) = tokio_tungstenite::connect_async(replay_url).await.unwrap();
    let replay = wait_for(&mut caretaker, |v| {
        v["t"] == "care" && v["signal"]["id"] == summon_id
    })
    .await;
    assert_eq!(replay["signal"]["reason"], "direct_summon");

    client
        .post(format!("{base}/rooms/reviewer/messages"))
        .json(&serde_json::json!({
            "sender": "operator",
            "sender_type": "user",
            "target": "all",
            "text": "@all ordinary announcement"
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(250), caretaker.next())
            .await
            .is_err(),
        "@all must not wake a caretaker"
    );
}

#[tokio::test]
async fn operator_all_in_iye_reaches_every_caretaker_mentions_stream() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("iye-all.db").to_string_lossy().to_string();
    let (port, _guard) = spawn_server_env(
        "",
        &[
            ("LOCA_AGENT_ROOM", "iye".into()),
            ("RESERVED_LOCA", "iye".into()),
            ("LOCA_CARETAKERS", "loca-dev,loca-care".into()),
            ("DB_PATH", db),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let open = |name: &str| {
        tokio_tungstenite::connect_async(format!(
            "ws://127.0.0.1:{port}/ws?room=iye&name={name}&type=agent&filter=mentions&turn_max=1"
        ))
    };
    let (mut dev, _) = open("loca-dev").await.unwrap();
    let (mut care, _) = open("loca-care").await.unwrap();

    // The HTTP upgrade can finish just before ws_session records the seat.
    // Wait for both subscriptions instead of racing the first broadcast.
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let members: Vec<Value> = reqwest::get(format!("{base}/rooms/iye/members"))
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            if members.len() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("caretaker sockets did not join iye");

    reqwest::Client::new()
        .post(format!("{base}/rooms/iye/messages"))
        .json(&serde_json::json!({
            "sender": "operator",
            "sender_type": "user",
            "text": "@all cevap ver"
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    for socket in [&mut dev, &mut care] {
        let delivered = wait_for(socket, |value| {
            value["t"] == "msg" && value["message"]["text"] == "@all cevap ver"
        })
        .await;
        assert_eq!(delivered["message"]["sender"], "operator");
    }
}

/// P0#2 invariant: the server must never place a Care envelope on a socket
/// bound to a different room than signal.room. A caretaker socket in Iye that is
/// summoned from another loca must see the wake homed to Iye — never carrying the
/// source room as signal.room. Fails on master @44ef95b (envelope arrives with
/// signal.room == "reviewer" on the Iye socket).
#[tokio::test]
async fn care_for_one_room_is_not_delivered_to_a_socket_in_another_room() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir
        .path()
        .join("crossroom-invariant.db")
        .to_string_lossy()
        .to_string();
    let (port, _guard) = spawn_server_env(
        "",
        &[
            ("LOCA_AGENT_ROOM", "iye".into()),
            ("RESERVED_LOCA", "iye".into()),
            ("LOCA_CARETAKERS", "loca-dev,loca-care".into()),
            ("DB_PATH", db),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    // A socket bound to the home loca (iye).
    let url = format!(
        "ws://127.0.0.1:{port}/ws?room=iye&name=loca-dev&type=agent&filter=mentions&turn_max=1"
    );
    let (mut caretaker, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let client = reqwest::Client::new();

    // Summon loca-dev from a DIFFERENT loca ("reviewer").
    client
        .post(format!("{base}/rooms/reviewer/messages"))
        .json(&serde_json::json!({
            "sender": "operator",
            "sender_type": "user",
            "target": "loca-dev",
            "text": "@loca-dev cross-room summon"
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    let care = wait_for(&mut caretaker, |v| {
        v["t"] == "care" && v["signal"]["reason"] == "direct_summon"
    })
    .await;
    // The invariant: the envelope delivered to an iye socket is homed to iye,
    // and the source loca is carried out-of-band in source_room. It must never
    // arrive with signal.room set to another loca.
    assert_eq!(
        care["signal"]["room"], "iye",
        "a Care envelope on an iye socket must be homed to iye, not the source room"
    );
    assert_ne!(care["signal"]["room"], "reviewer");
    assert_eq!(care["signal"]["source_room"], "reviewer");
}

#[tokio::test]
async fn direct_caretaker_summon_and_message_commit_or_fail_together() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir
        .path()
        .join("summon-atomic.db")
        .to_string_lossy()
        .to_string();
    let (port, _guard) = spawn_server_env(
        "MASTER",
        &[
            ("LOCA_AGENT_ROOM", "iye".into()),
            ("RESERVED_LOCA", "iye".into()),
            ("LOCA_CARETAKERS", "loca-care".into()),
            ("DB_PATH", db.clone()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let sql = rusqlite::Connection::open(&db).unwrap();
    sql.execute_batch(
        "CREATE TRIGGER reject_summon_attention
         BEFORE INSERT ON attentions
         BEGIN SELECT RAISE(FAIL, 'injected summon failure'); END;",
    )
    .unwrap();
    let body = serde_json::json!({
        "sender": "operator", "sender_type": "user",
        "target": "loca-care", "text": "@loca-care inspect",
        "op_id": "summon-once"
    });
    let failed = client
        .post(format!("{base}/rooms/project/messages"))
        .header("x-admin-token", "MASTER")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(failed.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    let counts: (u64, u64, u64) = sql
        .query_row(
            "SELECT
                (SELECT count(*) FROM messages),
                (SELECT count(*) FROM attentions),
                (SELECT count(*) FROM care_outbox)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(counts, (0, 0, 0));

    sql.execute_batch("DROP TRIGGER reject_summon_attention")
        .unwrap();
    let repaired = client
        .post(format!("{base}/rooms/project/messages"))
        .header("x-admin-token", "MASTER")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(repaired.status(), reqwest::StatusCode::CREATED);
    let counts: (u64, u64, u64) = sql
        .query_row(
            "SELECT
                (SELECT count(*) FROM messages),
                (SELECT count(*) FROM attentions),
                (SELECT count(*) FROM care_outbox)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(counts, (1, 1, 1));
}

#[tokio::test]
async fn caretaker_claims_source_attention_without_source_room_access() {
    let (port, _guard) = spawn_server_env(
        "MASTER",
        &[
            ("REQUIRE_INVITE", "1".into()),
            ("REQUIRE_SESSIONS", "1".into()),
            ("LOCA_AGENT_ROOM", "iye".into()),
            ("RESERVED_LOCA", "iye".into()),
            ("LOCA_CARETAKERS", "loca-care".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let care_davet = davet_for(&base, "MASTER", "iye", "loca-care").await;
    let care_session = session_with(
        &base,
        ("x-room-token", care_davet.as_str()),
        "loca-care",
        Some("iye"),
    )
    .await;
    let operator_davet = davet_for(&base, "MASTER", "project", "operator").await;
    let operator_session = session_with(
        &base,
        ("x-room-token", operator_davet.as_str()),
        "operator",
        Some("project"),
    )
    .await;
    let admin_session = session_with(
        &base,
        ("x-admin-token", "MASTER"),
        "operator",
        Some("project"),
    )
    .await;
    report_ready_runtime(&client, &base, "MASTER", "loca-care").await;
    let care_url = format!(
        "ws://127.0.0.1:{port}/ws?room=iye&name=loca-care&type=agent\
         &filter=mentions&turn_max=1&session={care_session}"
    );
    let (mut care_ws, _) = tokio_tungstenite::connect_async(care_url).await.unwrap();

    client
        .post(format!("{base}/rooms/project/messages"))
        .header("x-session-token", &operator_session)
        .json(&serde_json::json!({
            "sender": "operator", "sender_type": "user",
            "target": "loca-care", "text": "@loca-care inspect this"
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let care = wait_for(&mut care_ws, |value| {
        value["t"] == "care" && value["signal"]["room"] == "iye"
    })
    .await;
    // Re-homed onto Iye for delivery; origin preserved in source_room.
    assert_eq!(care["signal"]["source_room"], "project");
    let attention_id = care["signal"]["attention_id"].as_str().unwrap();

    let source_read = client
        .get(format!("{base}/rooms/project/attentions"))
        .header("x-session-token", &care_session)
        .send()
        .await
        .unwrap();
    assert_eq!(
        source_read.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "caretaker ownership must not open source-room reads"
    );

    let claimed: Value = client
        .post(format!(
            "{base}/rooms/project/attentions/{attention_id}/claim"
        ))
        .header("x-session-token", &care_session)
        .json(&serde_json::json!({ "by": "spoofed-name" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(claimed["status"], "claimed");
    assert_eq!(claimed["claimed_by"], "loca-care");

    let resolved: Value = client
        .post(format!(
            "{base}/rooms/project/attentions/{attention_id}/resolve"
        ))
        .header("x-session-token", &care_session)
        .json(&serde_json::json!({ "by": "spoofed-name" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resolved["status"], "resolved");

    let outsider_davet = davet_for(&base, "MASTER", "other", "outsider").await;
    let outsider_session = session_with(
        &base,
        ("x-room-token", outsider_davet.as_str()),
        "outsider",
        Some("other"),
    )
    .await;
    let outsider_attention: Value = client
        .post(format!("{base}/rooms/project/attentions"))
        .header("x-session-token", &admin_session)
        .json(&serde_json::json!({
            "subject": "private source work",
            "audience": {"kind": "person", "name": "outsider"},
            "by": "operator"
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let denied = client
        .post(format!(
            "{base}/rooms/project/attentions/{}/claim",
            outsider_attention["id"].as_str().unwrap()
        ))
        .header("x-session-token", &outsider_session)
        .json(&serde_json::json!({ "by": "outsider" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        denied.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "only the configured caretaker gets the bounded cross-loca owner exception"
    );
}

#[tokio::test]
async fn attention_has_one_owner_and_delivery_ack_is_not_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("attention.sqlite");
    let db_path = db.to_string_lossy().to_string();
    let (port, guard) = spawn_server_env("MASTER", &[("DB_PATH", db_path.clone())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let lead = client
        .post(format!("{base}/rooms/proj/lead"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "lead": "lead-engineer" }))
        .send()
        .await
        .unwrap();
    assert_eq!(lead.status(), 200);
    let mut ws = connect_ws(port, "proj", "operator", "user").await;

    let attention: Value = client
        .post(format!("{base}/rooms/proj/attentions"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({
            "subject": "review the public release",
            "audience": { "kind": "lead" },
            "by": "operator"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = attention["id"].as_str().unwrap();
    let care = wait_for(&mut ws, |value| {
        value["t"] == "care" && value["signal"]["attention_id"] == id
    })
    .await;
    let delivery_id = care["signal"]["id"].as_str().unwrap();
    assert_eq!(attention["owner"], "lead-engineer");
    assert_eq!(attention["status"], "open");

    let wrong = client
        .post(format!("{base}/rooms/proj/attentions/{id}/claim"))
        .json(&serde_json::json!({ "by": "someone-else" }))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 403);
    let claimed: Value = client
        .post(format!("{base}/rooms/proj/attentions/{id}/claim"))
        .json(&serde_json::json!({ "by": "lead-engineer" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(claimed["status"], "claimed");

    let ack = client
        .post(format!("{base}/rooms/proj/care/{delivery_id}/ack"))
        .json(&serde_json::json!({ "by": "lead-engineer" }))
        .send()
        .await
        .unwrap();
    assert_eq!(ack.status(), 204);
    let after_ack: Value = client
        .get(format!("{base}/rooms/proj/attentions"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(after_ack[0]["status"], "claimed");
    assert!(after_ack[0]["delivered_at"].as_u64().is_some());

    drop(guard);
    let (restart_port, _restart_guard) = spawn_server_env("MASTER", &[("DB_PATH", db_path)]).await;
    let restart_base = format!("http://127.0.0.1:{restart_port}");
    let restored: Value = client
        .get(format!("{restart_base}/rooms/proj/attentions"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(restored[0]["id"], id);
    assert_eq!(restored[0]["status"], "claimed");

    let resolved: Value = client
        .post(format!("{restart_base}/rooms/proj/attentions/{id}/resolve"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "by": "operator" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resolved["status"], "resolved");
}

#[tokio::test]
async fn reminders_cannot_be_enabled_for_a_missing_room_lead() {
    let (port, _guard) = spawn_server_with("buyuk").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let rejected = client
        .put(format!("{base}/rooms/proj/settings"))
        .header("x-admin-token", "buyuk")
        .json(&serde_json::json!({
            "care_recipient": { "kind": "lead" },
            "care_goal_secs": 60
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), reqwest::StatusCode::BAD_REQUEST);
    assert!(rejected
        .text()
        .await
        .unwrap()
        .contains("select a room lead"));

    let settings: Value = client
        .get(format!("{base}/rooms/proj/settings"))
        .header("x-admin-token", "buyuk")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(settings["care_goal_secs"], 0);
}

#[tokio::test]
async fn explicit_wait_cycle_wakes_only_the_live_lead_with_bounded_context() {
    let (port, _guard) = spawn_server_with("buyuk").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    report_ready_runtime(&client, &base, "buyuk", "lead").await;
    client
        .post(format!("{base}/rooms/proj/lead"))
        .header("x-admin-token", "buyuk")
        .json(&serde_json::json!({ "lead": "lead" }))
        .send()
        .await
        .unwrap();
    client
        .put(format!("{base}/rooms/proj/settings"))
        .header("x-admin-token", "buyuk")
        .json(&serde_json::json!({
            "care_context_messages": 2,
            "care_wait_secs": 600
        }))
        .send()
        .await
        .unwrap();
    let url = format!(
        "ws://127.0.0.1:{port}/ws?room=proj&name=lead&type=agent\
         &filter=mentions&turn_max=1&admin=buyuk"
    );
    let (mut lead_ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    for text in ["old", "relevant one", "relevant two"] {
        client
            .post(format!("{base}/rooms/proj/messages"))
            .header("x-admin-token", "buyuk")
            .json(&serde_json::json!({
                "sender": "operator", "sender_type": "user", "text": text
            }))
            .send()
            .await
            .unwrap();
        // As room lead, this socket sees all normal messages. Drain each so
        // the assertion below observes the care event itself.
        let _ = wait_for(&mut lead_ws, |value| {
            value["t"] == "msg" && value["message"]["text"] == text
        })
        .await;
    }
    for (waiter, waiting_for) in [("a", "b"), ("b", "a")] {
        let response = client
            .post(format!("{base}/rooms/proj/waits"))
            .json(&serde_json::json!({
                "by": waiter, "waiting_for": waiting_for,
                "reason": "need the other contract"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 201);
    }
    let care = wait_for(&mut lead_ws, |value| value["t"] == "care").await;
    assert_eq!(care["signal"]["reason"], "wait_cycle");
    assert_eq!(care["signal"]["owner"], "lead");
    assert_eq!(care["signal"]["context"].as_array().unwrap().len(), 2);
    assert_eq!(care["signal"]["context"][0]["text"], "relevant one");
    assert_eq!(care["signal"]["context"][1]["text"], "relevant two");

    let self_wait = client
        .post(format!("{base}/rooms/proj/waits"))
        .json(&serde_json::json!({
            "by": "a", "waiting_for": "a", "reason": "invalid"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(self_wait.status(), 400);
}

#[tokio::test]
async fn loca_care_owns_the_signal_in_iye_when_no_live_lead_exists() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("care.db").to_string_lossy().to_string();
    let (port, _guard) = spawn_server_env(
        "buyuk",
        &[
            ("LOCA_AGENT_ROOM", "iye".into()),
            ("LOCA_CARETAKERS", "loca-dev,loca-care".into()),
            ("DB_PATH", db),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    report_ready_runtime(&client, &base, "buyuk", "loca-care").await;
    let care_url = format!(
        "ws://127.0.0.1:{port}/ws?room=iye&name=loca-care&type=agent\
         &filter=mentions&turn_max=1&admin=buyuk"
    );
    let (mut care_ws, _) = tokio_tungstenite::connect_async(care_url.clone())
        .await
        .unwrap();

    for (waiter, waiting_for) in [("worker", "reviewer"), ("reviewer", "worker")] {
        let response = client
            .post(format!("{base}/rooms/proj/waits"))
            .json(&serde_json::json!({
                "by": waiter, "waiting_for": waiting_for,
                "reason": "each side needs the other"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 201);
    }
    let care = wait_for(&mut care_ws, |value| value["t"] == "care").await;
    // Delivered in Iye (the caretaker's home loca and this socket's room); the
    // source loca travels in source_room, never as signal.room on this socket.
    assert_eq!(care["signal"]["room"], "iye");
    assert_eq!(care["signal"]["source_room"], "proj");
    assert_eq!(care["signal"]["owner"], "loca-care");
    assert_eq!(care["signal"]["reason"], "wait_cycle");
    assert!(
        care["signal"]["context"].as_array().unwrap().is_empty(),
        "the source room had no messages, and its full history was not opened"
    );
    let signal_id = care["signal"]["id"].as_str().unwrap().to_string();

    // No ACK yet: reconnect replays the same durable outbox id exactly.
    drop(care_ws);
    let (mut replay_ws, _) = tokio_tungstenite::connect_async(care_url.clone())
        .await
        .unwrap();
    let replay = wait_for(&mut replay_ws, |value| value["t"] == "care").await;
    assert_eq!(replay["signal"]["id"], signal_id);
    let ack = client
        .post(format!("{base}/rooms/iye/care/{signal_id}/ack"))
        .header("x-admin-token", "buyuk")
        .json(&serde_json::json!({ "by": "loca-care" }))
        .send()
        .await
        .unwrap();
    assert_eq!(ack.status(), 204);
    drop(replay_ws);
    let (mut clean_ws, _) = tokio_tungstenite::connect_async(care_url).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(150), clean_ws.next())
            .await
            .is_err(),
        "ACKed care signal replayed again"
    );
}

#[tokio::test]
async fn failed_lead_runtime_is_not_treated_as_healthy_presence() {
    let (port, _guard) = spawn_server_env(
        "buyuk",
        &[
            ("LOCA_AGENT_ROOM", "iye".into()),
            ("LOCA_CARETAKERS", "loca-care".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let lead_token = report_ready_runtime(&client, &base, "buyuk", "lead").await;
    report_ready_runtime(&client, &base, "buyuk", "loca-care").await;
    client
        .post(format!("{base}/runtime/health"))
        .header("x-room-token", lead_token)
        .json(&serde_json::json!({
            "wake": "FAILED", "ack": "PENDING", "delivery_id": "proj:7",
            "attention_id": "attention:lead:proj:7", "stored": true,
            "accepted": true, "first_response": false,
            "final_response": false, "turn_completed": false
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    client
        .post(format!("{base}/rooms/proj/lead"))
        .header("x-admin-token", "buyuk")
        .json(&serde_json::json!({ "lead": "lead" }))
        .send()
        .await
        .unwrap();
    let lead_url = format!("ws://127.0.0.1:{port}/ws?room=proj&name=lead&type=agent&admin=buyuk");
    let (_lead_ws, _) = tokio_tungstenite::connect_async(lead_url).await.unwrap();
    let care_url = format!(
        "ws://127.0.0.1:{port}/ws?room=iye&name=loca-care&type=agent&filter=mentions&admin=buyuk"
    );
    let (mut care_ws, _) = tokio_tungstenite::connect_async(care_url).await.unwrap();
    for (waiter, waiting_for) in [("a", "b"), ("b", "a")] {
        client
            .post(format!("{base}/rooms/proj/waits"))
            .json(&serde_json::json!({
                "by": waiter, "waiting_for": waiting_for, "reason": "cycle"
            }))
            .send()
            .await
            .unwrap();
    }
    let care = wait_for(&mut care_ws, |value| value["t"] == "care").await;
    assert_eq!(care["signal"]["owner"], "loca-care");

    let residents: Vec<Value> = client
        .get(format!("{base}/residents"))
        .header("x-admin-token", "buyuk")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let lead = residents
        .iter()
        .find(|value| value["name"] == "lead")
        .unwrap();
    assert_eq!(lead["online"], true, "transport is still honestly online");
    assert_eq!(lead["runtime"]["ready"], false, "wake health is separate");
    assert_eq!(lead["runtime"]["attention_id"], "attention:lead:proj:7");
    assert_eq!(lead["runtime"]["accepted"], true);
    assert_eq!(lead["runtime"]["final_response"], false);
}

#[tokio::test]
async fn configured_task_goal_and_silence_checks_emit_distinct_care_reasons() {
    let (port, _guard) = spawn_server_with("buyuk").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    report_ready_runtime(&client, &base, "buyuk", "lead").await;
    client
        .post(format!("{base}/rooms/proj/lead"))
        .header("x-admin-token", "buyuk")
        .json(&serde_json::json!({ "lead": "lead" }))
        .send()
        .await
        .unwrap();
    client
        .put(format!("{base}/rooms/proj/settings"))
        .header("x-admin-token", "buyuk")
        .json(&serde_json::json!({
            "care_task_secs": 1,
            "care_goal_secs": 1,
            "care_silence_secs": 0,
            "care_cooldown_secs": 30,
            "care_max_attempts": 1
        }))
        .send()
        .await
        .unwrap();
    let url = format!(
        "ws://127.0.0.1:{port}/ws?room=proj&name=lead&type=agent\
         &filter=mentions&turn_max=1&admin=buyuk"
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();

    let task: Value = client
        .post(format!("{base}/rooms/proj/tasks"))
        .header("x-admin-token", "buyuk")
        .json(&serde_json::json!({
            "title": "ship adapter", "by": "operator", "assigned_to": "worker"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let task_care = wait_for(&mut ws, |value| {
        value["t"] == "care" && value["signal"]["reason"] == "task_reminder"
    })
    .await;
    assert_eq!(task_care["signal"]["target"], "worker");
    client
        .patch(format!(
            "{base}/rooms/proj/tasks/{}",
            task["id"].as_u64().unwrap()
        ))
        .header("x-admin-token", "buyuk")
        .json(&serde_json::json!({ "status": "cancelled", "by": "operator" }))
        .send()
        .await
        .unwrap();

    let goal: Value = client
        .post(format!("{base}/rooms/proj/goals"))
        .header("x-admin-token", "buyuk")
        .json(&serde_json::json!({
            "outcome": "release announced", "by": "operator"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let goal_care = wait_for(&mut ws, |value| {
        value["t"] == "care" && value["signal"]["reason"] == "goal_reminder"
    })
    .await;
    assert!(goal_care["signal"]["subject"]
        .as_str()
        .unwrap()
        .contains("release announced"));
    client
        .patch(format!(
            "{base}/rooms/proj/goals/{}",
            goal["id"].as_u64().unwrap()
        ))
        .header("x-admin-token", "buyuk")
        .json(&serde_json::json!({
            "status": "achieved", "by": "operator"
        }))
        .send()
        .await
        .unwrap();

    client
        .put(format!("{base}/rooms/proj/settings"))
        .header("x-admin-token", "buyuk")
        .json(&serde_json::json!({
            "care_task_secs": 0,
            "care_goal_secs": 0,
            "care_silence_secs": 1
        }))
        .send()
        .await
        .unwrap();
    client
        .post(format!("{base}/rooms/proj/messages"))
        .header("x-admin-token", "buyuk")
        .json(&serde_json::json!({
            "sender": "operator", "sender_type": "user", "text": "pause here"
        }))
        .send()
        .await
        .unwrap();
    let _ = wait_for(&mut ws, |value| {
        value["t"] == "msg" && value["message"]["text"] == "pause here"
    })
    .await;
    let silence = wait_for(&mut ws, |value| {
        value["t"] == "care" && value["signal"]["reason"] == "room_silence"
    })
    .await;
    assert_eq!(
        silence["signal"]["subject"],
        "operator-enabled room silence check"
    );
}

/// A direct goal PATCH and its care reset are also one logical write. If the
/// reset fails, neither the durable goal nor the in-memory goal may advance.
#[tokio::test]
async fn direct_goal_progress_and_care_reset_commit_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir
        .path()
        .join("atomic-direct-goal.db")
        .to_string_lossy()
        .to_string();
    let (port, _guard) = spawn_server_env("MASTER", &[("DB_PATH", db.clone())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    client
        .post(format!("{base}/rooms/proj/lead"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "lead": "operator" }))
        .send()
        .await
        .unwrap();
    let goal: Value = client
        .post(format!("{base}/rooms/proj/goals"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({
            "outcome": "before", "completion": "manual", "by": "operator"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let goal_id = goal["id"].as_u64().unwrap();
    let goal_progress = goal["progress_at"].as_u64().unwrap();
    let sql = rusqlite::Connection::open(&db).unwrap();
    sql.execute(
        "INSERT INTO care_marks (room, signal_key, last_signal_at, signal_count)
         VALUES ('proj', ?1, 1, 1)",
        [format!("goal:{goal_id}")],
    )
    .unwrap();
    sql.execute_batch(
        "CREATE TRIGGER reject_goal_care_reset
         BEFORE DELETE ON care_marks
         BEGIN SELECT RAISE(FAIL, 'injected care reset failure'); END;",
    )
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;

    let failed = client
        .patch(format!("{base}/rooms/proj/goals/{goal_id}"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "outcome": "after", "by": "operator" }))
        .send()
        .await
        .unwrap();
    assert_eq!(failed.status(), 503);
    let goals: Value = client
        .get(format!("{base}/rooms/proj/goals"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(goals[0]["outcome"], "before");
    assert_eq!(goals[0]["progress_at"], goal_progress);
    let durable: (String, u64) = sql
        .query_row(
            "SELECT outcome, progress_at FROM goals WHERE room = 'proj' AND id = ?1",
            [goal_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(durable, ("before".into(), goal_progress));
    assert_eq!(
        sql.query_row(
            "SELECT COUNT(*) FROM care_marks WHERE room = 'proj' AND signal_key = ?1",
            [format!("goal:{goal_id}")],
            |row| row.get::<_, u64>(0),
        )
        .unwrap(),
        1
    );

    sql.execute_batch("DROP TRIGGER reject_goal_care_reset")
        .unwrap();
    let repaired: Value = client
        .patch(format!("{base}/rooms/proj/goals/{goal_id}"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "outcome": "after", "by": "operator" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(repaired["outcome"], "after");
    assert!(repaired["progress_at"].as_u64().unwrap() > goal_progress);
    assert_eq!(
        sql.query_row(
            "SELECT COUNT(*) FROM care_marks WHERE room = 'proj' AND signal_key = ?1",
            [format!("goal:{goal_id}")],
            |row| row.get::<_, u64>(0),
        )
        .unwrap(),
        0
    );
}

/// A configured caretaker can inspect Building presence with its own
/// least-privilege membership. Ordinary members cannot use that operational
/// view, and the response never contains credentials.
#[tokio::test]
async fn caretaker_can_audit_residents_without_master_authority() {
    let (port, _g) = spawn_server_env("MASTER", &[]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let admit = |name: &'static str| {
        let client = client.clone();
        let base = base.clone();
        async move {
            client
                .post(format!("{base}/members"))
                .header("x-admin-token", "MASTER")
                .json(&serde_json::json!({ "name": name, "kind": "agent" }))
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()
        }
    };
    let caretaker = admit("loca-care").await;
    let worker = admit("worker").await;

    let denied = client
        .get(format!("{base}/care/residents"))
        .header("x-room-token", worker["token"].as_str().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), reqwest::StatusCode::FORBIDDEN);

    let response = client
        .get(format!("{base}/care/residents"))
        .header("x-room-token", caretaker["token"].as_str().unwrap())
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let bytes = response.bytes().await.unwrap();
    let residents: Vec<Value> = serde_json::from_slice(&bytes).unwrap();
    assert!(residents.iter().any(|row| row["name"] == "loca-care"));
    assert!(residents.iter().any(|row| row["name"] == "worker"));
    assert!(
        !String::from_utf8_lossy(&bytes).contains("mb_"),
        "presence reports must never return membership credentials"
    );
}

/// (1) POS-live: a live waiter is woken exactly once by a direct reply.
#[tokio::test]
async fn wait_reply_wakes_the_live_waiter_exactly_once() {
    let (port, _guard) = spawn_server().await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let created = declare_wait(&client, &base, "proj", "A", "B").await;
    let since0 = created["since"].as_u64().unwrap();

    let mut a = connect_agent_mentions(port, "proj", "A").await;
    post_direct(
        &client,
        &base,
        "proj",
        "B",
        Some("A"),
        "here is what you waited for",
    )
    .await;

    let care = wait_for(&mut a, |v| v["t"] == "care").await;
    assert_eq!(care["signal"]["reason"], "wait_replied");
    assert_eq!(care["signal"]["owner"], "A");
    assert_eq!(care["signal"]["target"], "A");
    let participants: Vec<&str> = care["signal"]["participants"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p.as_str())
        .collect();
    assert!(participants.contains(&"A") && participants.contains(&"B"));
    assert_eq!(
        care["signal"]["context"][0]["text"],
        "here is what you waited for"
    );
    let care_id = care["signal"]["id"].as_str().unwrap().to_string();

    // Exactly once: no second frame carrying the same delivery id.
    assert!(
        !saw_care_within(&mut a, 300, |v| v["signal"]["id"] == care_id.as_str()).await,
        "the live wake must be delivered exactly once"
    );

    // Progress, not completion: the edge stays, its generation advanced, its
    // overdue counter reset.
    let waits = get_json(&client, format!("{base}/rooms/proj/waits")).await;
    assert_eq!(waits.len(), 1);
    assert_eq!(waits[0]["waiter"], "A");
    assert_eq!(waits[0]["waiting_for"], "B");
    assert!(waits[0]["since"].as_u64().unwrap() > since0);
    assert_eq!(waits[0]["signal_count"], 0);
}

/// (2) POS-offline: the wake is durable and replays the same id on reconnect.
#[tokio::test]
async fn wait_reply_is_durable_for_an_offline_waiter() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir
        .path()
        .join("wait-reply.db")
        .to_string_lossy()
        .to_string();
    let (port, _guard) = spawn_server_env("", &[("DB_PATH", db)]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    declare_wait(&client, &base, "proj", "A", "B").await;
    // A is offline; the wake must be durably queued, not lost.
    post_direct(
        &client,
        &base,
        "proj",
        "B",
        Some("A"),
        "answer while you were away",
    )
    .await;

    let attentions = get_json(&client, format!("{base}/rooms/proj/attentions")).await;
    let wake_atts: Vec<&Value> = attentions
        .iter()
        .filter(|a| a["reason"] == "wait_replied")
        .collect();
    assert_eq!(wake_atts.len(), 1);
    assert_eq!(wake_atts[0]["owner"], "A");
    assert_eq!(wake_atts[0]["status"], "open");

    let mut a = connect_agent_mentions(port, "proj", "A").await;
    let care = wait_for(&mut a, |v| {
        v["t"] == "care" && v["signal"]["reason"] == "wait_replied"
    })
    .await;
    let delivery_id = care["signal"]["id"].as_str().unwrap().to_string();

    // Drop without ACK, reconnect: the exact same delivery id replays.
    drop(a);
    let mut a2 = connect_agent_mentions(port, "proj", "A").await;
    let replay = wait_for(&mut a2, |v| {
        v["t"] == "care" && v["signal"]["reason"] == "wait_replied"
    })
    .await;
    assert_eq!(replay["signal"]["id"], delivery_id);
}

/// (3) NEG wall-post: neither an untargeted post nor an @all broadcast wakes a
/// waiter, and the wait is left completely untouched.
#[tokio::test]
async fn wall_posts_do_not_wake_a_waiter() {
    let (port, _guard) = spawn_server().await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let created = declare_wait(&client, &base, "proj", "A", "B").await;
    let since0 = created["since"].as_u64().unwrap();

    let mut a = connect_agent_mentions(port, "proj", "A").await;
    post_direct(&client, &base, "proj", "B", None, "thinking out loud").await;
    post_direct(
        &client,
        &base,
        "proj",
        "B",
        Some("all"),
        "announcement to everyone",
    )
    .await;

    assert!(
        !saw_care_within(&mut a, 300, |_| true).await,
        "a wall post must not wake a waiter"
    );

    let waits = get_json(&client, format!("{base}/rooms/proj/waits")).await;
    assert_eq!(waits.len(), 1);
    assert_eq!(waits[0]["waiter"], "A");
    assert_eq!(waits[0]["since"].as_u64().unwrap(), since0);
    assert_eq!(waits[0]["signal_count"], 0);
}

/// (4) reconnect-dedup: exactly one durable, unacked wake row is owed, and it
/// replays at most once per reconnect.
#[tokio::test]
async fn wait_reply_wake_is_at_most_once_across_reconnects() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir
        .path()
        .join("wait-reply-dedup.db")
        .to_string_lossy()
        .to_string();
    let (port, _guard) = spawn_server_env("", &[("DB_PATH", db.clone())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    declare_wait(&client, &base, "proj", "A", "B").await;
    post_direct(&client, &base, "proj", "B", Some("A"), "reply once").await;

    let sql = rusqlite::Connection::open(&db).unwrap();
    let unacked = |sql: &rusqlite::Connection| -> u64 {
        sql.query_row(
            "SELECT count(*) FROM care_outbox WHERE owner = 'A' AND acked_at IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap()
    };
    assert_eq!(unacked(&sql), 1, "exactly one durable wake is owed to A");

    let mut a = connect_agent_mentions(port, "proj", "A").await;
    let care = wait_for(&mut a, |v| {
        v["t"] == "care" && v["signal"]["reason"] == "wait_replied"
    })
    .await;
    let care_id = care["signal"]["id"].as_str().unwrap().to_string();

    drop(a);
    let mut a2 = connect_agent_mentions(port, "proj", "A").await;
    let replay = wait_for(&mut a2, |v| v["t"] == "care").await;
    assert_eq!(replay["signal"]["id"], care_id);
    assert!(
        !saw_care_within(&mut a2, 300, |v| v["signal"]["id"] == care_id.as_str()).await,
        "the durable wake must replay at most once per reconnect"
    );
    assert_eq!(
        unacked(&sql),
        1,
        "replay must not multiply the durable outbox row"
    );
}

/// (5) no-self-wake: the sender of a reply is never woken by its own reply.
#[tokio::test]
async fn a_direct_reply_never_wakes_its_own_sender() {
    let (port, _guard) = spawn_server().await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // Only A waits for B; B waits for nobody.
    declare_wait(&client, &base, "proj", "A", "B").await;

    let mut b = connect_agent_mentions(port, "proj", "B").await;
    post_direct(&client, &base, "proj", "B", Some("A"), "here you go").await;

    assert!(
        !saw_care_within(&mut b, 300, |v| v["signal"]["owner"] == "B").await,
        "the sender of a reply must never wake itself"
    );

    // The wake did fire — but only for A, and no attention is ever owned by B.
    let attentions = get_json(&client, format!("{base}/rooms/proj/attentions")).await;
    assert!(
        attentions
            .iter()
            .any(|a| a["reason"] == "wait_replied" && a["owner"] == "A"),
        "the addressed waiter A must still be woken"
    );
    assert!(
        !attentions.iter().any(|a| a["owner"] == "B"),
        "no attention may be owned by the reply's sender"
    );
}

/// (6) generation-interplay: a reply retires the current overdue generation and
/// re-arms a fresh one, so a later overdue is a NEW attention.
#[tokio::test]
async fn wait_reply_resets_the_overdue_generation() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir
        .path()
        .join("wait-reply-gen.db")
        .to_string_lossy()
        .to_string();
    let (port, _guard) = spawn_server_env(
        "buyuk",
        &[("DB_PATH", db.clone()), ("CARE_WAIT_SECS", "1".into())],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // A live, healthy lead owns overdue reminders in this room.
    report_ready_runtime(&client, &base, "buyuk", "L").await;
    client
        .post(format!("{base}/rooms/proj/lead"))
        .header("x-admin-token", "buyuk")
        .json(&serde_json::json!({ "lead": "L" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let lead_url = format!(
        "ws://127.0.0.1:{port}/ws?room=proj&name=L&type=agent&filter=mentions&turn_max=1&admin=buyuk"
    );
    let (mut lead, _) = tokio_tungstenite::connect_async(lead_url).await.unwrap();

    let created = declare_wait(&client, &base, "proj", "A", "B").await;
    let g1_since = created["since"].as_u64().unwrap();

    // Past the 1s threshold: the lead is nudged about A's overdue wait (G1).
    let overdue = wait_for(&mut lead, |v| {
        v["t"] == "care" && v["signal"]["reason"] == "wait_overdue"
    })
    .await;
    let att_g1 = overdue["signal"]["attention_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(att_g1, format!("attention:proj:wait:A:{g1_since}"));

    // B replies directly to A: A is re-woken and A's G1 overdue is retired.
    let mut a = connect_agent_mentions(port, "proj", "A").await;
    post_direct(&client, &base, "proj", "B", Some("A"), "unblocking you").await;
    let _ = wait_for(&mut a, |v| {
        v["t"] == "care" && v["signal"]["reason"] == "wait_replied"
    })
    .await;

    // The G1 overdue is resolved and its durable delivery acked.
    let attentions = get_json(&client, format!("{base}/rooms/proj/attentions")).await;
    let g1 = attentions
        .iter()
        .find(|a| a["id"] == att_g1.as_str())
        .expect("the G1 overdue attention exists");
    assert_eq!(g1["status"], "resolved");
    let acked_open: u64 = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row(
            "SELECT count(*) FROM care_outbox WHERE attention_id = ?1 AND acked_at IS NULL",
            [&att_g1],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(acked_open, 0, "the G1 overdue outbox row must be acked");

    // No re-fire of the retired G1 overdue to the lead.
    assert!(
        !saw_care_within(&mut lead, 400, |v| v["signal"]["attention_id"]
            == att_g1.as_str())
        .await,
        "the retired G1 overdue must not fire again"
    );

    // The wait advanced to a NEW generation with a reset counter.
    let waits = get_json(&client, format!("{base}/rooms/proj/waits")).await;
    let g2_since = waits[0]["since"].as_u64().unwrap();
    assert!(g2_since > g1_since);
    assert_eq!(waits[0]["signal_count"], 0);

    // Past the threshold again: a fresh overdue under the NEW generation.
    let overdue2 = wait_for(&mut lead, |v| {
        v["t"] == "care" && v["signal"]["reason"] == "wait_overdue"
    })
    .await;
    let att_g2 = overdue2["signal"]["attention_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(att_g2, att_g1);
    assert_eq!(att_g2, format!("attention:proj:wait:A:{g2_since}"));
}

/// (7) lifecycle: the reply never completes the wait, and once the edge is
/// deleted a later reply resurrects nothing.
#[tokio::test]
async fn wait_reply_does_not_complete_or_resurrect_the_wait() {
    let (port, _guard) = spawn_server().await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    declare_wait(&client, &base, "proj", "A", "B").await;
    let mut a = connect_agent_mentions(port, "proj", "A").await;
    post_direct(&client, &base, "proj", "B", Some("A"), "answered").await;
    let _ = wait_for(&mut a, |v| {
        v["t"] == "care" && v["signal"]["reason"] == "wait_replied"
    })
    .await;

    // The wait still exists — a reply is progress, not completion.
    let waits = get_json(&client, format!("{base}/rooms/proj/waits")).await;
    assert_eq!(waits.len(), 1);
    assert_eq!(waits[0]["waiter"], "A");
    assert_eq!(waits[0]["waiting_for"], "B");

    // Delete the edge explicitly.
    let deleted = client
        .delete(format!("{base}/rooms/proj/waits/A"))
        .json(&serde_json::json!({ "by": "A" }))
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), 204);
    let waits = get_json(&client, format!("{base}/rooms/proj/waits")).await;
    assert!(waits.is_empty());

    // A later direct reply must not resurrect a wake: there is no edge.
    post_direct(&client, &base, "proj", "B", Some("A"), "answered again").await;
    assert!(
        !saw_care_within(&mut a, 300, |_| true).await,
        "a deleted wait must not be resurrected by a later reply"
    );
}

#[tokio::test]
async fn a_reply_to_a_caretaker_summons_them_cross_loca() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir
        .path()
        .join("reply-summon.db")
        .to_string_lossy()
        .to_string();
    let (port, _guard) = spawn_server_env(
        "",
        &[
            ("LOCA_AGENT_ROOM", "iye".into()),
            ("RESERVED_LOCA", "iye".into()),
            ("LOCA_CARETAKERS", "loca-dev,loca-care".into()),
            ("DB_PATH", db),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let (mut caretaker, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/ws?room=iye&name=loca-dev&type=agent&filter=mentions&turn_max=1"
    ))
    .await
    .unwrap();
    let client = reqwest::Client::new();

    // loca-dev authored a message in the source loca `reviewer`.
    let root: Value = client
        .post(format!("{base}/rooms/reviewer/messages"))
        .json(&serde_json::json!({
            "sender": "loca-dev", "sender_type": "agent", "text": "the note"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let root_id = root["id"].as_u64().unwrap();

    // A reply to loca-dev's message with NO explicit target and NO @loca-dev in
    // text. The reply author (loca-dev) is addressed via reply_to_sender, so the
    // cross-loca caretaker summon must still fire — exactly like an @mention.
    client
        .post(format!("{base}/rooms/reviewer/messages"))
        .json(&serde_json::json!({
            "sender": "reviewer1", "sender_type": "agent",
            "text": "answering the note", "reply_to": root_id
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    let summoned = wait_for(&mut caretaker, |v| {
        v["t"] == "care"
            && v["signal"]["reason"] == "direct_summon"
            && v["signal"]["context"][0]["text"] == "answering the note"
    })
    .await;
    assert_eq!(summoned["signal"]["room"], "iye");
    assert_eq!(summoned["signal"]["source_room"], "reviewer");
    assert_eq!(summoned["signal"]["owner"], "loca-dev");
    assert_eq!(summoned["signal"]["context"].as_array().unwrap().len(), 1);
}

/// Count room-silence Care frames delivered to `ws` within `ms` ms.
async fn count_silence_care(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    ms: u64,
) -> usize {
    let mut count = 0usize;
    let deadline = tokio::time::sleep(Duration::from_millis(ms));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => break,
            item = ws.next() => match item {
                Some(Ok(WsMessage::Text(text))) => {
                    if let Ok(value) = serde_json::from_str::<Value>(&text) {
                        if value["t"] == "care"
                            && value["signal"]["reason"] == "room_silence"
                        {
                            count += 1;
                        }
                    }
                }
                _ => break,
            },
        }
    }
    count
}

#[tokio::test]
async fn everyone_reminder_wakes_each_member_exactly_once_over_websockets() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir
        .path()
        .join("everyone-ws.sqlite")
        .to_string_lossy()
        .to_string();
    // Silence fires after 1s; a long cooldown means exactly ONE sweep in the test
    // window, so per-socket Care counts are deterministic — the pre-fix N×N group
    // broadcast delivered one-per-member to EVERY socket (count 3), the fix
    // delivers each member only its own (count 1).
    let (port, _guard) = spawn_server_env(
        "MASTER",
        &[
            ("DB_PATH", db),
            ("CARE_SILENCE_SECS", "1".into()),
            ("CARE_COOLDOWN_SECS", "60".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // Three real members, each with a davet and a principal-bound session in proj.
    let mut session = std::collections::HashMap::new();
    for name in ["alice", "bob", "carol"] {
        let davet = davet_for(&base, "MASTER", "proj", name).await;
        session.insert(
            name,
            session_with(&base, ("x-room-token", davet.as_str()), name, Some("proj")).await,
        );
    }

    // The reminder recipient is Everyone.
    client
        .put(format!("{base}/rooms/proj/settings"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "care_recipient": { "kind": "all" } }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    let ws_url = |name: &str| {
        format!(
            "ws://127.0.0.1:{port}/ws?room=proj&name={name}&type=agent\
             &filter=mentions&turn_max=1&session={}",
            session[name]
        )
    };
    let (mut alice, _) = tokio_tungstenite::connect_async(ws_url("alice"))
        .await
        .unwrap();
    let (mut bob, _) = tokio_tungstenite::connect_async(ws_url("bob"))
        .await
        .unwrap();

    // One message sets last_msg_ms; then the room goes quiet and silence elapses.
    client
        .post(format!("{base}/rooms/proj/messages"))
        .header("x-session-token", &session["alice"])
        .json(&serde_json::json!({ "sender": "alice", "sender_type": "agent", "text": "hi" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    // Each online member is woken EXACTLY ONCE — never once-per-member.
    assert_eq!(
        count_silence_care(&mut alice, 4000).await,
        1,
        "alice woken exactly once, not once per member (no N×N)"
    );
    assert_eq!(
        count_silence_care(&mut bob, 800).await,
        1,
        "bob woken exactly once, not once per member (no N×N)"
    );

    // carol was offline during the fan-out; her per-member reminder is durable and
    // delivered exactly once on her first reconnect (principal-scoped replay).
    let (mut carol, _) = tokio_tungstenite::connect_async(ws_url("carol"))
        .await
        .unwrap();
    assert_eq!(
        count_silence_care(&mut carol, 2000).await,
        1,
        "offline carol gets exactly one on reconnect"
    );
}

/// Return the id of the first room-silence Care signal delivered to `ws` within
/// `ms` ms — the delivery id the client would POST back to ACK.
async fn next_silence_signal(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    ms: u64,
) -> Option<String> {
    let deadline = tokio::time::sleep(Duration::from_millis(ms));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => return None,
            item = ws.next() => match item {
                Some(Ok(WsMessage::Text(text))) => {
                    if let Ok(value) = serde_json::from_str::<Value>(&text) {
                        if value["t"] == "care" && value["signal"]["reason"] == "room_silence" {
                            return value["signal"]["id"].as_str().map(str::to_string);
                        }
                    }
                }
                _ => return None,
            },
        }
    }
}

/// An Everyone per-member ACK is gated by the ACKing session's authenticated
/// principal end-to-end over the real HTTP door: the row's owner cannot be
/// ACK'd by a different member, the owner ACKs its own and the receipt is
/// durable (reconnect no longer replays it), and a co-member's own delivery is
/// wholly independent of that ACK.
#[tokio::test]
async fn everyone_reminder_ack_is_principal_scoped_over_http() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir
        .path()
        .join("everyone-ack.sqlite")
        .to_string_lossy()
        .to_string();
    let (port, _guard) = spawn_server_env(
        "MASTER",
        &[
            ("DB_PATH", db),
            ("CARE_SILENCE_SECS", "1".into()),
            ("CARE_COOLDOWN_SECS", "60".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let mut session = std::collections::HashMap::new();
    for name in ["alice", "bob"] {
        let davet = davet_for(&base, "MASTER", "proj", name).await;
        session.insert(
            name,
            session_with(&base, ("x-room-token", davet.as_str()), name, Some("proj")).await,
        );
    }
    client
        .put(format!("{base}/rooms/proj/settings"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "care_recipient": { "kind": "all" } }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    let ws_url = |name: &str| {
        format!(
            "ws://127.0.0.1:{port}/ws?room=proj&name={name}&type=agent\
             &filter=mentions&turn_max=1&session={}",
            session[name]
        )
    };

    // Only alice is online during the fan-out; bob's per-member row is durable.
    let (mut alice, _) = tokio_tungstenite::connect_async(ws_url("alice"))
        .await
        .unwrap();
    client
        .post(format!("{base}/rooms/proj/messages"))
        .header("x-session-token", &session["alice"])
        .json(&serde_json::json!({ "sender": "alice", "sender_type": "agent", "text": "hi" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    let sig_alice = next_silence_signal(&mut alice, 4000)
        .await
        .expect("alice receives her per-member reminder");

    // A DIFFERENT member cannot ACK alice's delivery, even over the real door.
    let ack = |token: &str, by: &str, signal: &str| {
        client
            .post(format!("{base}/rooms/proj/care/{signal}/ack"))
            .header("x-session-token", token.to_string())
            .json(&serde_json::json!({ "by": by }))
            .send()
    };
    let bob_try = ack(&session["bob"], "bob", &sig_alice).await.unwrap();
    assert_eq!(
        bob_try.status(),
        reqwest::StatusCode::NOT_FOUND,
        "a co-member cannot ACK another member's Everyone delivery"
    );

    // Alice ACKs her own — accepted.
    let own = ack(&session["alice"], "alice", &sig_alice).await.unwrap();
    assert_eq!(
        own.status(),
        reqwest::StatusCode::NO_CONTENT,
        "the owning principal ACKs its own delivery over HTTP"
    );

    // The ACK is durable: alice's reconnect no longer replays it.
    let (mut alice2, _) = tokio_tungstenite::connect_async(ws_url("alice"))
        .await
        .unwrap();
    assert_eq!(
        count_silence_care(&mut alice2, 1500).await,
        0,
        "an acknowledged per-member reminder is not replayed on reconnect"
    );

    // bob's own delivery is wholly independent — his first connect still wakes
    // him exactly once, untouched by alice's ACK.
    let (mut bob, _) = tokio_tungstenite::connect_async(ws_url("bob"))
        .await
        .unwrap();
    assert_eq!(
        count_silence_care(&mut bob, 2000).await,
        1,
        "bob's per-member reminder is unaffected by alice's ACK"
    );
}

/// A member revoked AFTER the fan-out (its durable row already enqueued) gets
/// ZERO Care on reconnect. Over the real door revocation cascades to the
/// session, so the socket is refused outright — the durable row is never
/// delivered. (The defense-in-depth roster re-check inside the principal-scoped
/// replay, for the narrower case of an off-roster principal that still holds a
/// live session, is proved at the hub level in
/// `everyone_reconnect_replay_drops_a_revoked_principals_row`.)
#[tokio::test]
async fn everyone_reminder_revoked_member_gets_zero_on_reconnect() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir
        .path()
        .join("everyone-revoke.sqlite")
        .to_string_lossy()
        .to_string();
    let (port, _guard) = spawn_server_env(
        "MASTER",
        &[
            ("DB_PATH", db),
            ("CARE_SILENCE_SECS", "1".into()),
            ("CARE_COOLDOWN_SECS", "60".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let mut session = std::collections::HashMap::new();
    let mut davet = std::collections::HashMap::new();
    for name in ["alice", "carol"] {
        let dv = davet_for(&base, "MASTER", "proj", name).await;
        session.insert(
            name,
            session_with(&base, ("x-room-token", dv.as_str()), name, Some("proj")).await,
        );
        davet.insert(name, dv);
    }
    client
        .put(format!("{base}/rooms/proj/settings"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "care_recipient": { "kind": "all" } }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    let ws_url = |name: &str| {
        format!(
            "ws://127.0.0.1:{port}/ws?room=proj&name={name}&type=agent\
             &filter=mentions&turn_max=1&session={}",
            session[name]
        )
    };

    // alice online drives the silence; carol is offline, so her per-member row
    // is written to the durable outbox.
    let (mut alice, _) = tokio_tungstenite::connect_async(ws_url("alice"))
        .await
        .unwrap();
    client
        .post(format!("{base}/rooms/proj/messages"))
        .header("x-session-token", &session["alice"])
        .json(&serde_json::json!({ "sender": "alice", "sender_type": "agent", "text": "hi" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    assert_eq!(
        count_silence_care(&mut alice, 4000).await,
        1,
        "alice (still on the roster) is woken once"
    );

    // Revoke carol's davet AFTER the fan-out already enqueued her row.
    client
        .delete(format!("{base}/rooms/proj/invites/{}", davet["carol"]))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    // carol tries to reconnect: revocation cascaded to her session, so the door
    // refuses the socket — she receives zero Care because she cannot reconnect.
    let reconnect = tokio_tungstenite::connect_async(ws_url("carol")).await;
    assert!(
        reconnect.is_err(),
        "a member revoked after the fan-out cannot reconnect — its session is revoked with the davet, so it receives zero"
    );
}

/// Two DISTINCT canonical principals that share a display name each receive ONLY
/// their own Everyone Care over real WebSockets — delivery is principal-scoped,
/// never name-based, so a shared name cannot cross-deliver. The server refuses
/// two live members with one name, so we seed two distinct members (distinct
/// principals) with principal-bound sessions, rename the second to the same name
/// in the DB while the server is down, then restart onto the same DB+port: the
/// reloaded roster now holds two same-named principals and the sessions persist.
#[tokio::test]
async fn everyone_reminder_separates_two_same_named_principals_over_websockets() {
    // Fixed port + temp DB so we can rename between two boots of the same server.
    let port = {
        let l = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let p = l.local_addr().unwrap().port();
        drop(l);
        p
    };
    let db = std::env::temp_dir().join(format!("everyone-samename-{port}.db"));
    let _ = std::fs::remove_file(&db);
    let db_str = db.to_string_lossy().to_string();
    let boot_env = |port: u16| {
        vec![
            ("PORT", port.to_string()),
            ("DB_PATH", db_str.clone()),
            ("CARE_SILENCE_SECS", "1".to_string()),
            ("CARE_COOLDOWN_SECS", "60".to_string()),
        ]
    };
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // Boot 1: two distinct members (distinct principals), each davetted into proj.
    // care_recipient = Everyone. Davets are durable across restart; davet-derived
    // sessions are ephemeral, so we re-mint them on boot 2.
    let dv_a;
    let dv_b;
    {
        let (_p, _guard) = spawn_server_env("MASTER", &boot_env(port)).await;
        dv_a = davet_for(&base, "MASTER", "proj", "sam").await;
        dv_b = davet_for(&base, "MASTER", "proj", "sam-two").await;
        client
            .put(format!("{base}/rooms/proj/settings"))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({ "care_recipient": { "kind": "all" } }))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        // _guard drops here -> server killed.
    }
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Rename the second identity to the SAME display name while the server is
    // down — in members (the fan-out roster name) AND principals (the session's
    // resolved name) — so the reloaded roster holds two distinct principals both
    // shown as "sam". principal_id is untouched, so they stay distinct.
    {
        let sql = rusqlite::Connection::open(&db).unwrap();
        sql.busy_timeout(Duration::from_secs(5)).unwrap();
        let renamed_member = sql
            .execute("UPDATE members SET name = 'sam' WHERE name = 'sam-two'", [])
            .unwrap();
        assert_eq!(renamed_member, 1, "renamed exactly the second member");
        sql.execute(
            "UPDATE principals SET display_name = 'sam' WHERE display_name = 'sam-two'",
            [],
        )
        .unwrap();
    }

    // Boot 2: same DB + port. The roster now has two same-named principals.
    let (_p2, _guard2) = spawn_server_env("MASTER", &boot_env(port)).await;

    // Re-mint each session from its durable davet: a session speaks as its davet's
    // member, so both now resolve to the display name "sam" with DISTINCT principals.
    let session_a = session_with(&base, ("x-room-token", dv_a.as_str()), "sam", Some("proj")).await;
    let session_b = session_with(&base, ("x-room-token", dv_b.as_str()), "sam", Some("proj")).await;

    let ws_url = |session: &str| {
        format!(
            "ws://127.0.0.1:{port}/ws?room=proj&name=sam&type=agent\
             &filter=mentions&turn_max=1&session={session}"
        )
    };
    let (mut sam_a, _) = tokio_tungstenite::connect_async(ws_url(&session_a))
        .await
        .unwrap();
    let (mut sam_b, _) = tokio_tungstenite::connect_async(ws_url(&session_b))
        .await
        .unwrap();

    // Arm silence: one message, then the room goes quiet.
    client
        .post(format!("{base}/rooms/proj/messages"))
        .header("x-session-token", &session_a)
        .json(&serde_json::json!({ "sender": "sam", "sender_type": "agent", "text": "hi" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    // Each same-named socket receives EXACTLY its own principal's row. A name-based
    // delivery would hand BOTH "sam" rows to BOTH sockets.
    let a = next_silence_signal(&mut sam_a, 4000)
        .await
        .expect("sam(A) receives its own reminder");
    let b = next_silence_signal(&mut sam_b, 2000)
        .await
        .expect("sam(B) receives its own reminder");
    assert_ne!(a, b, "each socket received a distinct principal's delivery");
    // No SECOND Care to either socket — neither received the other principal's row.
    assert_eq!(
        count_silence_care(&mut sam_a, 800).await,
        0,
        "sam(A) got only its own row, never sam(B)'s"
    );
    assert_eq!(
        count_silence_care(&mut sam_b, 800).await,
        0,
        "sam(B) got only its own row, never sam(A)'s"
    );

    let _ = std::fs::remove_file(&db);
}
