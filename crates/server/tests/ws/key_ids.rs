//! A membership key or an invite used as an identifier, not checked as a
//! credential, is named by its management id: the seat key an `evicted`
//! frame carries, the membership an invite session proves, and nothing the
//! server logs carries either secret.

use super::*;

async fn member_id(base: &str, admin: &str, name: &str) -> String {
    let members: Vec<Value> = reqwest::Client::new()
        .get(format!("{base}/members"))
        .header("x-admin-token", admin)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    members
        .iter()
        .find(|m| m["name"] == name)
        .and_then(|m| m["id"].as_str())
        .expect("the member's id")
        .to_string()
}

/// The seat of a member taken over by a second connection: the older one
/// is told which seat in `evicted`, by the membership's id, whether the
/// seat was taken with the invite or with a session taken with it.
#[tokio::test]
async fn evicted_names_the_seat_by_the_membership_id_never_the_key() {
    let (port, _g) = spawn_server_env("MASTER", &[("REQUIRE_INVITE", "1".into())]).await;
    create_test_room(port, "MASTER", "general").await;
    let base = format!("http://127.0.0.1:{port}");
    let admitted: Value = reqwest::Client::new()
        .post(format!("{base}/members"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "cyber", "kind": "agent" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let key = admitted["token"]
        .as_str()
        .expect("the minted key")
        .to_string();
    let invite = invite_for(&base, "MASTER", "general", "cyber").await;
    let id = member_id(&base, "MASTER", "cyber").await;
    assert_eq!(id, management_id("mbid_", &format!("member:{key}")));
    let session = session_with(&base, ("x-room-token", &invite), "cyber", None).await;

    let url = |name: &str| format!("ws://127.0.0.1:{port}/ws?room=general&name={name}&type=agent");
    let mut by_invite = connect_ws_protocols(
        url("cyber"),
        &["loca.v1".into(), format!("loca.room.{invite}")],
    )
    .await;
    let _ = wait_for(&mut by_invite, |v| v["t"] == "history").await;
    let mut by_session = connect_ws_protocols(
        url("cyber-again"),
        &["loca.v1".into(), format!("loca.session.{session}")],
    )
    .await;
    let _ = wait_for(&mut by_session, |v| v["t"] == "history").await;
    let first = wait_for(&mut by_invite, |v| v["t"] == "evicted").await;
    let mut third = connect_ws_protocols(
        url("cyber"),
        &["loca.v1".into(), format!("loca.room.{invite}")],
    )
    .await;
    let _ = wait_for(&mut third, |v| v["t"] == "history").await;
    let second = wait_for(&mut by_session, |v| v["t"] == "evicted").await;

    for frame in [&first, &second] {
        let identity = frame["identity"].as_str().unwrap();
        assert_eq!(identity, format!("mb:{id}"), "{frame}");
        for secret in [key.as_str(), invite.as_str(), "mb_", "dv_"] {
            assert!(!identity.contains(secret), "{frame} carries {secret}");
        }
    }
}

/// `POST /rooms/:id/release` with only a session taken with the invite:
/// the session's membership (an id) finds the invite it holds there.
#[tokio::test]
async fn an_invite_session_alone_releases_its_own_seat() {
    let (port, _g) = spawn_server_env("MASTER", &[("REQUIRE_INVITE", "1".into())]).await;
    create_test_room(port, "MASTER", "parlor").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let invite = invite_for(&base, "MASTER", "parlor", "worker").await;
    let session = session_with(&base, ("x-room-token", &invite), "worker", None).await;

    let released = client
        .post(format!("{base}/rooms/parlor/release"))
        .header("x-session-token", &session)
        .send()
        .await
        .unwrap();
    assert_eq!(released.status(), 204);
    let old_door = client
        .get(format!("{base}/rooms/parlor/messages"))
        .header("x-room-token", &invite)
        .send()
        .await
        .unwrap();
    assert_eq!(old_door.status(), 401, "the released invite is over");
}

/// Everything the server logs, at its own debug level, across a member's
/// day: admission, the Lobby, a call, an invite session, posts with an
/// `op_id` over both transports, a seat taken over, a release and the end of
/// the membership. No line carries the membership key or an invite.
#[tokio::test]
async fn the_server_log_carries_no_membership_key_or_invite() {
    let directory = tempfile::tempdir().unwrap();
    let log = directory.path().join("server.log");
    let (port, guard) = spawn_server_env(
        "MASTER",
        &[
            ("REQUIRE_INVITE", "1".into()),
            (
                "DB_PATH",
                directory
                    .path()
                    .join("loca.db")
                    .to_string_lossy()
                    .into_owned(),
            ),
            ("RUST_LOG", "room_server=debug,server=debug".into()),
            (SERVER_LOG_FILE, log.to_string_lossy().into_owned()),
        ],
    )
    .await;
    create_test_room(port, "MASTER", "general").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let admitted: Value = client
        .post(format!("{base}/members"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "worker", "kind": "agent" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let key = admitted["token"]
        .as_str()
        .expect("the minted key")
        .to_string();
    let mut lobby = connect_ws_protocols(
        format!("ws://127.0.0.1:{port}/lobby/ws"),
        &["loca.v1".into(), format!("loca.membership.{key}")],
    )
    .await;
    let _ = wait_for(&mut lobby, |v| v["t"] == "lobby_ready").await;
    let called: Value = client
        .post(format!("{base}/rooms/general/call"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "worker" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let invite = wait_for(&mut lobby, |v| v["t"] == "called").await["token"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!called.to_string().is_empty());
    let session = session_with(&base, ("x-room-token", &invite), "worker", None).await;
    let posted = client
        .post(format!("{base}/rooms/general/messages"))
        .header("x-session-token", &session)
        .json(&serde_json::json!({
            "sender": "worker", "sender_type": "agent", "text": "over rest", "op_id": "op-log-1",
        }))
        .send()
        .await
        .unwrap();
    assert!(posted.status().is_success());
    let url = format!("ws://127.0.0.1:{port}/ws?room=general&name=worker&type=agent");
    let mut first = connect_ws_protocols(
        url.clone(),
        &["loca.v1".into(), format!("loca.room.{invite}")],
    )
    .await;
    let _ = wait_for(&mut first, |v| v["t"] == "history").await;
    first
        .send(WsMessage::Text(
            serde_json::json!({"t": "send", "text": "over the socket", "op_id": "op-log-1"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    let mut second =
        connect_ws_protocols(url, &["loca.v1".into(), format!("loca.session.{session}")]).await;
    let _ = wait_for(&mut second, |v| v["t"] == "history").await;
    let _ = wait_for(&mut first, |v| v["t"] == "evicted").await;
    let released = client
        .post(format!("{base}/rooms/general/release"))
        .header("x-room-token", &invite)
        .send()
        .await
        .unwrap();
    assert_eq!(released.status(), 204);
    let id = member_id(&base, "MASTER", "worker").await;
    let revoked = client
        .delete(format!("{base}/members/{id}"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap();
    assert!(revoked.status().is_success(), "{}", revoked.status());
    let _ = wait_for(&mut lobby, |v| v["t"] == "membership_revoked").await;
    drop((lobby, first, second));
    tokio::time::sleep(Duration::from_millis(200)).await;
    drop(guard);

    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("lobby join"), "the log was captured: {text}");
    for secret in [key.as_str(), invite.as_str(), session.as_str()] {
        assert!(!text.contains(secret), "the log carries a secret:\n{text}");
    }
    for prefix in ["mb_", "dv_", "mb:mb", "dv:dv"] {
        assert!(!text.contains(prefix), "the log carries {prefix}:\n{text}");
    }
}
