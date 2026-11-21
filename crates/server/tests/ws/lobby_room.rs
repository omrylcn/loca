//! The Lobby room (PRINCIPLES "Lobby"): everyone in the Building lands
//! there first and may talk. A Building member reaches it with their own
//! key or a session taken with it; an invite opens only its own loca; an
//! outsider is turned away. It takes no invitations, carries no extensions,
//! and cannot be deleted.
use super::*;

const MASTER: &str = "lobby-room-master";

async fn admit_key(base: &str, name: &str, kind: &str) -> String {
    let member: Value = reqwest::Client::new()
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
    member["token"].as_str().unwrap().to_string()
}

async fn status(request: reqwest::RequestBuilder) -> u16 {
    request.send().await.unwrap().status().as_u16()
}

#[tokio::test]
async fn every_member_reaches_the_lobby_room_and_nobody_else_does() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _guard) = spawn_server_env(
        MASTER,
        &[
            ("REQUIRE_INVITE", "1".into()),
            ("REQUIRE_SESSIONS", "1".into()),
            (
                "DB_PATH",
                dir.path().join("lobby.db").to_str().unwrap().into(),
            ),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    create_test_room(port, MASTER, "workshop").await;

    // A member signed in with their own key: reads, writes, takes a seat.
    let key = admit_key(&base, "deniz", "agent").await;
    let session = session_with(&base, ("x-room-token", &key), "deniz", None).await;
    assert_eq!(
        status(
            client
                .get(format!("{base}/rooms/lobby/messages"))
                .header("x-session-token", &session)
        )
        .await,
        200
    );
    let posted = client
        .post(format!("{base}/rooms/lobby/messages"))
        .header("x-session-token", &session)
        .json(&serde_json::json!({ "sender": "deniz", "sender_type": "agent", "text": "hello, building" }))
        .send()
        .await
        .unwrap();
    assert!(posted.status().is_success(), "{}", posted.status());
    let mut ws = connect_ws(port, "lobby", &format!("deniz&session={session}"), "agent").await;
    let history = wait_for(&mut ws, |f| f["t"] == "history").await;
    assert!(history.to_string().contains("hello, building"));
    // The member's own key works as the room credential too.
    assert_eq!(
        status(
            client
                .get(format!("{base}/rooms/lobby/messages"))
                .header("x-room-token", &key)
        )
        .await,
        200
    );

    // An invite opens only its loca, not the Lobby room.
    let invite = invite_for(&base, MASTER, "workshop", "guest").await;
    let scoped = session_with(&base, ("x-room-token", &invite), "guest", Some("workshop")).await;
    // A refusal is 401, or the 404 that names nothing.
    let refused = |code: u16| matches!(code, 401 | 404);
    let scoped_read = status(
        client
            .get(format!("{base}/rooms/lobby/messages"))
            .header("x-session-token", &scoped),
    )
    .await;
    assert!(refused(scoped_read), "invite session: {scoped_read}");
    let invite_read = status(
        client
            .get(format!("{base}/rooms/lobby/messages"))
            .header("x-room-token", &invite),
    )
    .await;
    assert!(refused(invite_read), "invite: {invite_read}");
    // An outsider is turned away.
    let outsider = status(client.get(format!("{base}/rooms/lobby/messages"))).await;
    assert!(refused(outsider), "outsider: {outsider}");

    // No invitations (the Lobby's fence answers 403 before the invite
    // handler's 409), no extensions, no deletion.
    let invited = status(
        client
            .post(format!("{base}/rooms/lobby/invites"))
            .header("x-admin-token", MASTER)
            .json(&serde_json::json!({ "name": "deniz" })),
    )
    .await;
    assert!(matches!(invited, 403 | 409), "invite: {invited}");
    assert_eq!(
        status(
            client
                .post(format!("{base}/rooms/lobby/notes"))
                .header("x-session-token", &session)
                .json(&serde_json::json!({ "key": "k", "text": "t" }))
        )
        .await,
        403
    );
    let deleted = status(
        client
            .delete(format!("{base}/rooms/lobby"))
            .header("x-admin-token", MASTER),
    )
    .await;
    assert!(
        matches!(deleted, 403 | 409),
        "the Lobby cannot be deleted: {deleted}"
    );
    assert_eq!(
        status(
            client
                .get(format!("{base}/rooms/lobby/messages"))
                .header("x-session-token", &session)
        )
        .await,
        200
    );
}

#[tokio::test]
async fn the_lobby_room_has_no_seat_limit() {
    let (port, _guard) = spawn_server_env(
        MASTER,
        &[
            ("REQUIRE_INVITE", "1".into()),
            ("LOCA_CAPACITY", "2".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let mut sockets = Vec::new();
    for index in 0..4 {
        let name = format!("crowd{index}");
        let key = admit_key(&base, &name, "agent").await;
        let session = session_with(&base, ("x-room-token", &key), &name, None).await;
        let mut ws = connect_ws(port, "lobby", &format!("{name}&session={session}"), "agent").await;
        wait_for(&mut ws, |f| f["t"] == "history").await;
        sockets.push(ws);
    }
    let members: Vec<Value> = reqwest::Client::new()
        .get(format!("{base}/rooms/lobby/members"))
        .header("x-admin-token", MASTER)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(members.len(), 4, "{members:?}");
}

/// The text of the chat message a mentions socket was handed (`msg` or a
/// one-message `turn`), if the frame is one.
fn delivered_text(frame: &Value) -> Option<String> {
    match frame["t"].as_str()? {
        "msg" => frame["message"]["text"].as_str().map(str::to_string),
        "turn" => frame["messages"][0]["text"].as_str().map(str::to_string),
        _ => None,
    }
}

#[tokio::test]
async fn in_the_lobby_room_an_agent_wakes_by_name_and_only_a_rank_calls_everyone() {
    let (port, _guard) = spawn_server_env(
        MASTER,
        &[
            ("REQUIRE_INVITE", "1".into()),
            ("REQUIRE_SESSIONS", "1".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let agent_key = admit_key(&base, "lobby-bot", "agent").await;
    let member_key = admit_key(&base, "deniz", "agent").await;
    let member = session_with(&base, ("x-room-token", &member_key), "deniz", None).await;

    // The agent's listener: the membership key as the room credential, the
    // mentions filter, no session.
    let mut agent = connect_ws_protocols(
        format!(
            "ws://127.0.0.1:{port}/ws?room=lobby&name=lobby-bot&type=agent&filter=mentions&turn_max=1"
        ),
        &["loca.v1".to_string(), format!("loca.room.{agent_key}")],
    )
    .await;
    wait_for_member(
        &client,
        &base,
        "lobby",
        "lobby-bot",
        Some(("x-admin-token", MASTER)),
    )
    .await;

    let post = |headers: (&'static str, String), body: Value| {
        let client = client.clone();
        let base = base.clone();
        async move {
            let response = client
                .post(format!("{base}/rooms/lobby/messages"))
                .header(headers.0, headers.1)
                .json(&body)
                .send()
                .await
                .unwrap();
            assert!(response.status().is_success(), "{}", response.status());
            response.json::<Value>().await.unwrap()
        }
    };

    // A member's `@all` (text and target) calls nobody in the Lobby: the
    // target is dropped, and the agent's socket is not handed it.
    let hello = post(
        ("x-session-token", member.clone()),
        serde_json::json!({
            "sender": "deniz", "sender_type": "agent",
            "target": "all", "text": "@all hello, building"
        }),
    )
    .await;
    assert!(hello.get("target").is_none(), "{hello}");
    // A message that names the agent wakes it, and it is the first frame the
    // socket gets: the `@all` before it was not delivered.
    post(
        ("x-session-token", member.clone()),
        serde_json::json!({
            "sender": "deniz", "sender_type": "agent", "text": "@lobby-bot are you around?"
        }),
    )
    .await;
    let first = wait_for(&mut agent, |f| delivered_text(f).is_some()).await;
    assert_eq!(
        delivered_text(&first).as_deref(),
        Some("@lobby-bot are you around?"),
        "{first}"
    );

    // A rank's `@all` in the words is stored as `target: all` and calls
    // every agent in the Lobby.
    let master = session_with(&base, ("x-admin-token", MASTER), "master", None).await;
    let call = post(
        ("x-session-token", master),
        serde_json::json!({
            "sender": "master", "sender_type": "user", "text": "@all meeting in five"
        }),
    )
    .await;
    assert_eq!(call["target"], "all", "{call}");
    let woken = wait_for(&mut agent, |f| delivered_text(f).is_some()).await;
    assert_eq!(
        delivered_text(&woken).as_deref(),
        Some("@all meeting in five"),
        "{woken}"
    );

    // A loca is left as it was: a member's `target: all` stays there.
    create_test_room(port, MASTER, "workshop").await;
    let invite = invite_for(&base, MASTER, "workshop", "deniz").await;
    let seated = session_with(&base, ("x-room-token", &invite), "deniz", Some("workshop")).await;
    let in_loca: Value = client
        .post(format!("{base}/rooms/workshop/messages"))
        .header("x-session-token", &seated)
        .json(&serde_json::json!({
            "sender": "deniz", "sender_type": "agent", "target": "all", "text": "@all ready"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(in_loca["target"], "all", "{in_loca}");
}

/// The Lobby door names a caller presenting their own key by that key's
/// member, never by the `?name=` the caller chose: a banned member's key
/// stays out, a caretaker's key stays out (the Lobby is not its room), and
/// without a session nobody speaks there under a name of their choosing.
/// Run in compat mode (no `REQUIRE_SESSIONS`), where the body names the
/// sender elsewhere.
#[tokio::test]
async fn the_lobby_door_names_a_key_by_its_member() {
    let (port, _guard) = spawn_server_env(
        MASTER,
        &[
            ("REQUIRE_INVITE", "1".into()),
            ("LOCA_CARETAKERS", "loca-care".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let deniz = admit_key(&base, "deniz", "agent").await;
    let eve = admit_key(&base, "eve", "user").await;
    let care = admit_key(&base, "loca-care", "agent").await;
    let read = |key: &str| {
        client
            .get(format!("{base}/rooms/lobby/messages"))
            .header("x-room-token", key.to_string())
    };
    assert_eq!(status(read(&deniz)).await, 200);
    assert_eq!(status(read(&eve)).await, 200);

    let banned = status(
        client
            .post(format!("{base}/rooms/lobby/moderate"))
            .header("x-admin-token", MASTER)
            .json(&serde_json::json!({ "action": "ban", "name": "deniz" })),
    )
    .await;
    assert!((200..300).contains(&banned), "ban: {banned}");
    let refused = |code: u16| matches!(code, 401 | 403 | 404);
    let after_ban = status(read(&deniz)).await;
    assert!(refused(after_ban), "a banned member's key: {after_ban}");
    let with_hint = status(
        client
            .get(format!("{base}/rooms/lobby/messages?name=someone-else"))
            .header("x-room-token", &deniz),
    )
    .await;
    assert!(
        refused(with_hint),
        "a banned key with a name hint: {with_hint}"
    );
    let caretaker = status(read(&care)).await;
    assert!(refused(caretaker), "a caretaker's key: {caretaker}");

    let spoofed = status(
        client
            .post(format!("{base}/rooms/lobby/messages"))
            .header("x-room-token", &eve)
            .json(
                &serde_json::json!({ "sender": "ayse", "sender_type": "user", "text": "it is me" }),
            ),
    )
    .await;
    assert_eq!(spoofed, 401, "a Lobby post without a session");
    // A rank's own key is who it says: the Master may still post there.
    let by_master = status(
        client
            .post(format!("{base}/rooms/lobby/messages"))
            .header("x-admin-token", MASTER)
            .json(&serde_json::json!({ "sender": "master", "sender_type": "user", "text": "welcome" })),
    )
    .await;
    assert!(
        (200..300).contains(&by_master),
        "the Master's key: {by_master}"
    );
    let history = read(&eve).send().await.unwrap().text().await.unwrap();
    assert!(!history.contains("it is me"), "{history}");
}

/// A search reads every loca the caller may enter: the query is bounded,
/// and one caller cannot loop it.
#[tokio::test]
async fn building_search_is_bounded_and_rate_limited() {
    let (port, _guard) = spawn_server_env(MASTER, &[("REQUIRE_INVITE", "1".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let key = admit_key(&base, "searcher", "user").await;
    let search = |q: String| {
        client
            .get(format!("{base}/search?q={}", q.replace(' ', "%20")))
            .header("x-room-token", key.clone())
    };
    assert_eq!(status(search("x".repeat(201))).await, 400);
    for n in 0..30 {
        assert_eq!(
            status(search(format!("needle {n}"))).await,
            200,
            "search {n}"
        );
    }
    let limited = search("one more".into()).send().await.unwrap();
    assert_eq!(limited.status().as_u16(), 429);
    assert!(limited.headers().contains_key("retry-after"));
    // Another caller has their own allowance.
    let other = admit_key(&base, "another", "user").await;
    assert_eq!(
        status(
            client
                .get(format!("{base}/search?q=needle"))
                .header("x-room-token", &other)
        )
        .await,
        200
    );
}

/// The Lobby is where everyone waits, not a loca at work: it has no Lead.
#[tokio::test]
async fn the_lobby_room_has_no_lead() {
    let (port, _guard) = spawn_server_env(MASTER, &[("REQUIRE_INVITE", "1".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let key = admit_key(&base, "waiter", "agent").await;
    let session = session_with(&base, ("x-room-token", &key), "waiter", None).await;
    let mut ws = connect_ws(port, "lobby", &format!("waiter&session={session}"), "agent").await;
    wait_for(&mut ws, |f| f["t"] == "history").await;
    let c = reqwest::Client::new();
    let named = c
        .post(format!("{base}/rooms/lobby/lead"))
        .header("x-admin-token", MASTER)
        .json(&serde_json::json!({ "lead": "waiter" }))
        .send()
        .await
        .unwrap();
    let status = named.status();
    let why = named.text().await.unwrap();
    assert_eq!(status, 403, "{why}");
    assert!(why.contains("only messages"), "{why}");
    let settings: Value = c
        .get(format!("{base}/rooms/lobby/settings"))
        .header("x-admin-token", MASTER)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(settings["lead"].is_null(), "{settings}");
}
