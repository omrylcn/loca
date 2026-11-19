//! An agent's "replying…" signal. The agent's model turn runs in its runtime
//! adapter, not in the listener that holds its socket, so the adapter says it
//! is working over REST: `POST /rooms/:id/typing {"on": bool}` with its loca
//! session. The room hears the same ephemeral `typing` frame a browser's
//! keystrokes send. Only a session whose name is seated in that loca may say
//! it; nothing is stored.
use super::*;

const MASTER: &str = "typing-master-key";
const ROOM: &str = "typing-table";

async fn post_typing(base: &str, room: &str, session: Option<&str>, on: bool) -> u16 {
    let mut request = reqwest::Client::new()
        .post(format!("{base}/rooms/{room}/typing"))
        .json(&serde_json::json!({ "on": on }));
    if let Some(token) = session {
        request = request.header("x-session-token", token);
    }
    request.send().await.unwrap().status().as_u16()
}

#[tokio::test]
async fn a_seated_agent_session_says_it_is_replying() {
    let (port, _guard) = spawn_server_env(MASTER, &[("REQUIRE_INVITE", "1".into())]).await;
    create_test_room(port, MASTER, ROOM).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // The watcher is a browser seat; the agent's listener holds its seat.
    let watcher_invite = invite_for(&base, MASTER, ROOM, "ana").await;
    let watcher_session =
        session_with(&base, ("x-room-token", &watcher_invite), "ana", Some(ROOM)).await;
    let mut watcher = connect_ws(
        port,
        ROOM,
        &format!("ana&session={watcher_session}"),
        "user",
    )
    .await;
    let invite = invite_for(&base, MASTER, ROOM, "bot").await;
    let session = session_with(&base, ("x-room-token", &invite), "bot", Some(ROOM)).await;

    // Not seated yet: the signal is refused and nothing is broadcast.
    assert_eq!(post_typing(&base, ROOM, Some(&session), true).await, 409);

    let _listener = connect_ws(port, ROOM, &format!("bot&session={session}"), "agent").await;
    wait_for_member(
        &client,
        &base,
        ROOM,
        "bot",
        Some(("x-session-token", &watcher_session)),
    )
    .await;

    assert_eq!(post_typing(&base, ROOM, Some(&session), true).await, 204);
    let on = wait_for(&mut watcher, |f| f["t"] == "typing").await;
    assert_eq!(on["name"], "bot");
    assert_eq!(on["on"], true);

    assert_eq!(post_typing(&base, ROOM, Some(&session), false).await, 204);
    let off = wait_for(&mut watcher, |f| f["t"] == "typing").await;
    assert_eq!(off["name"], "bot");
    assert_eq!(off["on"], false);

    // No session, or a session scoped to another loca, says nothing.
    assert_eq!(post_typing(&base, ROOM, None, true).await, 401);
    create_test_room(port, MASTER, "other-table").await;
    assert_eq!(
        post_typing(&base, "other-table", Some(&session), true).await,
        401
    );
}
