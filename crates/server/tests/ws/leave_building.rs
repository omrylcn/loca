//! Leaving the Building (PRINCIPLES "Authority and departure matrix":
//! Leave). A member ends their own membership with their own Building
//! sign-in: every seat, key and session it authorized ends with it, and the
//! words already said stay with their locas. A loca invite cannot leave the
//! Building, the Master cannot leave their own, and nobody leaves without
//! saying so.
use super::*;

const MASTER: &str = "leave-building-master";
const ROOM: &str = "leave-table";

async fn admit_key(base: &str, name: &str) -> String {
    let member: Value = reqwest::Client::new()
        .post(format!("{base}/members"))
        .header("x-admin-token", MASTER)
        .json(&serde_json::json!({ "name": name, "kind": "agent" }))
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

async fn leave(base: &str, session: Option<&str>, confirm: &str) -> u16 {
    let mut request = reqwest::Client::new()
        .post(format!("{base}/membership/leave"))
        .json(&serde_json::json!({ "confirm": confirm }));
    if let Some(token) = session {
        request = request.header("x-session-token", token);
    }
    request.send().await.unwrap().status().as_u16()
}

#[tokio::test]
async fn a_member_leaves_the_building_and_everything_it_opened_ends() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _guard) = spawn_server_env(
        MASTER,
        &[
            ("REQUIRE_INVITE", "1".into()),
            (
                "DB_PATH",
                dir.path().join("leave.db").to_str().unwrap().into(),
            ),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    create_test_room(port, MASTER, ROOM).await;

    let key = admit_key(&base, "leaver").await;
    let invite = invite_for(&base, MASTER, ROOM, "leaver").await;
    let own = session_with(&base, ("x-room-token", &key), "leaver", None).await;
    let seat = session_with(&base, ("x-room-token", &invite), "leaver", Some(ROOM)).await;
    let said = client
        .post(format!("{base}/rooms/{ROOM}/messages"))
        .header("x-session-token", &seat)
        .json(&serde_json::json!({ "sender": "leaver", "sender_type": "agent", "text": "my last words" }))
        .send()
        .await
        .unwrap();
    assert!(said.status().is_success(), "{}", said.status());

    // Nobody leaves without a session, through a loca invite, or unasked.
    assert_eq!(leave(&base, None, "leave").await, 401);
    assert_eq!(leave(&base, Some(&seat), "leave").await, 403);
    assert_eq!(leave(&base, Some(&own), "").await, 400);
    let whoami = |token: String| {
        let client = client.clone();
        let base = base.clone();
        async move {
            client
                .get(format!("{base}/whoami"))
                .header("x-room-token", token)
                .send()
                .await
                .unwrap()
                .status()
                .as_u16()
        }
    };
    assert_eq!(
        whoami(key.clone()).await,
        200,
        "still a member before leaving"
    );

    assert_eq!(leave(&base, Some(&own), "leave").await, 200);

    // The membership, its key, its invite and both sessions are gone.
    assert_ne!(whoami(key.clone()).await, 200);
    for token in [&own, &seat] {
        let read = client
            .get(format!("{base}/rooms/{ROOM}/messages"))
            .header("x-session-token", token)
            .send()
            .await
            .unwrap()
            .status()
            .as_u16();
        assert_eq!(read, 401, "a session of a member who left opens nothing");
    }
    let members: Vec<Value> = client
        .get(format!("{base}/members"))
        .header("x-admin-token", MASTER)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(members.iter().all(|m| m["name"] != "leaver"), "{members:?}");
    // What was said stays with the loca.
    let history: Vec<Value> = client
        .get(format!("{base}/rooms/{ROOM}/messages"))
        .header("x-admin-token", MASTER)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(history.iter().any(|m| m["text"] == "my last words"));
    // Leaving twice finds nothing to leave.
    assert_eq!(leave(&base, Some(&own), "leave").await, 401);
}
