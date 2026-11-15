//! The Lobby against the real binary: `lobby_ready` names seats, never
//! invites, and has no `invites` key; an invite minted for delivery is sent
//! again on every connect, the same invite, until its first use; a profile
//! credential revoke closes exactly that credential's Lobby sockets.
use super::*;
use sha2::Digest;

const MASTER: &str = "delivery-master-key";

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

async fn closed_building(db: &std::path::Path) -> (u16, ServerGuard) {
    spawn_server_env(
        MASTER,
        &[
            ("DB_PATH", db.to_str().unwrap().to_string()),
            ("REQUIRE_INVITE", "1".to_string()),
            ("LEGACY_WS_QUERY_AUTH", "0".to_string()),
        ],
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

async fn call(base: &str, room: &str, name: &str) -> String {
    let (status, v) = send(
        reqwest::Method::POST,
        format!("{base}/rooms/{room}/call"),
        &[("x-admin-token", MASTER)],
        Some(serde_json::json!({ "name": name })),
    )
    .await;
    assert_eq!(status, 201, "{v}");
    v["token"].as_str().unwrap().to_string()
}

async fn lobby(port: u16, key: &str) -> Socket {
    connect_ws_protocols(
        format!("ws://127.0.0.1:{port}/lobby/ws"),
        &["loca.v1".into(), format!("loca.membership.{key}")],
    )
    .await
}

/// The Lobby handshake's HTTP answer for `key`, or 101 when it opened.
async fn lobby_status(port: u16, key: &str) -> u16 {
    let mut request = format!("ws://127.0.0.1:{port}/lobby/ws")
        .into_client_request()
        .unwrap();
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_str(&format!("loca.v1, loca.membership.{key}")).unwrap(),
    );
    match tokio_tungstenite::connect_async(request).await {
        Ok(_) => 101,
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => response.status().as_u16(),
        Err(error) => panic!("lobby handshake: {error}"),
    }
}

async fn loca_socket(port: u16, room: &str, name: &str, invite: &str) -> Socket {
    connect_ws_protocols(
        format!("ws://127.0.0.1:{port}/ws?room={room}&name={name}&type=agent"),
        &["loca.v1".into(), format!("loca.room.{invite}")],
    )
    .await
}

/// The server's public id for an invite (`ivid_`), as the kit computes it.
fn invite_id(invite: &str) -> String {
    let digest = sha2::Sha256::digest(format!("invite:{invite}").as_bytes());
    let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
    format!("ivid_{hex}")
}

/// Every text frame within `window` (each checked against the frame list),
/// and whether the socket was still open at its end.
async fn frames_within(socket: &mut Socket, window: Duration) -> (Vec<Value>, bool) {
    let mut seen = Vec::new();
    let deadline = tokio::time::sleep(window);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => return (seen, true),
            item = socket.next() => match item {
                Some(Ok(WsMessage::Text(text))) => {
                    let v: Value = serde_json::from_str(&text).unwrap();
                    assert_listed_frame(&v);
                    seen.push(v);
                }
                Some(Ok(WsMessage::Close(_))) | None | Some(Err(_)) => return (seen, false),
                Some(Ok(_)) => {}
            }
        }
    }
}

/// What a fresh Lobby connection is sent first: `lobby_ready`, then the
/// frames that follow it at once.
async fn connect_and_read(port: u16, key: &str) -> (Socket, Value, Vec<Value>) {
    let mut socket = lobby(port, key).await;
    let ready = wait_for(&mut socket, |v| v["t"] == "lobby_ready").await;
    let (after, open) = frames_within(&mut socket, Duration::from_millis(500)).await;
    assert!(open, "the Lobby stays open");
    (socket, ready, after)
}

fn assert_seat_shape(ready: &Value, seats: &[(&str, &str)]) {
    assert!(
        ready.get("invites").is_none(),
        "lobby_ready never carries an invites key: {ready}"
    );
    assert!(
        !ready.to_string().contains("dv_"),
        "lobby_ready never carries an invite: {ready}"
    );
    assert!(ready["member_id"].as_str().unwrap().starts_with("mbid_"));
    let want: Vec<Value> = seats
        .iter()
        .map(|(room, invite)| serde_json::json!({ "room": room, "invite_id": invite_id(invite) }))
        .collect();
    assert_eq!(ready["seats"], Value::Array(want));
}

fn calls(frames: &[Value]) -> Vec<Value> {
    frames
        .iter()
        .filter(|v| v["t"] == "called")
        .cloned()
        .collect()
}

fn called(room: &str, invite: &str) -> Value {
    serde_json::json!({ "t": "called", "room": room, "token": invite, "invite_id": invite_id(invite) })
}

/// Two Lobby listeners, a frame lost with a dropped socket, and a loca
/// socket that comes late: every connect before the invite's first use is
/// sent the same invite, none changes it, and after the use no connect
/// carries it. It keeps working throughout and stays the seat's only invite.
#[tokio::test]
async fn an_owed_invite_is_sent_again_unchanged_until_its_first_use() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _guard) = closed_building(&dir.path().join("owed.db")).await;
    create_test_room(port, MASTER, "parlor").await;
    let base = format!("http://127.0.0.1:{port}");
    let key = member_key(&base, "worker").await;
    // Called while no Lobby socket is open.
    let invite = call(&base, "parlor", "worker").await;

    let (mut first, ready, after) = connect_and_read(port, &key).await;
    assert_seat_shape(&ready, &[("parlor", &invite)]);
    assert_eq!(calls(&after), vec![called("parlor", &invite)]);

    // A second listener, before the first has opened the loca socket.
    let (mut second, ready, after) = connect_and_read(port, &key).await;
    assert_seat_shape(&ready, &[("parlor", &invite)]);
    assert_eq!(calls(&after), vec![called("parlor", &invite)]);

    // A delivery lost with its socket: dropped before anything was read.
    drop(lobby(port, &key).await);
    let (_third, _, after) = connect_and_read(port, &key).await;
    assert_eq!(calls(&after), vec![called("parlor", &invite)]);

    // The loca socket comes late, after all of that.
    let mut seated = loca_socket(port, "parlor", "worker", &invite).await;
    wait_for(&mut seated, |v| v["t"] == "history").await;
    let (_, ready, after) = connect_and_read(port, &key).await;
    assert_seat_shape(&ready, &[("parlor", &invite)]);
    assert!(
        calls(&after).is_empty(),
        "a used invite is not sent again: {after:?}"
    );
    assert!(!Value::Array(after).to_string().contains("dv_"));

    // The listeners that received it are untouched, and so is the invite.
    for socket in [&mut first, &mut second] {
        let (frames, open) = frames_within(socket, Duration::from_millis(200)).await;
        assert!(open && frames.is_empty(), "{frames:?}");
    }
    let (status, _) = send(
        reqwest::Method::GET,
        format!("{base}/rooms/parlor/messages"),
        &[("x-room-token", &invite)],
        None,
    )
    .await;
    assert_eq!(status, 200, "the delivered invite works");
    let (_, invites) = send(
        reqwest::Method::GET,
        format!("{base}/rooms/parlor/invites"),
        &[("x-admin-token", MASTER)],
        None,
    )
    .await;
    assert_eq!(invites.as_array().unwrap().len(), 1, "one invite per seat");
    assert_eq!(invites[0]["id"], invite_id(&invite));

    // The copy is gone from the database, the invite itself is not.
    let sql = rusqlite::Connection::open(dir.path().join("owed.db")).unwrap();
    let (copy, used): (Option<String>, Option<u64>) = sql
        .query_row(
            "SELECT delivery_token, first_used_at FROM invites WHERE id = ?1",
            [invite_id(&invite)],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(copy, None);
    assert!(used.is_some());
}

/// A server restart in the middle of a delivery resumes it; a restart after
/// the first use does not restart it.
#[tokio::test]
async fn a_restart_resumes_an_owed_delivery_and_not_a_finished_one() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("restart.db");
    let (key, invite) = {
        let (port, guard) = closed_building(&db).await;
        create_test_room(port, MASTER, "parlor").await;
        let base = format!("http://127.0.0.1:{port}");
        let key = member_key(&base, "worker").await;
        let invite = call(&base, "parlor", "worker").await;
        drop(guard);
        wait_for_server_shutdown(port).await;
        (key, invite)
    };
    let (port, guard) = closed_building(&db).await;
    let (_, ready, after) = connect_and_read(port, &key).await;
    assert_seat_shape(&ready, &[("parlor", &invite)]);
    assert_eq!(calls(&after), vec![called("parlor", &invite)]);
    // First use: a session taken with the invite.
    let (status, v) = send(
        reqwest::Method::POST,
        format!("http://127.0.0.1:{port}/sessions"),
        &[("x-room-token", &invite)],
        Some(serde_json::json!({ "name": "worker", "kind": "agent" })),
    )
    .await;
    assert_eq!(status, 201, "{v}");
    drop(guard);
    wait_for_server_shutdown(port).await;

    let (port, _guard) = closed_building(&db).await;
    let (_, ready, after) = connect_and_read(port, &key).await;
    assert_seat_shape(&ready, &[("parlor", &invite)]);
    assert!(calls(&after).is_empty(), "{after:?}");
    let (status, _) = send(
        reqwest::Method::GET,
        format!("http://127.0.0.1:{port}/rooms/parlor/messages"),
        &[("x-room-token", &invite)],
        None,
    )
    .await;
    assert_eq!(status, 200, "the invite survives both restarts");
}

/// Door B: revoking one credential at the profile door sends
/// `credential_revoked` to the Lobby sockets opened with it, and closes
/// them; their reconnect is refused. The member's other credential keeps
/// its Lobby socket, which still receives calls.
#[tokio::test]
async fn a_profile_revoke_closes_exactly_that_credentials_lobby_sockets() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _guard) = closed_building(&dir.path().join("door-b.db")).await;
    create_test_room(port, MASTER, "parlor").await;
    let base = format!("http://127.0.0.1:{port}");
    let key = member_key(&base, "worker").await;
    let (status, created) = send(
        reqwest::Method::POST,
        format!("{base}/profile/credentials"),
        &[("x-admin-token", &key)],
        Some(serde_json::json!({ "label": "worker laptop" })),
    )
    .await;
    assert_eq!(status, 201, "{created}");
    let device = created["secret"].as_str().unwrap().to_string();
    let device_id = created["credential"]["id"].as_str().unwrap().to_string();

    let (mut kept, _, _) = connect_and_read(port, &key).await;
    let (mut revoked_a, _, _) = connect_and_read(port, &device).await;
    let (mut revoked_b, _, _) = connect_and_read(port, &device).await;

    let (status, v) = send(
        reqwest::Method::DELETE,
        format!("{base}/profile/credentials/{device_id}"),
        &[("x-admin-token", &key)],
        None,
    )
    .await;
    assert_eq!(status, 204, "{v}");

    for socket in [&mut revoked_a, &mut revoked_b] {
        let (frames, open) = frames_within(socket, Duration::from_secs(3)).await;
        assert_eq!(
            frames,
            vec![serde_json::json!({ "t": "credential_revoked", "credential_id": device_id })]
        );
        assert!(!open, "the server closes the socket after the frame");
    }
    assert_eq!(
        lobby_status(port, &device).await,
        401,
        "its reconnect is refused"
    );

    let (frames, open) = frames_within(&mut kept, Duration::from_millis(300)).await;
    assert!(
        open && frames.is_empty(),
        "the other credential's socket stays: {frames:?}"
    );
    let invite = call(&base, "parlor", "worker").await;
    let live = wait_for(&mut kept, |v| v["t"] == "called").await;
    assert_eq!(live, called("parlor", &invite));
    assert_eq!(lobby_status(port, &key).await, 101);
}

/// How a Lobby socket ended after its last frame: with the server's close
/// frame, or broken off.
async fn ending_of(socket: &mut Socket) -> String {
    let next = tokio::time::timeout(Duration::from_secs(3), socket.next())
        .await
        .expect("the server ends the socket");
    match next {
        Some(Ok(WsMessage::Close(_))) => "closed".into(),
        Some(Ok(other)) => format!("another frame: {other:?}"),
        Some(Err(error)) => format!("broken: {error}"),
        None => "ended without a close frame".into(),
    }
}

/// A Lobby socket the server ends is closed, not dropped: the last frame
/// (`credential_revoked`, `membership_revoked`) is followed by a close frame.
/// A dropped socket reads as a broken connection, and when the peer had
/// bytes on their way the last frame itself could be lost to a reset.
#[tokio::test]
async fn a_lobby_socket_the_server_ends_gets_a_close_frame_after_its_last_frame() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _guard) = closed_building(&dir.path().join("lobby-close.db")).await;
    let base = format!("http://127.0.0.1:{port}");

    // A profile credential is revoked.
    let key = member_key(&base, "worker").await;
    let (status, created) = send(
        reqwest::Method::POST,
        format!("{base}/profile/credentials"),
        &[("x-admin-token", &key)],
        Some(serde_json::json!({ "label": "worker laptop" })),
    )
    .await;
    assert_eq!(status, 201, "{created}");
    let device = created["secret"].as_str().unwrap().to_string();
    let device_id = created["credential"]["id"].as_str().unwrap().to_string();
    let (mut by_device, _, _) = connect_and_read(port, &device).await;
    let (status, v) = send(
        reqwest::Method::DELETE,
        format!("{base}/profile/credentials/{device_id}"),
        &[("x-admin-token", &key)],
        None,
    )
    .await;
    assert_eq!(status, 204, "{v}");
    let _ = wait_for(&mut by_device, |v| v["t"] == "credential_revoked").await;
    assert_eq!(ending_of(&mut by_device).await, "closed");

    // The membership itself ends.
    let (mut by_key, _, _) = connect_and_read(port, &key).await;
    let (status, members) = send(
        reqwest::Method::GET,
        format!("{base}/members"),
        &[("x-admin-token", MASTER)],
        None,
    )
    .await;
    assert_eq!(status, 200, "{members}");
    let id = members
        .as_array()
        .and_then(|list| list.iter().find(|m| m["name"] == "worker"))
        .and_then(|m| m["id"].as_str())
        .expect("the member's id")
        .to_string();
    let (status, v) = send(
        reqwest::Method::DELETE,
        format!("{base}/members/{id}"),
        &[("x-admin-token", MASTER)],
        None,
    )
    .await;
    assert!(status < 300, "{status} {v}");
    let _ = wait_for(&mut by_key, |v| v["t"] == "membership_revoked").await;
    assert_eq!(ending_of(&mut by_key).await, "closed");
}
