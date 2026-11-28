//! A mentat room over HTTP, with real sessions: it is founded as one and
//! has no Operator, it waits for its mentat, seated askers open, join and
//! withdraw requests under their verified names, the line lets in only the
//! askers whose turn it is (no rank writes past it), only they, the mentat
//! or a rank who manages the room end a consultation, the Building lists the
//! room, and the line survives a restart.

use super::lead_tasks::{answer, Loca, Seat, ROOT};
use super::*;

const TOPIC: &str = "sessions, keys and the release order";

async fn as_root(
    loca: &Loca,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let mut request = loca
        .client
        .request(method, format!("{}/rooms/proj{path}", loca.base))
        .header("x-admin-token", ROOT);
    if let Some(body) = body {
        request = request.json(&body);
    }
    answer(request).await
}

async fn as_seat(
    loca: &Loca,
    seat: &Seat,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let mut request = seat.on(loca
        .client
        .request(method, format!("{}/rooms/proj{path}", loca.base)));
    if let Some(body) = body {
        request = request.json(&body);
    }
    answer(request).await
}

async fn say(loca: &Loca, seat: &Seat, name: &str, text: &str) -> (u16, Value) {
    as_seat(
        loca,
        seat,
        reqwest::Method::POST,
        "/messages",
        Some(serde_json::json!({"sender": name, "sender_type": "agent", "text": text})),
    )
    .await
}

async fn ask(loca: &Loca, seat: &Seat, topic: &str) -> (u16, Value) {
    as_seat(
        loca,
        seat,
        reqwest::Method::POST,
        "/mentat/requests",
        Some(serde_json::json!({ "topic": topic })),
    )
    .await
}

#[tokio::test]
async fn a_mentat_room_is_founded_waits_for_its_mentat_and_has_no_operator() {
    use reqwest::Method;
    let loca = Loca::open_mentat(TOPIC).await;
    let sage = loca.seat("sage").await;
    let ada = loca.seat("ada").await;

    let (status, view) = as_seat(&loca, &ada, Method::GET, "/mentat", None).await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["state"]["mentat"]["topic"], TOPIC);
    assert!(view["state"]["mentat"]["mentat"].is_null());
    assert!(
        view["state"]["mentat"].get("founder").is_none(),
        "the founder's principal is not shown"
    );
    assert_eq!(view["can_manage"], false);

    // The first rule: nobody asks a room that has no mentat.
    let (status, refusal) = ask(&loca, &ada, "how do sessions renew?").await;
    assert_eq!(status, 409, "{refusal}");
    assert!(refusal.as_str().unwrap().contains("no mentat yet"));

    // It has no Operator seat, and takes no Loca Board.
    let (status, operator) = as_root(&loca, Method::GET, "/operators", None).await;
    assert_eq!(status, 200);
    assert!(operator["appointed"].is_null(), "{operator}");
    let (status, _) = as_root(&loca, Method::PUT, "/extensions/board", None).await;
    assert_eq!(status, 409);
    let (status, _) = as_root(&loca, Method::PUT, "/extensions/notes", None).await;
    assert_eq!(status, 200, "other extensions are its managers' choice");

    // A seated agent does not appoint; the Master does, by a name the
    // Building knows, and only an agent's.
    let (status, _) = as_seat(
        &loca,
        &ada,
        Method::PUT,
        "/mentat",
        Some(serde_json::json!({"mentat": "ada"})),
    )
    .await;
    assert_eq!(status, 403);
    let (status, _) = as_root(
        &loca,
        Method::PUT,
        "/mentat",
        Some(serde_json::json!({"mentat": "nobody-here"})),
    )
    .await;
    assert_eq!(status, 404);
    let (status, state) = as_root(
        &loca,
        Method::PUT,
        "/mentat",
        Some(serde_json::json!({"mentat": "sage", "limits": {"questions": 3}})),
    )
    .await;
    assert_eq!(status, 200, "{state}");
    assert_eq!(state["mentat"]["mentat"], "sage");
    assert_eq!(state["mentat"]["limits"]["questions"], 3);
    assert_eq!(state["mentat"]["limits"]["session_secs"], 600);
    assert_eq!(state["mentat"]["limits"]["call_secs"], 120);

    // The Building's directory lists it; an ordinary loca never becomes one.
    let (status, list) = answer(
        loca.client
            .get(format!("{}/mentats", loca.base))
            .header("x-admin-token", ROOT),
    )
    .await;
    assert_eq!(status, 200, "{list}");
    assert_eq!(list["mentats"][0]["room"], "proj");
    assert_eq!(list["mentats"][0]["topic"], TOPIC);
    assert_eq!(list["mentats"][0]["mentat"], "sage");
    let (status, _) = answer(loca.client.get(format!("{}/mentats", loca.base))).await;
    assert_eq!(status, 403, "strangers see no directory");
    let (status, _) = answer(sage.on(loca.client.get(format!("{}/mentats", loca.base)))).await;
    assert_eq!(
        status, 403,
        "a session for one loca is not a Building identity"
    );
    create_test_room(
        loca.base.rsplit(':').next().unwrap().parse().unwrap(),
        ROOT,
        "plain",
    )
    .await;
    let (status, _) = answer(
        loca.client
            .put(format!("{}/rooms/plain/mentat", loca.base))
            .header("x-admin-token", ROOT)
            .json(&serde_json::json!({"mentat": "sage"})),
    )
    .await;
    assert_eq!(status, 404);

    // Taking the appointment back keeps the room, waiting again.
    let (status, state) = as_root(&loca, Method::DELETE, "/mentat", None).await;
    assert_eq!(status, 200, "{state}");
    assert!(state["mentat"]["mentat"].is_null());
    let (status, _) = ask(&loca, &ada, "how do sessions renew?").await;
    assert_eq!(status, 409);
}

#[tokio::test]
async fn askers_take_turns_through_the_api_and_only_the_right_people_act() {
    use reqwest::Method;
    let loca = Loca::open_mentat(TOPIC).await;
    let sage = loca.seat("sage").await;
    let ada = loca.seat("ada").await;
    let bob = loca.seat("bob").await;
    let cem = loca.seat("cem").await;
    let (status, _) = as_root(
        &loca,
        Method::PUT,
        "/mentat",
        Some(serde_json::json!({"mentat": "sage"})),
    )
    .await;
    assert_eq!(status, 200);

    // Ada's request is called at once, under her verified name; Bob waits.
    let (status, request) = ask(&loca, &ada, "how sessions renew").await;
    assert_eq!(status, 201, "{request}");
    assert_eq!(request["status"], "called");
    assert_eq!(request["owners"], serde_json::json!(["ada"]));
    let (status, request) = ask(&loca, &bob, "the release order").await;
    assert_eq!(status, 201);
    assert_eq!(request["status"], "waiting");
    let bob_id = request["id"].as_u64().unwrap();
    let (status, _) = ask(&loca, &bob, "something else again").await;
    assert_eq!(status, 409, "one open request per asker");
    let (status, _) = ask(&loca, &cem, "short").await;
    assert_eq!(status, 400, "a topic says what it is about");

    // The room lets in only the mentat and the asker inside, and says why.
    let (status, refusal) = say(&loca, &bob, "bob", "can I?").await;
    assert_eq!(status, 403);
    assert!(
        refusal.as_str().unwrap().contains("mentat room"),
        "{refusal}"
    );
    assert_eq!(
        say(&loca, &ada, "ada", "how do sessions renew?").await.0,
        201
    );
    assert_eq!(
        say(&loca, &ada, "ada", "and when do they expire?").await.0,
        201
    );
    assert_eq!(say(&loca, &sage, "sage", "on the 401").await.0, 201);
    let (_, view) = as_seat(&loca, &bob, Method::GET, "/mentat", None).await;
    assert_eq!(view["state"]["current"]["status"], "active");
    assert_eq!(
        view["state"]["current"]["questions"], 1,
        "one question in two messages"
    );
    assert_eq!(view["can_manage"], false);
    let lines = loca.announcements().await;
    assert!(
        lines.iter().any(|(target, text)| target == "ada"
            && text.starts_with("@ada: your turn with the mentat, sage")),
        "{lines:?}"
    );

    // No rank writes past the line.
    let (status, _) = answer(
        loca.client
            .post(format!("{}/rooms/proj/messages", loca.base))
            .header("x-admin-token", ROOT)
            .json(&serde_json::json!({"sender": "master", "sender_type": "user", "text": "let me add"})),
    )
    .await;
    assert_eq!(status, 403);

    // Cem joins Bob's request, then takes it back.
    let (status, joined) = as_seat(
        &loca,
        &cem,
        Method::POST,
        &format!("/mentat/requests/{bob_id}/join"),
        None,
    )
    .await;
    assert_eq!(status, 200, "{joined}");
    assert_eq!(joined["owners"], serde_json::json!(["bob", "cem"]));
    let (status, left) = as_seat(
        &loca,
        &cem,
        Method::POST,
        &format!("/mentat/requests/{bob_id}/withdraw"),
        None,
    )
    .await;
    assert_eq!(status, 200, "{left}");
    assert_eq!(left["owners"], serde_json::json!(["bob"]));

    // Bob cannot end Ada's consultation; Ada can, saying it was answered.
    let (status, _) = as_seat(&loca, &bob, Method::POST, "/mentat/end", None).await;
    assert_eq!(status, 403);
    let (status, ended) = as_seat(
        &loca,
        &ada,
        Method::POST,
        "/mentat/end",
        Some(serde_json::json!({"answered": "partly"})),
    )
    .await;
    assert_eq!(status, 200, "{ended}");
    assert_eq!(ended["end_reason"], "asker");
    assert_eq!(ended["answered"], "partly");
    let (_, history) = as_seat(&loca, &bob, Method::GET, "/mentat/history", None).await;
    assert_eq!(history["requests"][0]["answered"], "partly", "{history}");
    let (_, view) = as_root(&loca, Method::GET, "/mentat", None).await;
    assert_eq!(
        view["state"]["current"]["owners"],
        serde_json::json!(["bob"])
    );
    assert_eq!(view["can_manage"], true);
    assert_eq!(say(&loca, &ada, "ada", "one more").await.0, 403);

    // The mentat pauses its line and resumes it; a bystander cannot.
    let (status, _) = as_seat(&loca, &ada, Method::POST, "/mentat/pause", None).await;
    assert_eq!(status, 403);
    let (status, paused) = as_seat(&loca, &sage, Method::POST, "/mentat/pause", None).await;
    assert_eq!(status, 200, "{paused}");
    assert_eq!(paused["paused"], true);
    let (status, resumed) = as_seat(&loca, &sage, Method::POST, "/mentat/resume", None).await;
    assert_eq!(status, 200);
    assert_eq!(resumed["paused"], false);

    // A rank who manages the room ends a consultation.
    let (status, ended) = as_root(&loca, Method::POST, "/mentat/end", None).await;
    assert_eq!(status, 200);
    assert_eq!(ended["end_reason"], "rank");
}

#[tokio::test]
async fn the_line_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("mentat.db").to_string_lossy().to_string();
    let client = reqwest::Client::new();
    {
        let (port, _guard) = spawn_server_env(ROOT, &[("DB_PATH", db.clone())]).await;
        let base = format!("http://127.0.0.1:{port}");
        let founded = client
            .post(format!("{base}/rooms"))
            .header("x-admin-token", ROOT)
            .json(&serde_json::json!({"name": "proj", "mentat": {"topic": TOPIC}}))
            .send()
            .await
            .unwrap();
        assert_eq!(founded.status(), 201);
        let admitted = client
            .post(format!("{base}/members"))
            .header("x-admin-token", ROOT)
            .json(&serde_json::json!({"name": "sage", "kind": "agent"}))
            .send()
            .await
            .unwrap();
        assert!(admitted.status().is_success());
        let put = client
            .put(format!("{base}/rooms/proj/mentat"))
            .header("x-admin-token", ROOT)
            .json(&serde_json::json!({"mentat": "sage"}))
            .send()
            .await
            .unwrap();
        assert_eq!(put.status(), 200);
        // The Master asks too, in the line like anyone.
        let opened = client
            .post(format!("{base}/rooms/proj/mentat/requests"))
            .header("x-admin-token", ROOT)
            .json(&serde_json::json!({ "topic": "the first question here" }))
            .send()
            .await
            .unwrap();
        assert_eq!(opened.status(), 201);
    }
    let (port, _guard) = spawn_server_env(ROOT, &[("DB_PATH", db)]).await;
    let base = format!("http://127.0.0.1:{port}");
    let view: Value = client
        .get(format!("{base}/rooms/proj/mentat"))
        .header("x-admin-token", ROOT)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(view["state"]["mentat"]["mentat"], "sage");
    assert_eq!(view["state"]["mentat"]["topic"], TOPIC);
    assert_eq!(view["state"]["current"]["id"], 1);
    assert_eq!(view["state"]["current"]["status"], "called");
    assert_eq!(view["state"]["current"]["topic"], "the first question here");
    let mode: Value = client
        .get(format!("{base}/rooms/proj/mode"))
        .header("x-admin-token", ROOT)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(mode["mode"], "restricted", "{mode}");
}
