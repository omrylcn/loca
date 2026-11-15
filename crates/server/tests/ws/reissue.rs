//! Re-issue: `POST /rooms/:id/call {"name", "reissue": true}` gives a member a
//! fresh invite for the seat it already holds. The old one ends with its
//! sessions and socket, the new one arrives over the Lobby like a call, and
//! the seat never moves. Without the flag the call answers as it always did.
use super::*;

const MASTER: &str = "reissue-master-key";

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

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

async fn call(base: &str, room: &str, auth: (&str, &str), body: Value) -> (u16, Value) {
    send(
        reqwest::Method::POST,
        format!("{base}/rooms/{room}/call"),
        &[auth],
        Some(body),
    )
    .await
}

async fn reissue(base: &str, room: &str, auth: (&str, &str), name: &str) -> (u16, Value) {
    call(
        base,
        room,
        auth,
        serde_json::json!({ "name": name, "reissue": true }),
    )
    .await
}

/// Admit a member and return its `mb_` key.
async fn member_key(base: &str, name: &str) -> String {
    let (status, v) = send(
        reqwest::Method::POST,
        format!("{base}/members"),
        &[("x-admin-token", MASTER)],
        Some(serde_json::json!({ "name": name, "kind": "agent" })),
    )
    .await;
    assert!(status < 300, "member {name}: {status} {v}");
    v["token"].as_str().unwrap().to_string()
}

async fn take_session(base: &str, invite: &str, name: &str) -> (u16, Value) {
    send(
        reqwest::Method::POST,
        format!("{base}/sessions"),
        &[("x-room-token", invite)],
        Some(serde_json::json!({ "name": name, "kind": "agent" })),
    )
    .await
}

async fn read_messages(base: &str, room: &str, header: (&str, &str)) -> u16 {
    send(
        reqwest::Method::GET,
        format!("{base}/rooms/{room}/messages"),
        &[header],
        None,
    )
    .await
    .0
}

async fn live_invites(base: &str, room: &str) -> Vec<Value> {
    let (status, v) = send(
        reqwest::Method::GET,
        format!("{base}/rooms/{room}/invites"),
        &[("x-admin-token", MASTER)],
        None,
    )
    .await;
    assert_eq!(status, 200, "{v}");
    v.as_array().unwrap().clone()
}

async fn lobby(port: u16, key: &str) -> Socket {
    connect_ws_protocols(
        format!("ws://127.0.0.1:{port}/lobby/ws"),
        &["loca.v1".into(), format!("loca.membership.{key}")],
    )
    .await
}

async fn loca_socket(port: u16, room: &str, name: &str, invite: &str) -> Socket {
    connect_ws_protocols(
        format!("ws://127.0.0.1:{port}/ws?room={room}&name={name}&type=agent"),
        &["loca.v1".into(), format!("loca.room.{invite}")],
    )
    .await
}

/// Every `called` frame that arrives within `window`.
async fn calls_within(socket: &mut Socket, window: Duration) -> Vec<Value> {
    let mut seen = Vec::new();
    let deadline = tokio::time::sleep(window);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => return seen,
            item = socket.next() => match item {
                Some(Ok(WsMessage::Text(text))) => {
                    let v: Value = serde_json::from_str(&text).unwrap();
                    if v["t"] == "called" {
                        seen.push(v);
                    }
                }
                Some(Ok(_)) => {}
                _ => return seen,
            }
        }
    }
}

/// The frames a socket receives until the server closes it.
async fn frames_until_closed(socket: &mut Socket) -> Vec<Value> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut seen = Vec::new();
        while let Some(item) = socket.next().await {
            match item {
                Ok(WsMessage::Text(text)) => seen.push(serde_json::from_str(&text).unwrap()),
                Ok(WsMessage::Close(_)) | Err(_) => break,
                Ok(_) => {}
            }
        }
        seen
    })
    .await
    .expect("the socket seated with the old invite must be closed")
}

async fn closed_building(db: &std::path::Path, extra: &[(&str, String)]) -> (u16, ServerGuard) {
    let mut env = vec![
        ("DB_PATH", db.to_str().unwrap().to_string()),
        ("REQUIRE_INVITE", "1".to_string()),
        ("LEGACY_WS_QUERY_AUTH", "0".to_string()),
    ];
    env.extend(extra.iter().cloned());
    spawn_server_env(MASTER, &env).await
}

/// Without `reissue` the call route answers exactly as before: a first call
/// mints, a second says `already`, and `reissue: false` is the same call.
#[tokio::test]
async fn a_call_without_reissue_answers_as_before() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _guard) = closed_building(&dir.path().join("plain.db"), &[]).await;
    create_test_room(port, MASTER, "parlor").await;
    let base = format!("http://127.0.0.1:{port}");
    member_key(&base, "worker").await;
    let admin = ("x-admin-token", MASTER);

    let (status, first) = call(
        &base,
        "parlor",
        admin,
        serde_json::json!({ "name": "worker" }),
    )
    .await;
    assert_eq!(status, 201, "{first}");
    let token = first["token"].as_str().unwrap().to_string();
    assert!(token.starts_with("dv_"));
    assert_eq!(
        first,
        serde_json::json!({ "name": "worker", "loca": "parlor", "token": token }),
        "a first call answers name, loca and the invite, nothing else"
    );

    for body in [
        serde_json::json!({ "name": "worker" }),
        serde_json::json!({ "name": "worker", "reissue": false }),
    ] {
        let (status, again) = call(&base, "parlor", admin, body).await;
        assert_eq!(status, 200, "{again}");
        assert_eq!(
            again,
            serde_json::json!({ "already": true, "name": "worker" })
        );
    }
    let invites = live_invites(&base, "parlor").await;
    assert_eq!(invites.len(), 1, "a second call mints nothing");
    assert_eq!(
        read_messages(&base, "parlor", ("x-room-token", &token)).await,
        200,
        "the first invite is untouched"
    );

    let (status, _) = call(
        &base,
        "parlor",
        admin,
        serde_json::json!({ "name": "stranger" }),
    )
    .await;
    assert_eq!(status, 404, "an unknown name is not called in");
    let (status, _) = call(
        &base,
        "parlor",
        ("x-room-token", &token),
        serde_json::json!({ "name": "worker" }),
    )
    .await;
    assert_eq!(status, 401, "an invite holder calls nobody in");
}

/// The whole re-issue, against the real binary: the old invite stops opening
/// the session door and the loca socket, the socket seated with it is closed
/// and its session ends, the Lobby receives exactly one `called` with the new
/// invite, the new invite works, the loca still lists one invite for the member
/// and the key ledger shows one ended and one issued.
#[tokio::test]
async fn reissue_ends_the_old_invite_and_delivers_a_fresh_one() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _guard) = closed_building(&dir.path().join("reissue.db"), &[]).await;
    create_test_room(port, MASTER, "parlor").await;
    let base = format!("http://127.0.0.1:{port}");
    let key = member_key(&base, "worker").await;
    let admin = ("x-admin-token", MASTER);
    let (status, first) = call(
        &base,
        "parlor",
        admin,
        serde_json::json!({ "name": "worker" }),
    )
    .await;
    assert_eq!(status, 201, "{first}");
    let old = first["token"].as_str().unwrap().to_string();

    let mut lobby = lobby(port, &key).await;
    wait_for(&mut lobby, |v| v["t"] == "lobby_ready").await;
    let replay = wait_for(&mut lobby, |v| v["t"] == "called").await;
    assert_eq!(replay["token"], old.as_str());

    let (status, session) = take_session(&base, &old, "worker").await;
    assert_eq!(status, 201);
    let old_session = session["session_token"].as_str().unwrap().to_string();
    let mut seated = loca_socket(port, "parlor", "worker", &old).await;
    wait_for(&mut seated, |v| v["t"] == "history").await;

    let (status, fresh) = reissue(&base, "parlor", admin, "worker").await;
    assert_eq!(status, 201, "{fresh}");
    let new = fresh["token"].as_str().unwrap().to_string();
    assert!(new.starts_with("dv_"));
    assert_ne!(new, old);
    assert_eq!(
        fresh,
        serde_json::json!({ "name": "worker", "loca": "parlor", "token": new }),
        "a re-issue answers with a first call's shape"
    );

    // The socket seated with the old invite closes, and is not told `kicked`:
    // a listener forgets a loca's invite on `kicked`, which would race the
    // Lobby's `called` carrying the fresh one.
    let closing = frames_until_closed(&mut seated).await;
    assert!(
        closing.iter().all(|f| f["t"] == "members"),
        "the old seat closes with no kick and nothing of the loca: {closing:?}"
    );

    let called = calls_within(&mut lobby, Duration::from_millis(800)).await;
    assert_eq!(called.len(), 1, "exactly one called: {called:?}");
    assert_eq!(called[0]["room"], "parlor");
    assert_eq!(called[0]["token"], new.as_str());

    // The old invite is dead at every door; its session ended with it.
    assert_eq!(take_session(&base, &old, "worker").await.0, 401);
    assert_eq!(
        read_messages(&base, "parlor", ("x-room-token", &old)).await,
        401
    );
    assert_eq!(
        read_messages(&base, "parlor", ("x-session-token", &old_session)).await,
        401
    );
    assert!(!ws_protocol_can(port, "parlor", &old).await);

    // The new one works.
    let (status, session) = take_session(&base, &new, "worker").await;
    assert_eq!(status, 201);
    let new_session = session["session_token"].as_str().unwrap();
    assert_eq!(
        read_messages(&base, "parlor", ("x-session-token", new_session)).await,
        200
    );
    assert_eq!(
        read_messages(&base, "parlor", ("x-room-token", &new)).await,
        200
    );
    let mut reseated = loca_socket(port, "parlor", "worker", &new).await;
    wait_for(&mut reseated, |v| v["t"] == "history").await;

    // One invite for the member, still signed by the one who issued it.
    let invites = live_invites(&base, "parlor").await;
    assert_eq!(invites.len(), 1, "{invites:?}");
    assert_eq!(invites[0]["name"], "worker");
    assert_eq!(invites[0]["issued_by"], "master");

    // The ledger: the old invite ended, the new one issued, never two live.
    let (status, ledger) = send(reqwest::Method::GET, format!("{base}/keys"), &[admin], None).await;
    assert_eq!(status, 200, "{ledger}");
    let text = ledger.to_string();
    assert!(
        !text.contains(&old) && !text.contains(&new),
        "no invite in the ledger"
    );
    let rows: Vec<&Value> = ledger
        .as_array()
        .unwrap()
        .iter()
        .filter(|k| k["kind"] == "davet" && k["loca"] == "parlor" && k["for_name"] == "worker")
        .collect();
    let mut states: Vec<&str> = rows.iter().map(|k| k["state"].as_str().unwrap()).collect();
    states.sort();
    assert_eq!(states, ["active", "revoked"], "{rows:?}");
}

/// A re-issue keeps the seat: a full loca stays full, the member keeps its
/// place, and a call for somebody else racing the re-issue never takes it.
#[tokio::test]
async fn reissue_in_a_full_loca_keeps_the_seat() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _guard) = closed_building(
        &dir.path().join("full.db"),
        &[("LOCA_CAPACITY", "2".to_string())],
    )
    .await;
    create_test_room(port, MASTER, "packed").await;
    let base = format!("http://127.0.0.1:{port}");
    let admin = ("x-admin-token", MASTER);
    for name in ["sat-a", "sat-b", "waiting"] {
        member_key(&base, name).await;
    }
    for name in ["sat-a", "sat-b"] {
        let (status, v) = call(&base, "packed", admin, serde_json::json!({ "name": name })).await;
        assert_eq!(status, 201, "{v}");
    }
    let (status, _) = call(
        &base,
        "packed",
        admin,
        serde_json::json!({ "name": "waiting" }),
    )
    .await;
    assert_eq!(status, 409, "the loca is full before the re-issue");

    for round in 0..10 {
        let (re, other) = tokio::join!(
            reissue(&base, "packed", admin, "sat-a"),
            call(
                &base,
                "packed",
                admin,
                serde_json::json!({ "name": "waiting" })
            ),
        );
        assert_eq!(re.0, 201, "round {round}: a full loca re-issues: {}", re.1);
        assert_eq!(
            other.0, 409,
            "round {round}: the re-issued seat is never free: {}",
            other.1
        );
        let invites = live_invites(&base, "packed").await;
        let mut names: Vec<&str> = invites
            .iter()
            .map(|d| d["name"].as_str().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["sat-a", "sat-b"], "round {round}");
    }
}

/// When the store refuses the write, nothing changes: the old invite, its
/// session and its socket keep working, and the Lobby hears nothing. Once the
/// store accepts writes again the same re-issue succeeds.
#[tokio::test]
async fn a_failed_reissue_write_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("fail.db");
    let (port, _guard) = closed_building(&db, &[]).await;
    create_test_room(port, MASTER, "parlor").await;
    let base = format!("http://127.0.0.1:{port}");
    let key = member_key(&base, "worker").await;
    let admin = ("x-admin-token", MASTER);
    let (_, first) = call(
        &base,
        "parlor",
        admin,
        serde_json::json!({ "name": "worker" }),
    )
    .await;
    let old = first["token"].as_str().unwrap().to_string();
    let (_, session) = take_session(&base, &old, "worker").await;
    let old_session = session["session_token"].as_str().unwrap().to_string();
    let mut lobby = lobby(port, &key).await;
    // The session above was the invite's first use, so the Lobby is not sent
    // it again: `lobby_ready` is all this connect gets.
    wait_for(&mut lobby, |v| v["t"] == "lobby_ready").await;
    let mut seated = loca_socket(port, "parlor", "worker", &old).await;
    wait_for(&mut seated, |v| v["t"] == "history").await;

    let sql = rusqlite::Connection::open(&db).unwrap();
    for trigger in [
        "CREATE TRIGGER fail_reissue BEFORE INSERT ON invites WHEN NEW.name = 'worker'
         BEGIN SELECT RAISE(ABORT, 'injected write failure'); END;",
        "CREATE TRIGGER fail_reissue BEFORE UPDATE ON invites WHEN OLD.name = 'worker'
         BEGIN SELECT RAISE(ABORT, 'injected write failure'); END;",
    ] {
        sql.execute_batch(trigger).unwrap();
        let (status, v) = reissue(&base, "parlor", admin, "worker").await;
        assert_eq!(status, 503, "a failed write is reported: {v}");
        sql.execute_batch("DROP TRIGGER fail_reissue").unwrap();

        assert_eq!(
            read_messages(&base, "parlor", ("x-room-token", &old)).await,
            200
        );
        assert_eq!(
            read_messages(&base, "parlor", ("x-session-token", &old_session)).await,
            200
        );
        let rows: Vec<(String, Option<i64>)> = sql
            .prepare("SELECT id, revoked_at FROM invites WHERE name = 'worker'")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            rows,
            vec![(invite_ref(&old), None)],
            "nothing reached the table"
        );
        assert_eq!(live_invites(&base, "parlor").await.len(), 1);
        assert!(
            calls_within(&mut lobby, Duration::from_millis(400))
                .await
                .is_empty(),
            "no frame is sent for a re-issue that did not happen"
        );
    }
    // The seated socket is still open and still carries the loca.
    let (status, v) = send(
        reqwest::Method::POST,
        format!("{base}/rooms/parlor/messages"),
        &[admin],
        Some(serde_json::json!({ "sender": "operator", "sender_type": "user", "text": "still here" })),
    )
    .await;
    assert!(status < 300, "{status} {v}");
    wait_for(&mut seated, |v| {
        v["t"] == "msg" && v["message"]["text"] == "still here"
    })
    .await;

    let (status, fresh) = reissue(&base, "parlor", admin, "worker").await;
    assert_eq!(status, 201, "{fresh}");
    assert_eq!(
        read_messages(&base, "parlor", ("x-room-token", &old)).await,
        401
    );
}

/// A restart rebuilds from the store: the re-issued invite stays dead and the
/// fresh one opens the loca.
#[tokio::test]
async fn a_restart_keeps_the_old_invite_dead_and_the_new_one_working() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("restart.db");
    let (old, new) = {
        let (port, guard) = closed_building(&db, &[]).await;
        create_test_room(port, MASTER, "parlor").await;
        let base = format!("http://127.0.0.1:{port}");
        member_key(&base, "worker").await;
        let admin = ("x-admin-token", MASTER);
        let (_, first) = call(
            &base,
            "parlor",
            admin,
            serde_json::json!({ "name": "worker" }),
        )
        .await;
        let (status, fresh) = reissue(&base, "parlor", admin, "worker").await;
        assert_eq!(status, 201, "{fresh}");
        drop(guard);
        wait_for_server_shutdown(port).await;
        (
            first["token"].as_str().unwrap().to_string(),
            fresh["token"].as_str().unwrap().to_string(),
        )
    };
    let (port, _guard) = closed_building(&db, &[]).await;
    let base = format!("http://127.0.0.1:{port}");
    assert_eq!(
        read_messages(&base, "parlor", ("x-room-token", &old)).await,
        401
    );
    assert_eq!(take_session(&base, &old, "worker").await.0, 401);
    assert_eq!(
        read_messages(&base, "parlor", ("x-room-token", &new)).await,
        200
    );
    assert_eq!(take_session(&base, &new, "worker").await.0, 201);
    assert_eq!(live_invites(&base, "parlor").await.len(), 1);
}

/// Re-issuing ends an invite, so it takes the authority ending it takes: a
/// Smaster cannot re-issue the Master's invite (the revoke route's 403), and
/// may re-issue its own. The Master may re-issue the Smaster's, which stays
/// signed by the Smaster.
#[tokio::test]
async fn a_smaster_reissues_only_what_it_may_end() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _guard) = closed_building(&dir.path().join("rank.db"), &[]).await;
    create_test_room(port, MASTER, "parlor").await;
    let base = format!("http://127.0.0.1:{port}");
    let admin = ("x-admin-token", MASTER);
    let (status, deputy) = send(
        reqwest::Method::POST,
        format!("{base}/smasters"),
        &[admin],
        Some(serde_json::json!({ "name": "deputy" })),
    )
    .await;
    assert!(status < 300, "{deputy}");
    let deputy_key = deputy["token"].as_str().unwrap().to_string();
    let (status, s) = send(
        reqwest::Method::POST,
        format!("{base}/sessions"),
        &[("x-admin-token", &deputy_key)],
        Some(serde_json::json!({ "name": "deputy", "kind": "user" })),
    )
    .await;
    assert!(status < 300, "{s}");
    let smaster = s["session_token"].as_str().unwrap().to_string();
    let as_smaster = ("x-session-token", smaster.as_str());

    member_key(&base, "by-master").await;
    member_key(&base, "by-deputy").await;
    let (_, m) = call(
        &base,
        "parlor",
        admin,
        serde_json::json!({ "name": "by-master" }),
    )
    .await;
    let master_invite = m["token"].as_str().unwrap().to_string();
    let (status, d) = call(
        &base,
        "parlor",
        as_smaster,
        serde_json::json!({ "name": "by-deputy" }),
    )
    .await;
    assert_eq!(status, 201, "{d}");

    // The revoke route's answer for the same invite, for comparison.
    let invites = live_invites(&base, "parlor").await;
    let master_id = invites.iter().find(|d| d["name"] == "by-master").unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (revoke_status, _) = send(
        reqwest::Method::DELETE,
        format!("{base}/rooms/parlor/invites/{master_id}"),
        &[as_smaster],
        None,
    )
    .await;
    assert_eq!(revoke_status, 403);

    let (status, refused) = reissue(&base, "parlor", as_smaster, "by-master").await;
    assert_eq!(
        status, revoke_status,
        "the revoke route's refusal: {refused}"
    );
    assert_eq!(
        read_messages(&base, "parlor", ("x-room-token", &master_invite)).await,
        200,
        "the Master's invite is untouched"
    );
    assert_eq!(live_invites(&base, "parlor").await.len(), 2);

    let (status, own) = reissue(&base, "parlor", as_smaster, "by-deputy").await;
    assert_eq!(status, 201, "{own}");
    let (status, again) = reissue(&base, "parlor", admin, "by-deputy").await;
    assert_eq!(status, 201, "{again}");
    let invites = live_invites(&base, "parlor").await;
    let deputy_row = invites.iter().find(|d| d["name"] == "by-deputy").unwrap();
    assert_eq!(
        deputy_row["issued_by"], "smaster:deputy",
        "a re-issue keeps who issued the invite"
    );

    // Without Building rank there is no call and no re-issue.
    let (status, _) = reissue(
        &base,
        "parlor",
        ("x-room-token", &master_invite),
        "by-master",
    )
    .await;
    assert_eq!(status, 401);
}

/// `reissue: true` for a member who holds no invite in this loca is refused;
/// it never becomes a first call. A malformed flag is refused too.
#[tokio::test]
async fn reissue_without_an_invite_is_refused_not_a_call() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _guard) = closed_building(&dir.path().join("none.db"), &[]).await;
    create_test_room(port, MASTER, "parlor").await;
    let base = format!("http://127.0.0.1:{port}");
    let key = member_key(&base, "worker").await;
    let admin = ("x-admin-token", MASTER);
    let mut lobby = lobby(port, &key).await;
    wait_for(&mut lobby, |v| v["t"] == "lobby_ready").await;

    let (status, v) = reissue(&base, "parlor", admin, "worker").await;
    assert_eq!(status, 404, "{v}");
    assert!(
        v.as_str().unwrap().contains("holds no davet"),
        "the refusal says why: {v}"
    );
    assert!(
        live_invites(&base, "parlor").await.is_empty(),
        "nothing minted"
    );
    assert!(calls_within(&mut lobby, Duration::from_millis(400))
        .await
        .is_empty());

    let (status, _) = call(
        &base,
        "parlor",
        admin,
        serde_json::json!({ "name": "worker", "reissue": "yes" }),
    )
    .await;
    assert_eq!(status, 400, "the flag is a boolean");
    assert!(live_invites(&base, "parlor").await.is_empty());
}

/// A suspended member is refused a re-issue exactly as it is refused a call.
#[tokio::test]
async fn a_suspended_member_is_refused_a_reissue_like_a_call() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _guard) = closed_building(&dir.path().join("suspended.db"), &[]).await;
    create_test_room(port, MASTER, "parlor").await;
    let base = format!("http://127.0.0.1:{port}");
    member_key(&base, "worker").await;
    let admin = ("x-admin-token", MASTER);
    let (_, first) = call(
        &base,
        "parlor",
        admin,
        serde_json::json!({ "name": "worker" }),
    )
    .await;
    let old = first["token"].as_str().unwrap().to_string();
    let (_, residents) = send(
        reqwest::Method::GET,
        format!("{base}/residents"),
        &[admin],
        None,
    )
    .await;
    let id = residents
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "worker")
        .unwrap()["membership_id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, v) = send(
        reqwest::Method::POST,
        format!("{base}/members/{id}/suspension"),
        &[admin],
        Some(serde_json::json!({ "suspended": true })),
    )
    .await;
    assert!(status < 300, "{v}");

    let called = call(
        &base,
        "parlor",
        admin,
        serde_json::json!({ "name": "worker" }),
    )
    .await;
    let reissued = reissue(&base, "parlor", admin, "worker").await;
    assert!(called.0 >= 400, "{called:?}");
    assert_eq!(reissued, called, "the same refusal as a call");
    assert_eq!(
        read_messages(&base, "parlor", ("x-room-token", &old)).await,
        401
    );
    assert!(live_invites(&base, "parlor").await.is_empty());
}
