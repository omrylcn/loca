//! Birebir: a private loca for exactly two people (PRINCIPLES "Birebir").
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
async fn a_birebir_belongs_to_its_two_people_and_the_master_sees_only_that_it_exists() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("birebir.db");
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
        "an agent never opens a birebir"
    );
    assert_eq!(
        open(outsider.clone(), "ayse").await.0,
        403,
        "no shared table, no birebir"
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
    assert_eq!(again, 200, "the same pair returns to the same birebir");
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
        "an @all cannot widen a birebir"
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
        assert_eq!(status, 404, "{who} must not read a birebir");
        let (status, _) = call(
            get.clone(),
            format!("{base}/rooms/{room}/search?q=secret"),
            s,
            None,
        )
        .await;
        assert_eq!(status, 404, "{who} must not search a birebir");
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
            "a rank's loca list never names a birebir"
        );
    }
    let (_, ranks) = call(get.clone(), format!("{base}/building/ranks"), &master, None).await;
    assert!(!ranks.to_string().contains(&room));
    let (_, residents) = call(get.clone(), format!("{base}/residents"), &master, None).await;
    assert!(!residents.to_string().contains(&room));
    let (_, mine) = call(get.clone(), format!("{base}/rooms"), &mehmet, None).await;
    assert!(
        mine.to_string().contains(&room),
        "the two see their birebir"
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
        "an ordinary loca never takes a birebir id"
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

#[tokio::test]
async fn a_birebir_with_an_agent_reaches_it_by_davet_and_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("birebir-agent.db");
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
    // The agent side got a davet for exactly this room — the call path.
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
    // The Master is a party here, so it reads its own birebir by session.
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

    // Restart: the birebir, its door and its words come back.
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
    let db = dir.path().join("birebir-candidates.db");
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
        "the Master sees the birebir it is part of"
    );
}
