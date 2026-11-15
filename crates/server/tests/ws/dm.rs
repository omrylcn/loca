//! Direct message: a private loca for exactly two people (PRINCIPLES "Birebir").
use super::*;

const MASTER: &str = "bb-master-key";

async fn session(base: &str, header: (&str, &str), name: &str, kind: &str) -> String {
    let v: Value = reqwest::Client::new()
        .post(format!("{base}/sessions"))
        .header(header.0, header.1)
        .json(&serde_json::json!({ "name": name, "kind": kind }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    v["session_token"].as_str().unwrap().to_string()
}

async fn member(base: &str, name: &str, kind: &str) -> String {
    let v: Value = reqwest::Client::new()
        .post(format!("{base}/members"))
        .header("x-admin-token", MASTER)
        .json(&serde_json::json!({ "name": name, "kind": kind }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    v["token"].as_str().unwrap().to_string()
}

async fn seat(base: &str, room: &str, name: &str) {
    reqwest::Client::new()
        .post(format!("{base}/rooms/{room}/invites"))
        .header("x-admin-token", MASTER)
        .json(&serde_json::json!({ "name": name }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
}

async fn call(
    method: reqwest::Method,
    url: String,
    session: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let mut req = reqwest::Client::new()
        .request(method, url)
        .header("x-session-token", session);
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

async fn say(base: &str, room: &str, session: &str, text: &str) -> Value {
    let (status, v) = call(
        reqwest::Method::POST,
        format!("{base}/rooms/{room}/messages"),
        session,
        Some(serde_json::json!({ "sender": "x", "sender_type": "user", "text": text })),
    )
    .await;
    assert!(status < 300, "post in {room}: {status} {v}");
    v
}

fn texts(v: &Value) -> Vec<String> {
    v.as_array()
        .or_else(|| v["messages"].as_array())
        .map(|a| {
            a.iter()
                .map(|m| m["text"].as_str().unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_dm_belongs_to_its_two_people_and_the_master_sees_only_that_it_exists() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("dm.db");
    let (port, _guard) = spawn_server_env(
        MASTER,
        &[
            ("DB_PATH", db.to_str().unwrap().into()),
            ("REQUIRE_SESSIONS", "1".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let get = reqwest::Method::GET;
    let post = reqwest::Method::POST;

    // The cast: two people at a shared table, a Master, a Smaster, an agent,
    // and an outsider who shares no table with anyone.
    let pairing: Value = reqwest::Client::new()
        .post(format!("{base}/pairings?ttl_hours=1"))
        .header("x-admin-token", MASTER)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let master = session(
        &base,
        ("x-pairing-code", pairing["pairing_code"].as_str().unwrap()),
        "",
        "user",
    )
    .await;
    let deputy_key: Value = reqwest::Client::new()
        .post(format!("{base}/smasters"))
        .header("x-admin-token", MASTER)
        .json(&serde_json::json!({ "name": "deputy" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let deputy = session(
        &base,
        ("x-admin-token", deputy_key["token"].as_str().unwrap()),
        "deputy",
        "user",
    )
    .await;
    let ayse_key = member(&base, "ayse", "user").await;
    let mehmet_key = member(&base, "mehmet", "user").await;
    let outsider_key = member(&base, "outsider", "user").await;
    let helper_key = member(&base, "helper", "agent").await;
    create_test_room(port, MASTER, "team").await;
    seat(&base, "team", "ayse").await;
    seat(&base, "team", "mehmet").await;
    let ayse = session(&base, ("x-room-token", &ayse_key), "ayse", "user").await;
    let mehmet = session(&base, ("x-room-token", &mehmet_key), "mehmet", "user").await;
    let outsider = session(&base, ("x-room-token", &outsider_key), "outsider", "user").await;
    let helper = session(&base, ("x-room-token", &helper_key), "helper", "agent").await;

    // Opening: an agent never opens one; a member only with a tablemate.
    let open = |who: String, name: &'static str| {
        let base = base.clone();
        async move {
            call(
                reqwest::Method::POST,
                format!("{base}/birebirs"),
                &who,
                Some(serde_json::json!({ "name": name })),
            )
            .await
        }
    };
    assert_eq!(
        open(helper.clone(), "ayse").await.0,
        403,
        "an agent never opens a direct message"
    );
    assert_eq!(
        open(outsider.clone(), "ayse").await.0,
        403,
        "no shared table, no direct message"
    );
    assert_eq!(
        open(ayse.clone(), "ayse").await.0,
        400,
        "a birebir needs two people"
    );
    let (status, opened) = open(ayse.clone(), "mehmet").await;
    assert_eq!(status, 201, "{opened}");
    let room = opened["birebir"]["room"].as_str().unwrap().to_string();
    assert!(room.starts_with("bb-") && !room.contains("ayse") && !room.contains("mehmet"));
    assert_eq!(opened["birebir"]["peer"], "mehmet");
    let (again, reopened) = open(mehmet.clone(), "ayse").await;
    assert_eq!(
        again, 200,
        "the same pair returns to the same direct message"
    );
    assert_eq!(reopened["birebir"]["room"], room.as_str());

    // The two talk; a word is addressed to the other side without an @.
    let posted = say(&base, &room, &ayse, "secret before any call").await;
    assert_eq!(posted["target"], "mehmet");
    let (_, widened) = call(
        post.clone(),
        format!("{base}/rooms/{room}/messages"),
        &ayse,
        Some(serde_json::json!({ "sender": "x", "sender_type": "user", "text": "to all?", "target": "all" })),
    )
    .await;
    assert_eq!(
        widened["target"], "mehmet",
        "an @all cannot widen a direct message"
    );
    let (status, seen) = call(
        get.clone(),
        format!("{base}/rooms/{room}/messages"),
        &mehmet,
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert!(texts(&seen).contains(&"secret before any call".to_string()));

    // Nobody else gets in — not by rank, not by the raw Master key, not over
    // WS — and they get the same answer as a loca that does not exist.
    for (who, s) in [
        ("master", &master),
        ("smaster", &deputy),
        ("outsider", &outsider),
        ("agent", &helper),
    ] {
        let (status, _) = call(
            get.clone(),
            format!("{base}/rooms/{room}/messages"),
            s,
            None,
        )
        .await;
        assert_eq!(status, 404, "{who} must not read a direct message");
        let (status, _) = call(
            get.clone(),
            format!("{base}/rooms/{room}/search?q=secret"),
            s,
            None,
        )
        .await;
        assert_eq!(status, 404, "{who} must not search a direct message");
    }
    assert_eq!(
        rest_can(&base, &room, &[("x-admin-token", MASTER)]).await,
        404
    );
    assert!(!ws_can(port, &room, &format!("token={MASTER}")).await);

    // ...nor sees that it exists in any list.
    for s in [&master, &deputy] {
        let (_, rooms) = call(get.clone(), format!("{base}/rooms"), s, None).await;
        assert!(
            !rooms.to_string().contains(&room),
            "a rank's loca list never names a direct message"
        );
    }
    let (_, ranks) = call(get.clone(), format!("{base}/building/ranks"), &master, None).await;
    assert!(!ranks.to_string().contains(&room));
    let (_, residents) = call(get.clone(), format!("{base}/residents"), &master, None).await;
    assert!(!residents.to_string().contains(&room));
    let (_, mine) = call(get.clone(), format!("{base}/rooms"), &mehmet, None).await;
    assert!(
        mine.to_string().contains(&room),
        "the two see their direct message"
    );

    // The Master — only the Master — sees that it exists, never a word.
    let (status, overview) = call(
        get.clone(),
        format!("{base}/building/birebirs"),
        &master,
        None,
    )
    .await;
    assert_eq!(status, 200);
    let entry = overview
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["room"] == room.as_str())
        .unwrap();
    let mut between: Vec<String> = entry["between"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap().to_string())
        .collect();
    between.sort();
    assert_eq!(between, ["ayse", "mehmet"]);
    assert!(!overview.to_string().contains("secret"));
    assert_eq!(
        call(
            get.clone(),
            format!("{base}/building/birebirs"),
            &deputy,
            None
        )
        .await
        .0,
        403
    );

    // Conversation only: no tasks, invites, settings, deletion.
    assert_eq!(
        call(
            post.clone(),
            format!("{base}/rooms/{room}/tasks"),
            &ayse,
            Some(serde_json::json!({"title":"x"}))
        )
        .await
        .0,
        403
    );
    assert_eq!(
        call(
            post.clone(),
            format!("{base}/rooms/{room}/invites"),
            &ayse,
            Some(serde_json::json!({"name":"outsider"}))
        )
        .await
        .0,
        403
    );
    assert_eq!(
        call(
            post.clone(),
            format!("{base}/rooms/{room}/call"),
            &master,
            Some(serde_json::json!({"name":"outsider"}))
        )
        .await
        .0,
        404
    );

    // No look: the Master reads nothing until called in.
    let witness = format!("{base}/building/birebirs/{room}/messages");
    assert_eq!(
        call(get.clone(), witness.clone(), &master, None).await.0,
        403
    );
    let master_action = |who: String, action: &'static str| {
        let url = format!("{base}/birebirs/{room}/master");
        async move {
            call(
                reqwest::Method::POST,
                url,
                &who,
                Some(serde_json::json!({ "action": action })),
            )
            .await
        }
    };
    assert_eq!(
        master_action(outsider.clone(), "call").await.0,
        404,
        "only the two may call the Master"
    );
    let (status, view) = master_action(mehmet.clone(), "call").await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["master_in"], true);
    // From the call on — the earlier word stays closed.
    say(&base, &room, &ayse, "said after the call").await;
    let (status, read) = call(get.clone(), witness.clone(), &master, None).await;
    assert_eq!(status, 200);
    assert!(texts(&read).contains(&"said after the call".to_string()));
    assert!(!texts(&read).contains(&"secret before any call".to_string()));
    // A Smaster is never a witness.
    assert_eq!(
        call(get.clone(), witness.clone(), &deputy, None).await.0,
        403
    );
    // History opens only when BOTH allow it.
    master_action(ayse.clone(), "history").await;
    let (_, read) = call(get.clone(), witness.clone(), &master, None).await;
    assert!(!texts(&read).contains(&"secret before any call".to_string()));
    master_action(mehmet.clone(), "history").await;
    let (_, read) = call(get.clone(), witness.clone(), &master, None).await;
    assert!(texts(&read).contains(&"secret before any call".to_string()));
    // Only the caller sends the Master out; then nothing is readable again.
    assert_eq!(master_action(ayse.clone(), "dismiss").await.0, 403);
    assert_eq!(master_action(mehmet.clone(), "dismiss").await.0, 200);
    assert_eq!(
        call(get.clone(), witness.clone(), &master, None).await.0,
        403
    );

    // The id space is reserved: nobody squats a pair's room in advance.
    let squat = reqwest::Client::new()
        .post(format!("{base}/rooms"))
        .header("x-admin-token", MASTER)
        .json(&serde_json::json!({ "name": "bb-0000squat" }))
        .send()
        .await
        .unwrap();
    assert!(
        !squat.status().is_success(),
        "an ordinary loca never takes a direct message id"
    );
    assert!(!ws_can(port, "bb-0000squat", &format!("token={MASTER}")).await);

    // Seal: the Master closes, then seals; the pair cannot reopen it.
    let close = format!("{base}/building/birebirs/{room}");
    assert_eq!(
        call(post.clone(), format!("{close}/seal"), &master, None)
            .await
            .0,
        409,
        "close first"
    );
    assert_eq!(
        call(
            post.clone(),
            format!("{close}/close"),
            &deputy,
            Some(serde_json::json!({}))
        )
        .await
        .0,
        403
    );
    assert_eq!(
        call(
            post.clone(),
            format!("{close}/close"),
            &master,
            Some(serde_json::json!({}))
        )
        .await
        .0,
        200
    );
    assert_eq!(
        call(post.clone(), format!("{close}/seal"), &master, None)
            .await
            .0,
        200
    );
    assert_eq!(
        open(ayse.clone(), "mehmet").await.0,
        409,
        "a sealed pair stays sealed"
    );
}

/// Follow the witness route's `since`/`limit` paging to its end. Bounded, so
/// a server that ignores the cursor fails the caller's assertion, not the run.
async fn witness_pages(witness: &str, master: &str) -> Vec<String> {
    const PAGE: usize = 100;
    let mut all = Vec::new();
    let mut since = 0;
    for _ in 0..20 {
        let (status, page) = call(
            reqwest::Method::GET,
            format!("{witness}?since={since}&limit={PAGE}"),
            master,
            None,
        )
        .await;
        assert_eq!(status, 200, "{page}");
        let messages = page["messages"].as_array().unwrap();
        all.extend(texts(&page));
        if messages.len() < PAGE {
            break;
        }
        since = messages.last().unwrap()["id"].as_u64().unwrap();
    }
    all
}

#[tokio::test]
async fn a_called_in_master_reads_a_long_dm_whole_but_never_before_the_call() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("dm.db");
    let (port, _guard) = spawn_server_env(
        MASTER,
        &[
            ("DB_PATH", db.to_str().unwrap().into()),
            ("REQUIRE_SESSIONS", "1".into()),
            // Two people must say well over the 200-message hot tail.
            ("RATE_LIMIT", "0".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let get = reqwest::Method::GET;
    let post = reqwest::Method::POST;

    let pairing: Value = reqwest::Client::new()
        .post(format!("{base}/pairings?ttl_hours=1"))
        .header("x-admin-token", MASTER)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let master = session(
        &base,
        ("x-pairing-code", pairing["pairing_code"].as_str().unwrap()),
        "",
        "user",
    )
    .await;
    let ayse_key = member(&base, "ayse", "user").await;
    let mehmet_key = member(&base, "mehmet", "user").await;
    create_test_room(port, MASTER, "team").await;
    seat(&base, "team", "ayse").await;
    seat(&base, "team", "mehmet").await;
    let ayse = session(&base, ("x-room-token", &ayse_key), "ayse", "user").await;
    let mehmet = session(&base, ("x-room-token", &mehmet_key), "mehmet", "user").await;
    let (status, opened) = call(
        post.clone(),
        format!("{base}/birebirs"),
        &ayse,
        Some(serde_json::json!({ "name": "mehmet" })),
    )
    .await;
    assert_eq!(status, 201, "{opened}");
    let room = opened["birebir"]["room"].as_str().unwrap().to_string();
    let witness = format!("{base}/building/birebirs/{room}/messages");
    let paged = format!("{witness}?since=0");
    let master_action = |who: String, action: &'static str| {
        let url = format!("{base}/birebirs/{room}/master");
        async move {
            call(
                reqwest::Method::POST,
                url,
                &who,
                Some(serde_json::json!({ "action": action })),
            )
            .await
        }
    };

    let before: Vec<String> = (0..5).map(|i| format!("before the call {i}")).collect();
    for text in &before {
        say(&base, &room, &ayse, text).await;
    }
    assert_eq!(
        call(get.clone(), paged.clone(), &master, None).await.0,
        403,
        "paging is no way in before a call"
    );
    // The call is stamped in milliseconds; keep it clear of the last word.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    assert_eq!(master_action(mehmet.clone(), "call").await.0, 200);
    let after: Vec<String> = (0..230).map(|i| format!("after the call {i}")).collect();
    for (i, text) in after.iter().enumerate() {
        say(&base, &room, if i % 2 == 0 { &ayse } else { &mehmet }, text).await;
    }

    // From the call on: every word of it, and not one from before.
    assert_eq!(witness_pages(&witness, &master).await, after);
    // One side allowing history opens nothing.
    master_action(ayse.clone(), "history").await;
    assert_eq!(witness_pages(&witness, &master).await, after);
    // Both allowed: the whole direct message, from its first word.
    master_action(mehmet.clone(), "history").await;
    let whole: Vec<String> = before.iter().chain(&after).cloned().collect();
    assert_eq!(witness_pages(&witness, &master).await, whole);

    // The archive closes with the visit, and stays closed once sealed.
    assert_eq!(master_action(mehmet.clone(), "dismiss").await.0, 200);
    assert_eq!(call(get.clone(), paged.clone(), &master, None).await.0, 403);
    assert_eq!(master_action(ayse.clone(), "call").await.0, 200);
    assert_eq!(call(get.clone(), paged.clone(), &master, None).await.0, 200);
    let close = format!("{base}/building/birebirs/{room}");
    for step in ["close", "seal"] {
        let (status, body) = call(
            post.clone(),
            format!("{close}/{step}"),
            &master,
            Some(serde_json::json!({})),
        )
        .await;
        assert_eq!(status, 200, "{step}: {body}");
    }
    assert_eq!(
        call(get.clone(), paged.clone(), &master, None).await.0,
        403,
        "a sealed direct message is read by nobody"
    );
    assert_eq!(call(get, witness, &master, None).await.0, 403);
}

#[tokio::test]
async fn a_dm_with_an_agent_reaches_it_by_invite_and_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("dm-agent.db");
    let env = [
        ("DB_PATH", db.to_str().unwrap().to_string()),
        ("REQUIRE_SESSIONS", "1".to_string()),
    ];
    let (port, guard) = spawn_server_env(MASTER, &env).await;
    let base = format!("http://127.0.0.1:{port}");
    member(&base, "worker", "agent").await;
    let pairing: Value = reqwest::Client::new()
        .post(format!("{base}/pairings?ttl_hours=1"))
        .header("x-admin-token", MASTER)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let master = session(
        &base,
        ("x-pairing-code", pairing["pairing_code"].as_str().unwrap()),
        "",
        "user",
    )
    .await;
    // A rank may open one with any member, agents included.
    let (status, opened) = call(
        reqwest::Method::POST,
        format!("{base}/birebirs"),
        &master,
        Some(serde_json::json!({ "name": "worker" })),
    )
    .await;
    assert_eq!(status, 201, "{opened}");
    let room = opened["birebir"]["room"].as_str().unwrap().to_string();
    // The agent side got an invite for exactly this room — the call path.
    let invites: Vec<Value> = reqwest::Client::new()
        .get(format!("{base}/residents"))
        .header("x-admin-token", MASTER)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!serde_json::to_string(&invites).unwrap().contains(&room));
    // The Master is a party here, so it reads its own direct message by session.
    say(&base, &room, &master, "hello worker").await;
    let (status, read) = call(
        reqwest::Method::GET,
        format!("{base}/rooms/{room}/messages"),
        &master,
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert!(texts(&read).contains(&"hello worker".to_string()));

    // Restart: the direct message, its door and its words come back.
    drop(guard);
    wait_for_server_shutdown(port).await;
    let (port, _guard) = spawn_server_env(MASTER, &env).await;
    let base = format!("http://127.0.0.1:{port}");
    let (status, read) = call(
        reqwest::Method::GET,
        format!("{base}/rooms/{room}/messages"),
        &master,
        None,
    )
    .await;
    assert_eq!(status, 200, "a party still reads after restart");
    assert!(texts(&read).contains(&"hello worker".to_string()));
}

#[tokio::test]
async fn ranks_write_to_each_other_and_members_only_to_tablemates() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("dm-candidates.db");
    let (port, _guard) = spawn_server_env(
        MASTER,
        &[
            ("DB_PATH", db.to_str().unwrap().into()),
            ("REQUIRE_SESSIONS", "1".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let smaster = |name: &'static str| {
        let base = base.clone();
        async move {
            let key: Value = reqwest::Client::new()
                .post(format!("{base}/smasters"))
                .header("x-admin-token", MASTER)
                .json(&serde_json::json!({ "name": name }))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            session(
                &base,
                ("x-admin-token", key["token"].as_str().unwrap()),
                name,
                "user",
            )
            .await
        }
    };
    let sa = smaster("rank-a").await;
    let sb = smaster("rank-b").await;
    let pairing: Value = reqwest::Client::new()
        .post(format!("{base}/pairings?ttl_hours=1"))
        .header("x-admin-token", MASTER)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let master = session(
        &base,
        ("x-pairing-code", pairing["pairing_code"].as_str().unwrap()),
        "",
        "user",
    )
    .await;
    let ayse_key = member(&base, "cand-ayse", "user").await;
    let mehmet_key = member(&base, "cand-mehmet", "user").await;
    member(&base, "cand-lone", "user").await;
    let bot_key = member(&base, "cand-bot", "agent").await;
    create_test_room(port, MASTER, "cand-team").await;
    seat(&base, "cand-team", "cand-ayse").await;
    seat(&base, "cand-team", "cand-mehmet").await;
    let ayse = session(&base, ("x-room-token", &ayse_key), "cand-ayse", "user").await;
    let bot = session(&base, ("x-room-token", &bot_key), "cand-bot", "agent").await;
    let _ = mehmet_key;

    let names = |v: &Value| -> Vec<String> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap().to_string())
            .collect()
    };
    // A rank sees everyone, other ranks included; never itself.
    let (status, list) = call(
        reqwest::Method::GET,
        format!("{base}/birebirs/candidates"),
        &sa,
        None,
    )
    .await;
    assert_eq!(status, 200);
    let l = names(&list);
    assert!(l.contains(&"rank-b".to_string()) && l.contains(&"cand-lone".to_string()));
    assert!(!l.contains(&"rank-a".to_string()));
    // A member sees only tablemates; an agent sees nobody.
    let (_, list) = call(
        reqwest::Method::GET,
        format!("{base}/birebirs/candidates"),
        &ayse,
        None,
    )
    .await;
    assert_eq!(names(&list), vec!["cand-mehmet".to_string()]);
    let (_, list) = call(
        reqwest::Method::GET,
        format!("{base}/birebirs/candidates"),
        &bot,
        None,
    )
    .await;
    assert!(names(&list).is_empty());

    // Smaster↔Smaster and Smaster↔Master open without a shared loca.
    let open = |who: String, name: &'static str| {
        let base = base.clone();
        async move {
            call(
                reqwest::Method::POST,
                format!("{base}/birebirs"),
                &who,
                Some(serde_json::json!({ "name": name })),
            )
            .await
        }
    };
    assert_eq!(open(sa.clone(), "rank-b").await.0, 201);
    let master_name = {
        let (_, list) = call(
            reqwest::Method::GET,
            format!("{base}/birebirs/candidates"),
            &sa,
            None,
        )
        .await;
        list.as_array()
            .unwrap()
            .iter()
            .find(|c| c["role"] == "master")
            .unwrap()["name"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let (status, _) = call(
        reqwest::Method::POST,
        format!("{base}/birebirs"),
        &sb,
        Some(serde_json::json!({ "name": master_name })),
    )
    .await;
    assert_eq!(status, 201, "a Smaster writes to the Master");
    let (_, mine) = call(
        reqwest::Method::GET,
        format!("{base}/birebirs"),
        &master,
        None,
    )
    .await;
    assert_eq!(
        mine.as_array().unwrap().len(),
        1,
        "the Master sees the direct message it is part of"
    );
}

/// A Building with a Master, a Smaster, and three members at one table:
/// `ayse` and `mehmet` who open a direct message, and `neighbour`, who sits with both.
struct Cast {
    base: String,
    port: u16,
    master: String,
    deputy: String,
    ayse: String,
    mehmet: String,
    neighbour: String,
    mehmet_member_id: String,
}

async fn cast(db: &std::path::Path) -> (Cast, ServerGuard) {
    let (port, guard) = spawn_server_env(
        MASTER,
        &[
            ("DB_PATH", db.to_str().unwrap().into()),
            ("REQUIRE_SESSIONS", "1".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let pairing: Value = reqwest::Client::new()
        .post(format!("{base}/pairings?ttl_hours=1"))
        .header("x-admin-token", MASTER)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let master = session(
        &base,
        ("x-pairing-code", pairing["pairing_code"].as_str().unwrap()),
        "",
        "user",
    )
    .await;
    let deputy_key: Value = reqwest::Client::new()
        .post(format!("{base}/smasters"))
        .header("x-admin-token", MASTER)
        .json(&serde_json::json!({ "name": "deputy" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let deputy = session(
        &base,
        ("x-admin-token", deputy_key["token"].as_str().unwrap()),
        "deputy",
        "user",
    )
    .await;
    create_test_room(port, MASTER, "team").await;
    let mut sessions = Vec::new();
    for name in ["ayse", "mehmet", "neighbour"] {
        let key = member(&base, name, "user").await;
        seat(&base, "team", name).await;
        sessions.push(session(&base, ("x-room-token", &key), name, "user").await);
    }
    let members: Value = reqwest::Client::new()
        .get(format!("{base}/members"))
        .header("x-admin-token", MASTER)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mehmet_member_id = members
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "mehmet")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let neighbour = sessions.pop().unwrap();
    let mehmet = sessions.pop().unwrap();
    let ayse = sessions.pop().unwrap();
    (
        Cast {
            base,
            port,
            master,
            deputy,
            ayse,
            mehmet,
            neighbour,
            mehmet_member_id,
        },
        guard,
    )
}

/// The Messages list and the "new message" picker name a person as everyone
/// else does: the `@name` with the card beside it. A side that left the
/// building leaves the direct message read-only, and its view says so.
#[tokio::test]
async fn a_dm_describes_the_other_side_and_says_when_it_is_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let (c, _guard) = cast(&dir.path().join("dm-view.db")).await;
    let (status, card) = call(
        reqwest::Method::PUT,
        format!("{}/profile/card", c.base),
        &c.mehmet,
        Some(serde_json::json!({ "full_name": "Mehmet Öztürk", "icon": "leaf" })),
    )
    .await;
    assert_eq!(status, 200, "{card}");

    // The picker: the card of whom one may write to, beside the @name.
    let (_, candidates) = call(
        reqwest::Method::GET,
        format!("{}/birebirs/candidates", c.base),
        &c.ayse,
        None,
    )
    .await;
    let mehmet = candidates
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "mehmet")
        .unwrap();
    assert_eq!(mehmet["full_name"], "Mehmet Öztürk");
    assert_eq!(mehmet["icon"], "leaf");
    assert!(mehmet["principal_id"].as_str().unwrap().starts_with("pr_"));

    // The list: the other side's card and principal (what its lines carry).
    let (status, opened) = call(
        reqwest::Method::POST,
        format!("{}/birebirs", c.base),
        &c.ayse,
        Some(serde_json::json!({ "name": "mehmet" })),
    )
    .await;
    assert_eq!(status, 201, "{opened}");
    let room = opened["birebir"]["room"].as_str().unwrap().to_string();
    let posted = say(&c.base, &room, &c.mehmet, "from mehmet").await;
    let (_, mine) = call(
        reqwest::Method::GET,
        format!("{}/birebirs", c.base),
        &c.ayse,
        None,
    )
    .await;
    let view = &mine[0];
    assert_eq!(view["peer"], "mehmet");
    assert_eq!(view["peer_full_name"], "Mehmet Öztürk");
    assert_eq!(view["peer_icon"], "leaf");
    assert_eq!(view["peer_principal"], posted["sender_principal"]);
    assert_eq!(view["read_only"], false);
    // My own card is not described as the other side's.
    let (_, theirs) = call(
        reqwest::Method::GET,
        format!("{}/birebirs", c.base),
        &c.mehmet,
        None,
    )
    .await;
    assert_eq!(theirs[0]["peer"], "ayse");
    assert!(theirs[0].get("peer_full_name").is_none(), "{theirs}");

    // Mehmet leaves the building: read-only, said before anyone types.
    let revoked = reqwest::Client::new()
        .delete(format!("{}/members/{}", c.base, c.mehmet_member_id))
        .header("x-admin-token", MASTER)
        .send()
        .await
        .unwrap();
    assert!(revoked.status().is_success());
    let (_, mine) = call(
        reqwest::Method::GET,
        format!("{}/birebirs", c.base),
        &c.ayse,
        None,
    )
    .await;
    assert_eq!(mine[0]["read_only"], true, "{mine}");
    assert!(mine[0].get("peer_full_name").is_none(), "{mine}");
    let (status, _) = call(
        reqwest::Method::POST,
        format!("{}/rooms/{room}/messages", c.base),
        &c.ayse,
        Some(serde_json::json!({ "sender": "x", "sender_type": "user", "text": "still there?" })),
    )
    .await;
    assert!(
        status >= 400,
        "nothing more is said in a read-only direct message"
    );
}

/// What a person reads calls it a private conversation; the word "birebir"
/// stays in the code.
#[tokio::test]
async fn no_answer_a_person_reads_calls_it_a_dm() {
    let dir = tempfile::tempdir().unwrap();
    let (c, _guard) = cast(&dir.path().join("dm-words.db")).await;
    member(&c.base, "lone", "user").await;
    let open = |who: String, name: &'static str| {
        let url = format!("{}/birebirs", c.base);
        async move {
            call(
                reqwest::Method::POST,
                url,
                &who,
                Some(serde_json::json!({ "name": name })),
            )
            .await
        }
    };
    let mut answers = vec![
        open(c.ayse.clone(), "lone").await,
        open(c.ayse.clone(), "ayse").await,
    ];
    let (_, opened) = open(c.ayse.clone(), "mehmet").await;
    let room = opened["birebir"]["room"].as_str().unwrap().to_string();
    answers.push(
        call(
            reqwest::Method::GET,
            format!("{}/building/birebirs/{room}/messages", c.base),
            &c.master,
            None,
        )
        .await,
    );
    answers.push(
        call(
            reqwest::Method::POST,
            format!("{}/birebirs/{room}/master", c.base),
            &c.ayse,
            Some(serde_json::json!({ "action": "dismiss" })),
        )
        .await,
    );
    answers.push(
        call(
            reqwest::Method::POST,
            format!("{}/building/birebirs/{room}/seal", c.base),
            &c.master,
            None,
        )
        .await,
    );
    answers.push(
        call(
            reqwest::Method::POST,
            format!("{}/rooms/{room}/tasks", c.base),
            &c.ayse,
            Some(serde_json::json!({ "title": "x" })),
        )
        .await,
    );
    for (status, body) in answers {
        assert!(status >= 400, "{status} {body}");
        let text = body.as_str().unwrap_or_default().to_lowercase();
        assert!(!text.is_empty(), "{status}: a refusal says why");
        assert!(!text.contains("birebir"), "{status}: {text}");
    }
}

async fn ws_refusal(port: u16, room: &str, session: &str) -> Option<u16> {
    let url = format!("ws://127.0.0.1:{port}/ws?room={room}&name=probe&type=user");
    let mut request = url.into_client_request().unwrap();
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_str(&format!("loca.v1, loca.session.{session}")).unwrap(),
    );
    match tokio_tungstenite::connect_async(request).await {
        Ok(_) => None,
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            Some(response.status().as_u16())
        }
        // Only an HTTP answer is a refusal. A connection that broke for any
        // other reason proves nothing, and two sockets broken the same way
        // must not compare equal.
        Err(other) => panic!("the handshake did not get an HTTP answer: {other}"),
    }
}

/// The privacy probes the first test does not make: a member who sits with
/// both, a Smaster and the Master (not called in) learn nothing of someone
/// else's direct message by any other route. Every answer is exactly the answer for
/// an id that names nothing.
#[tokio::test]
async fn nobody_else_learns_a_dm_by_any_route() {
    let dir = tempfile::tempdir().unwrap();
    let (c, _guard) = cast(&dir.path().join("dm-probes.db")).await;
    let (_, opened) = call(
        reqwest::Method::POST,
        format!("{}/birebirs", c.base),
        &c.ayse,
        Some(serde_json::json!({ "name": "mehmet" })),
    )
    .await;
    let room = opened["birebir"]["room"].as_str().unwrap().to_string();
    let uploaded: Value = reqwest::Client::new()
        .post(format!("{}/rooms/{room}/attachments", c.base))
        .header("x-session-token", &c.ayse)
        .header("x-filename", "plan.txt")
        .header("content-type", "text/plain")
        .body("private file")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let attachment = uploaded["id"].as_str().unwrap().to_string();
    let message = say(&c.base, &room, &c.ayse, "private words").await;
    let id = message["id"].as_u64().unwrap();
    let missing = "bb-00000000000000000000000000000000";
    let get = reqwest::Method::GET;
    let post = reqwest::Method::POST;
    let routes = |r: &str| {
        vec![
            (get.clone(), format!("/rooms/{r}"), None),
            (get.clone(), format!("/rooms/{r}/members"), None),
            (get.clone(), format!("/rooms/{r}/settings"), None),
            (get.clone(), format!("/rooms/{r}/reactions"), None),
            (
                get.clone(),
                format!("/rooms/{r}/attachments/{attachment}"),
                None,
            ),
            (
                post.clone(),
                format!("/rooms/{r}/messages"),
                Some(serde_json::json!({ "sender": "x", "sender_type": "user", "text": "in" })),
            ),
            (
                post.clone(),
                format!("/rooms/{r}/messages/{id}/reactions"),
                Some(serde_json::json!({ "emoji": "✓", "active": true })),
            ),
            (
                post.clone(),
                format!("/birebirs/{r}/master"),
                Some(serde_json::json!({ "action": "call" })),
            ),
            (
                post.clone(),
                format!("/birebirs/{r}/master"),
                Some(serde_json::json!({ "action": "history" })),
            ),
        ]
    };
    for (who, s) in [
        ("tablemate", &c.neighbour),
        ("smaster", &c.deputy),
        ("master", &c.master),
    ] {
        for ((method, path, body), (_, missing_path, missing_body)) in
            routes(&room).into_iter().zip(routes(missing))
        {
            let seen = call(method.clone(), format!("{}{path}", c.base), s, body).await;
            let none = call(method, format!("{}{missing_path}", c.base), s, missing_body).await;
            assert_eq!(seen, none, "{who} {path}");
        }
        let (_, listed) = call(get.clone(), format!("{}/birebirs", c.base), s, None).await;
        assert!(!listed.to_string().contains(&room), "{who}: {listed}");
        assert_eq!(
            ws_refusal(c.port, &room, s).await,
            ws_refusal(c.port, missing, s).await,
            "{who} over the socket"
        );
        assert!(ws_refusal(c.port, &room, s).await.is_some(), "{who}");
    }
    // The words were never touched.
    let (_, read) = call(
        get.clone(),
        format!("{}/rooms/{room}/messages", c.base),
        &c.mehmet,
        None,
    )
    .await;
    assert_eq!(texts(&read), vec!["private words".to_string()]);
}
