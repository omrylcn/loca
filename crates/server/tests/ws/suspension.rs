//! A suspended membership is told so at the doors its key opens: the Lobby
//! handshake and the membership gate in front of REST (`/whoami` included)
//! answer 403 `membership suspended`, so an agent can wait for Resume instead
//! of stopping as rejected. Only a correct, unrevoked credential of the
//! suspended member gets that answer. An unknown key, a revoked key and any
//! key of a revoked membership are answered as they were before (401).
use super::*;

const MASTER: &str = "suspension-door-master-key";
const UNKNOWN: &str = "mb_no-such-membership-key";

async fn send(
    method: reqwest::Method,
    url: String,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> (u16, String) {
    let mut req = reqwest::Client::new().request(method, url);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    if let Some(body) = body {
        req = req.json(&body);
    }
    let r = req.send().await.unwrap();
    let status = r.status().as_u16();
    (status, r.text().await.unwrap_or_default())
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

/// Admit a member; returns its `mb_` key and its management id.
async fn admit(base: &str, name: &str) -> (String, String) {
    let (status, text) = send(
        reqwest::Method::POST,
        format!("{base}/members"),
        &[("x-admin-token", MASTER)],
        Some(serde_json::json!({ "name": name, "kind": "agent" })),
    )
    .await;
    assert!(status < 300, "admit {name}: {status} {text}");
    let member: Value = serde_json::from_str(&text).unwrap();
    let key = member["token"].as_str().unwrap().to_string();
    let (status, text) = send(
        reqwest::Method::GET,
        format!("{base}/residents"),
        &[("x-admin-token", MASTER)],
        None,
    )
    .await;
    assert_eq!(status, 200, "{text}");
    let residents: Value = serde_json::from_str(&text).unwrap();
    let id = residents
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == name)
        .unwrap()["membership_id"]
        .as_str()
        .unwrap()
        .to_string();
    (key, id)
}

/// A second, per-device credential of the same member; returns (id, secret).
async fn spare_credential(base: &str, key: &str, label: &str) -> (String, String) {
    let (status, text) = send(
        reqwest::Method::POST,
        format!("{base}/profile/credentials"),
        &[("x-admin-token", key)],
        Some(serde_json::json!({ "label": label })),
    )
    .await;
    assert!(status < 300, "profile credential: {status} {text}");
    let created: Value = serde_json::from_str(&text).unwrap();
    (
        created["credential"]["id"].as_str().unwrap().to_string(),
        created["secret"].as_str().unwrap().to_string(),
    )
}

async fn set_suspended(base: &str, id: &str, suspended: bool) {
    let (status, text) = send(
        reqwest::Method::POST,
        format!("{base}/members/{id}/suspension"),
        &[("x-admin-token", MASTER)],
        Some(serde_json::json!({ "suspended": suspended })),
    )
    .await;
    assert!(status < 300, "suspended={suspended}: {status} {text}");
}

/// The Lobby handshake's answer to one credential: (status, body).
async fn lobby(port: u16, credential: &str) -> (u16, String) {
    let mut request = format!("ws://127.0.0.1:{port}/lobby/ws")
        .into_client_request()
        .unwrap();
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_str(&format!("loca.v1, loca.membership.{credential}")).unwrap(),
    );
    match tokio_tungstenite::connect_async(request).await {
        Ok((mut ws, response)) => {
            // Seated only once `lobby_ready` arrives.
            let ready = tokio::time::timeout(Duration::from_secs(5), async {
                while let Some(Ok(frame)) = ws.next().await {
                    if let WsMessage::Text(text) = frame {
                        let value: Value = serde_json::from_str(&text).unwrap();
                        if value["t"] == "lobby_ready" {
                            return true;
                        }
                    }
                }
                false
            })
            .await
            .unwrap_or(false);
            assert!(ready, "an accepted Lobby handshake must seat the member");
            let _ = ws.close(None).await;
            (response.status().as_u16(), String::new())
        }
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => (
            response.status().as_u16(),
            response
                .body()
                .as_ref()
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .unwrap_or_default(),
        ),
        Err(other) => panic!("Lobby handshake failed outside HTTP: {other}"),
    }
}

async fn whoami(base: &str, credential: &str) -> (u16, String) {
    send(
        reqwest::Method::GET,
        format!("{base}/whoami"),
        &[("x-room-token", credential)],
        None,
    )
    .await
}

/// Suspended → 403 at the Lobby and `/whoami` for every live credential of
/// that member; a revoked credential of the same member and an unknown key
/// keep their 401; Resume opens both doors again with the same keys.
#[tokio::test]
async fn a_suspended_members_own_key_is_told_suspended_until_resume() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _guard) = closed_building(&dir.path().join("suspended-door.db")).await;
    let base = format!("http://127.0.0.1:{port}");
    let (key, id) = admit(&base, "worker").await;
    let (_, spare) = spare_credential(&base, &key, "worker laptop").await;
    let (dead_id, dead) = spare_credential(&base, &key, "worker old laptop").await;
    let (status, text) = send(
        reqwest::Method::DELETE,
        format!("{base}/profile/credentials/{dead_id}"),
        &[("x-admin-token", &key)],
        None,
    )
    .await;
    assert!(status < 300, "revoke one credential: {status} {text}");

    let unknown_lobby = lobby(port, UNKNOWN).await;
    let unknown_whoami = whoami(&base, UNKNOWN).await;
    assert_eq!(unknown_lobby, (401, "invalid membership".to_string()));
    assert_eq!(unknown_whoami, (401, "davet required".to_string()));
    let dead_lobby_before = lobby(port, &dead).await;
    let dead_whoami_before = whoami(&base, &dead).await;

    set_suspended(&base, &id, true).await;
    for credential in [&key, &spare] {
        assert_eq!(
            lobby(port, credential).await,
            (403, "membership suspended".to_string()),
            "the Lobby tells the member's own live key it is suspended"
        );
        assert_eq!(
            whoami(&base, credential).await,
            (403, "membership suspended".to_string()),
            "/whoami tells the member's own live key it is suspended"
        );
        // The same gate stands in front of every member REST route.
        let (status, text) = send(
            reqwest::Method::GET,
            format!("{base}/notifications"),
            &[("x-room-token", credential)],
            None,
        )
        .await;
        assert_eq!((status, text.as_str()), (403, "membership suspended"));
    }
    // A revoked credential of the suspended member learns nothing new, and
    // an unknown key is answered exactly as before.
    assert_eq!(lobby(port, &dead).await, dead_lobby_before);
    assert_eq!(dead_lobby_before, unknown_lobby);
    assert_eq!(whoami(&base, &dead).await, dead_whoami_before);
    assert_eq!(dead_whoami_before.0, 401);
    assert_eq!(lobby(port, UNKNOWN).await, unknown_lobby);
    assert_eq!(whoami(&base, UNKNOWN).await, unknown_whoami);

    set_suspended(&base, &id, false).await;
    for credential in [&key, &spare] {
        assert_eq!(
            lobby(port, credential).await.0,
            101,
            "Resume opens the Lobby"
        );
        let (status, text) = whoami(&base, credential).await;
        assert_eq!(status, 200, "{text}");
        let who: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(who["kind"], "member");
        assert_eq!(who["name"], "worker");
    }
    assert_eq!(lobby(port, &dead).await, unknown_lobby);
}

/// Suspended and then revoked: every key of that membership gets the answer
/// it would get had it never been suspended, at both doors. At the Lobby
/// that is the unknown key's answer. Only the "suspended before the revoke"
/// precondition is new behaviour; what follows the revoke is a guard.
#[tokio::test]
async fn a_revoked_suspended_membership_is_answered_like_any_revoked_one() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _guard) = closed_building(&dir.path().join("revoked-door.db")).await;
    let base = format!("http://127.0.0.1:{port}");
    // Revoked without ever being suspended: the reference answers.
    let (plain_key, plain_id) = admit(&base, "plain").await;
    let (_, plain_spare) = spare_credential(&base, &plain_key, "plain laptop").await;
    // Suspended first, then revoked.
    let (key, id) = admit(&base, "gone").await;
    let (_, spare) = spare_credential(&base, &key, "gone laptop").await;
    set_suspended(&base, &id, true).await;
    assert_eq!(
        lobby(port, &spare).await.0,
        403,
        "suspended before the revoke"
    );
    for member in [&plain_id, &id] {
        let (status, text) = send(
            reqwest::Method::DELETE,
            format!("{base}/members/{member}"),
            &[("x-admin-token", MASTER)],
            None,
        )
        .await;
        assert!(status < 300, "revoke: {status} {text}");
    }

    let unknown_lobby = lobby(port, UNKNOWN).await;
    assert_eq!(unknown_lobby, (401, "invalid membership".to_string()));
    for (credential, reference) in [(&key, &plain_key), (&spare, &plain_spare)] {
        assert_eq!(lobby(port, credential).await, unknown_lobby);
        assert_eq!(lobby(port, reference).await, unknown_lobby);
        let seen = whoami(&base, credential).await;
        assert_eq!(seen.0, 401, "{seen:?}");
        assert_eq!(seen, whoami(&base, reference).await);
    }
    // Revoking a membership revokes every credential of its member, so the
    // spare answers as any revoked credential does (above, like the one of
    // the membership that was never suspended): refused, and never 403.
    assert_eq!(whoami(&base, &spare).await.0, 401);
}
