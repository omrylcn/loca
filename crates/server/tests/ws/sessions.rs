//! Session lifetime and the per-credential cap, over the real binary.

use super::*;

const MASTER: &str = "sessions-master";
const TWELVE_HOURS_MS: u64 = 12 * 60 * 60 * 1000;
/// `SESSIONS_PER_CREDENTIAL` in the server.
const CAP: usize = 16;
/// The server's WebSocket ping interval (`WS_PING_SECS`), at which an open
/// socket re-checks its credentials.
const PING: Duration = Duration::from_secs(10);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

async fn take(base: &str, header: (&str, &str), name: &str) -> Value {
    let response = reqwest::Client::new()
        .post(format!("{base}/sessions"))
        .header(header.0, header.1)
        .json(&serde_json::json!({ "name": name, "kind": "agent" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201, "a session is taken");
    response.json().await.unwrap()
}

async fn token(base: &str, header: (&str, &str)) -> String {
    take(base, header, "body-name").await["session_token"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Does this session still open the loca's history?
async fn works(base: &str, room: &str, session: &str) -> bool {
    match rest_can(base, room, &[("x-session-token", session)]).await {
        200 => true,
        401 => false,
        other => panic!("unexpected status {other}"),
    }
}

async fn session_socket(
    port: u16,
    room: &str,
    session: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    connect_ws_protocols(
        format!("ws://127.0.0.1:{port}/ws?room={room}&type=agent"),
        &["loca.v1".to_string(), format!("loca.session.{session}")],
    )
    .await
}

/// Read until the server closes the socket; how long that took.
async fn closed_within(
    mut ws: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    limit: Duration,
) -> Option<Duration> {
    let started = std::time::Instant::now();
    tokio::time::timeout(limit, async {
        loop {
            match ws.next().await {
                None | Some(Err(_)) | Some(Ok(WsMessage::Close(_))) => return,
                Some(Ok(_)) => {}
            }
        }
    })
    .await
    .ok()
    .map(|_| started.elapsed())
}

#[tokio::test]
async fn invite_and_sandbox_sessions_are_issued_with_a_twelve_hour_lifetime() {
    let directory = tempfile::tempdir().unwrap();
    let (port, _server) = spawn_server_env(
        MASTER,
        &[
            (
                "DB_PATH",
                directory
                    .path()
                    .join("lifetime.db")
                    .to_str()
                    .unwrap()
                    .into(),
            ),
            ("ROOM_TOKEN", "sandbox-key".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    create_test_room(port, MASTER, "lifetime").await;
    let invite = invite_for(&base, MASTER, "lifetime", "lifetime-worker").await;

    let before = now_ms();
    let by_invite = take(&base, ("x-room-token", &invite), "lifetime-worker").await;
    let by_building_key = take(&base, ("x-room-token", "sandbox-key"), "sandbox-person").await;
    let after = now_ms();
    for session in [&by_invite, &by_building_key] {
        let expires_at = session["expires_at"]
            .as_u64()
            .unwrap_or_else(|| panic!("a lifetime is issued: {session}"));
        assert!(
            (before + TWELVE_HOURS_MS..=after + TWELVE_HOURS_MS).contains(&expires_at),
            "twelve hours from the take: {expires_at}"
        );
    }
}

/// An open loca socket depends on its session: when the session ends (signed
/// out here, or ended by the cap below; expiry is the same check), the server
/// closes the socket at its next credential check, within one ping interval.
#[tokio::test]
async fn an_open_loca_socket_closes_within_one_ping_of_its_session_ending() {
    let directory = tempfile::tempdir().unwrap();
    let (port, _server) = spawn_server_env(
        MASTER,
        &[
            (
                "DB_PATH",
                directory.path().join("socket.db").to_str().unwrap().into(),
            ),
            ("REQUIRE_INVITE", "1".into()),
            ("REQUIRE_SESSIONS", "1".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    create_test_room(port, MASTER, "socket").await;
    let signed_out_invite = invite_for(&base, MASTER, "socket", "signed-out-worker").await;
    let evicted_invite = invite_for(&base, MASTER, "socket", "evicted-worker").await;

    let signed_out = token(&base, ("x-room-token", &signed_out_invite)).await;
    let evicted = token(&base, ("x-room-token", &evicted_invite)).await;
    let first = session_socket(port, "socket", &signed_out).await;
    let second = session_socket(port, "socket", &evicted).await;

    let response = reqwest::Client::new()
        .delete(format!("{base}/sessions"))
        .header("x-session-token", &signed_out)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 204);
    for _ in 0..CAP {
        token(&base, ("x-room-token", &evicted_invite)).await;
    }
    assert!(!works(&base, "socket", &evicted).await, "the cap ended it");

    let limit = PING + Duration::from_secs(5);
    let (first, second) = tokio::join!(closed_within(first, limit), closed_within(second, limit));
    assert!(first.is_some(), "a signed-out session's socket is closed");
    assert!(second.is_some(), "an evicted session's socket is closed");
}

#[tokio::test]
async fn a_seventeenth_session_ends_the_oldest_of_its_invite_or_key_even_when_taken_at_once() {
    let directory = tempfile::tempdir().unwrap();
    let (port, _server) = spawn_server_env(
        MASTER,
        &[
            (
                "DB_PATH",
                directory.path().join("cap.db").to_str().unwrap().into(),
            ),
            ("REQUIRE_INVITE", "1".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    create_test_room(port, MASTER, "cap").await;
    let member: Value = reqwest::Client::new()
        .post(format!("{base}/members"))
        .header("x-admin-token", MASTER)
        .json(&serde_json::json!({ "name": "cap-worker", "kind": "agent" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let membership = member["token"].as_str().unwrap().to_string();
    let invite = invite_for(&base, MASTER, "cap", "cap-worker").await;
    // The same person's second key has a count of its own.
    let laptop: Value = reqwest::Client::new()
        .post(format!("{base}/profile/credentials"))
        .header("x-admin-token", &membership)
        .json(&serde_json::json!({ "label": "laptop" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let laptop = laptop["secret"].as_str().unwrap().to_string();
    let mut other = Vec::new();
    for _ in 0..3 {
        other.push(token(&base, ("x-admin-token", &laptop)).await);
    }

    // The member's own key has a count, and the invite has a count of its
    // own: an invite session is filed under its invite, not under the key.
    let mut own = Vec::new();
    for _ in 0..CAP {
        own.push(token(&base, ("x-admin-token", membership.as_str())).await);
        // Distinct creation times keep "oldest" unambiguous.
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let mut first = Vec::new();
    for _ in 0..CAP {
        first.push(token(&base, ("x-room-token", invite.as_str())).await);
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    for session in &own {
        assert!(works(&base, "cap", session).await, "two counts, both full");
    }
    for session in &first {
        assert!(works(&base, "cap", session).await);
    }
    let seventeenth = token(&base, ("x-room-token", &invite)).await;
    assert!(works(&base, "cap", &seventeenth).await);
    assert!(!works(&base, "cap", &first[0]).await, "the oldest ended");
    for session in &first[1..] {
        assert!(works(&base, "cap", session).await, "only the oldest ended");
    }
    for session in &other {
        assert!(
            works(&base, "cap", session).await,
            "another key is untouched"
        );
    }

    // Eight taken at once: still sixteen live, and every new one works.
    let raced: Vec<String> = futures_util::future::join_all(
        (0..8).map(|_| token(&base, ("x-room-token", invite.as_str()))),
    )
    .await;
    for session in &raced {
        assert!(works(&base, "cap", session).await);
    }
    let mut live = 0;
    for session in first.iter().chain([&seventeenth]).chain(&raced) {
        if works(&base, "cap", session).await {
            live += 1;
        }
    }
    assert_eq!(live, CAP);

    // None of that touched the key's own sessions; its seventeenth ends its
    // oldest and only that one.
    for session in &own {
        assert!(
            works(&base, "cap", session).await,
            "the invite's cap is its own"
        );
    }
    let another = token(&base, ("x-admin-token", membership.as_str())).await;
    assert!(works(&base, "cap", &another).await);
    assert!(
        !works(&base, "cap", &own[0]).await,
        "the key's oldest ended"
    );
    for session in &own[1..] {
        assert!(
            works(&base, "cap", session).await,
            "only the key's oldest ended"
        );
    }
}
