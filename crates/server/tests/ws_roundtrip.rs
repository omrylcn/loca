//! End-to-end: boot the server, connect two WS clients (an agent + a user),
//! POST a message over REST, and assert both clients receive it live, plus
//! that the roster reflects both members.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::{
    client::IntoClientRequest, http::HeaderValue, Message as WsMessage,
};

// Observable barriers normally return as soon as the room state converges.
// This is only the upper bound for a genuinely stuck test, not an accepted
// propagation budget: shared CI runners can take longer than five seconds to
// schedule the room actor and polling requests under full-suite load.
const ASYNC_STATE_HANG_GUARD: Duration = Duration::from_secs(30);

#[tokio::test]
async fn building_suspension_blocks_access_and_resume_keeps_identity() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("building-suspension.db");
    let (port, _server) = spawn_server_env(
        "suspension-master",
        &[
            ("DB_PATH", db.to_str().unwrap().into()),
            ("REQUIRE_INVITE", "1".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let member: Value = client
        .post(format!("{base}/members"))
        .header("x-admin-token", "suspension-master")
        .json(&serde_json::json!({"name": "suspension-worker", "kind": "agent"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let key = member["token"].as_str().unwrap();
    let before: Value = client
        .get(format!("{base}/profile"))
        .header("x-admin-token", key)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let residents: Vec<Value> = client
        .get(format!("{base}/residents"))
        .header("x-admin-token", "suspension-master")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = residents
        .iter()
        .find(|r| r["name"] == "suspension-worker")
        .unwrap()["membership_id"]
        .as_str()
        .unwrap();
    create_test_room(port, "suspension-master", "suspension-room").await;
    let invite = invite_for(
        &base,
        "suspension-master",
        "suspension-room",
        "suspension-worker",
    )
    .await;
    let session = session_with(&base, ("x-room-token", &invite), "suspension-worker", None).await;
    let url =
        format!("ws://127.0.0.1:{port}/ws?room=suspension-room&name=suspension-worker&type=agent");
    let mut socket =
        connect_ws_protocols(url, &["loca.v1".into(), format!("loca.room.{invite}")]).await;
    let mut lobby = connect_ws_protocols(
        format!("ws://127.0.0.1:{port}/lobby/ws"),
        &["loca.v1".into(), format!("loca.membership.{key}")],
    )
    .await;
    // Both handshakes answer before the server registers the sockets. The
    // room socket is seated before its history; the Lobby socket listens
    // before its lobby_ready. Without these the suspension can be broadcast
    // before either listens.
    let _ = wait_for(&mut socket, |frame| frame["t"] == "history").await;
    let _ = wait_for(&mut lobby, |frame| frame["t"] == "lobby_ready").await;
    for suspended in [true, false] {
        let denied = client
            .post(format!("{base}/members/{id}/suspension"))
            .header("x-room-token", key)
            .json(&serde_json::json!({"suspended": suspended}))
            .send()
            .await
            .unwrap();
        assert!(denied.status().is_client_error());
        client
            .post(format!("{base}/members/{id}/suspension"))
            .header("x-admin-token", "suspension-master")
            .json(&serde_json::json!({"suspended": suspended}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        let profile = client
            .get(format!("{base}/profile"))
            .header("x-admin-token", key)
            .send()
            .await
            .unwrap();
        if suspended {
            assert!(profile.status().is_client_error());
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let frame = lobby
                        .next()
                        .await
                        .expect("Lobby must notify suspension")
                        .unwrap();
                    if let WsMessage::Text(text) = frame {
                        let value: Value = serde_json::from_str(&text).unwrap();
                        assert_listed_frame(&value);
                        assert_ne!(
                            value["t"], "membership_revoked",
                            "suspension must not erase the listener credential"
                        );
                        if value["t"] == "membership_suspended" {
                            break;
                        }
                    }
                }
            })
            .await
            .expect("bounded Lobby suspension notification");
            tokio::time::timeout(Duration::from_secs(5), async {
                while let Some(frame) = socket.next().await {
                    match frame {
                        Ok(WsMessage::Close(_)) | Err(_) => break,
                        _ => {}
                    }
                }
            })
            .await
            .expect("suspension must close the already-open room socket");
        } else {
            let after: Value = profile.error_for_status().unwrap().json().await.unwrap();
            assert_eq!(before["principal"]["id"], after["principal"]["id"]);
            assert!(after["principal"]["id"].is_string());
        }
        for (header, credential) in [("x-room-token", &invite), ("x-session-token", &session)] {
            let response = client
                .get(format!("{base}/rooms/suspension-room/messages"))
                .header(header, credential)
                .send()
                .await
                .unwrap();
            assert!(
                response.status().is_client_error(),
                "old seats and sessions must not revive"
            );
        }
    }
}

#[tokio::test]
async fn building_membership_revocation_requires_master_and_uses_opaque_reference() {
    let (port, _server) = spawn_server_with("test-building-master").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let smaster: Value = client
        .post(format!("{base}/smasters"))
        .header("x-admin-token", "test-building-master")
        .json(&serde_json::json!({"name": "test-deputy"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let member: Value = client
        .post(format!("{base}/members"))
        .header("x-admin-token", "test-building-master")
        .json(&serde_json::json!({"name": "test-resident", "kind": "agent"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let residents: Vec<Value> = client
        .get(format!("{base}/residents"))
        .header("x-admin-token", "test-building-master")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let resident = residents
        .iter()
        .find(|r| r["name"] == "test-resident")
        .unwrap();
    let id = resident["membership_id"].as_str().unwrap();
    assert!(id.starts_with("mbid_"));
    assert_ne!(resident["membership_id"], member["token"]);
    for credential in [None, smaster["token"].as_str(), member["token"].as_str()] {
        let mut request = client.delete(format!("{base}/members/{id}"));
        if let Some(credential) = credential {
            request = request.header("x-admin-token", credential);
        }
        let status = request.send().await.unwrap().status();
        assert!(
            status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::UNAUTHORIZED,
            "unauthorized callers must be rejected before mutation"
        );
    }
    assert_eq!(
        client
            .delete(format!("{base}/members/{id}"))
            .header("x-admin-token", "test-building-master")
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::OK
    );
    let residents: Vec<Value> = client
        .get(format!("{base}/residents"))
        .header("x-admin-token", "test-building-master")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!residents.iter().any(|r| r["name"] == "test-resident"));
}

// Pull the server's router together the same way main does. Since main.rs owns
// the app wiring, we rebuild a minimal equivalent by spawning the actual binary
// would be heavier; instead we exercise it over a real socket via the public
// HTTP surface, which is what agents and the web client use anyway.

/// Keep the child alive for the test's lifetime; dropping it kills the server.
struct ServerGuard {
    _child: tokio::process::Child,
}

// Picking an ephemeral port and handing it to a child is not atomic: after the
// probe listener is dropped, another parallel test can select the same port
// before this child binds it. Keep that hand-off serialized through /health so
// one test can never mistake another test's server for its own.
static SERVER_SPAWN_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn spawn_server() -> (u16, ServerGuard) {
    spawn_server_with("").await
}

async fn spawn_server_with(admin_token: &str) -> (u16, ServerGuard) {
    spawn_server_env(admin_token, &[]).await
}

/// An `env` entry for `spawn_server_env` that is not passed to the server:
/// its value is a file the server's output (its log, on stdout, and stderr)
/// is written to, for a test that reads what the server logs. Without it the
/// output is discarded.
const SERVER_LOG_FILE: &str = "TEST_SERVER_LOG_FILE";

/// Spawn with extra env vars (DB_PATH, RATE_LIMIT, …). A fixed `port` can be
/// forced via the `PORT` entry in `env`; otherwise the child binds port zero
/// itself and reports the OS-selected port without a probe/drop race.
async fn spawn_server_env(admin_token: &str, env: &[(&str, String)]) -> (u16, ServerGuard) {
    let _spawn_lock = SERVER_SPAWN_LOCK.lock().await;
    let forced_port = env
        .iter()
        .find(|(k, _)| *k == "PORT")
        .map(|(_, value)| value.parse::<u16>().unwrap());
    let port_file = forced_port
        .is_none()
        .then(tempfile::NamedTempFile::new)
        .transpose()
        .unwrap();

    let bin = env!("CARGO_BIN_EXE_room-server");
    let mut cmd = tokio::process::Command::new(bin);
    cmd.env("PORT", forced_port.unwrap_or(0).to_string())
        .env("RUST_LOG", "warn")
        .env("ADMIN_TOKEN", admin_token)
        // Most historical cases exercise unrelated room semantics and still
        // spell auth in the old query form. Dedicated security tests below
        // run with this disabled and prove the public default/header path.
        .env("LEGACY_WS_QUERY_AUTH", "1");
    if let Some(file) = &port_file {
        cmd.env("BOUND_PORT_FILE", file.path());
    }
    for (k, v) in env {
        if *k != "PORT" && *k != SERVER_LOG_FILE {
            cmd.env(k, v);
        }
    }
    let (stdout, stderr) = match env.iter().find(|(k, _)| *k == SERVER_LOG_FILE) {
        Some((_, file)) => {
            let file = std::fs::File::create(file).unwrap();
            (file.try_clone().unwrap().into(), file.into())
        }
        None => (std::process::Stdio::null(), std::process::Stdio::null()),
    };
    let child = cmd
        .stdout(stdout)
        .stderr(stderr)
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let guard = ServerGuard { _child: child };

    // Wait for /health.
    let client = reqwest::Client::new();
    let mut port = forced_port;
    let mut last_err = String::new();
    // 100×100ms = 10s: under a full parallel test run many servers boot at
    // once and a 5s window was occasionally too tight (flaky "did not come up").
    for _ in 0..100 {
        if port.is_none() {
            port = port_file
                .as_ref()
                .and_then(|file| std::fs::read_to_string(file.path()).ok())
                .and_then(|value| value.parse::<u16>().ok());
        }
        if let Some(port) = port {
            match client
                .get(format!("http://127.0.0.1:{port}/health"))
                .send()
                .await
            {
                Ok(r) if r.status().is_success() => return (port, guard),
                Ok(r) => last_err = format!("status {}", r.status()),
                Err(e) => last_err = e.to_string(),
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("server did not come up on port {port:?}: {last_err}");
}

/// Explicit test setup, using the same creation boundary as the application.
/// Joining, sending a message, or issuing an invite must not provision a loca.
async fn create_test_room(port: u16, master: &str, room: &str) {
    let response = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/rooms"))
        .header("x-admin-token", master)
        .json(&serde_json::json!({ "name": room }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201, "explicit fixture creation: {room}");
}

async fn connect_ws(
    port: u16,
    room: &str,
    name: &str,
    kind: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let url = format!("ws://127.0.0.1:{port}/ws?room={room}&name={name}&type={kind}");
    let (ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    ws
}

async fn connect_ws_protocols(
    url: String,
    protocols: &[String],
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let mut request = url.into_client_request().unwrap();
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_str(&protocols.join(", ")).unwrap(),
    );
    let (ws, response) = tokio_tungstenite::connect_async(request).await.unwrap();
    assert_eq!(
        response
            .headers()
            .get("Sec-WebSocket-Protocol")
            .and_then(|value| value.to_str().ok()),
        Some("loca.v1"),
        "the server must echo only the non-secret protocol"
    );
    ws
}

async fn ws_protocol_can(port: u16, room: &str, room_credential: &str) -> bool {
    let url = format!("ws://127.0.0.1:{port}/ws?room={room}&name=probe&type=agent");
    let mut request = url.into_client_request().unwrap();
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_str(&format!("loca.v1, loca.room.{room_credential}")).unwrap(),
    );
    tokio_tungstenite::connect_async(request).await.is_ok()
}

/// Every frame a round trip reads through `wait_for` must be one that
/// docs/protocol-frames.json lists, with exactly the fields it lists. This is
/// where hand-built Lobby/notification frames are checked on the wire
/// (tests/protocol_frames.rs explains what the static checks cannot see).
fn assert_listed_frame(frame: &Value) {
    static LIST: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    let list = LIST.get_or_init(|| {
        serde_json::from_str(include_str!("../../../docs/protocol-frames.json")).unwrap()
    });
    let tag = frame["t"]
        .as_str()
        .unwrap_or_else(|| panic!("frame without t: {frame}"));
    let entry = &list["server_frames"][tag];
    assert!(
        entry.is_object() && entry["on_wire"] != false,
        "server sent t={tag:?}, which docs/protocol-frames.json does not list as on the wire"
    );
    let fields = entry["fields"].as_object().unwrap();
    let optional = entry["optional"].as_array().cloned().unwrap_or_default();
    for key in frame.as_object().unwrap().keys() {
        assert!(
            key == "t" || fields.contains_key(key),
            "t={tag} carries {key:?}, which docs/protocol-frames.json does not list"
        );
    }
    for name in fields.keys() {
        assert!(
            frame.get(name).is_some() || optional.iter().any(|o| o == name.as_str()),
            "t={tag} lacks listed field {name:?}"
        );
    }
}

/// Read text frames until `pred` matches one, or time out.
async fn wait_for<F: Fn(&Value) -> bool>(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    pred: F,
) -> Value {
    let deadline = tokio::time::sleep(Duration::from_secs(3));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => panic!("timed out waiting for frame"),
            item = ws.next() => {
                let msg = item.expect("stream ended").expect("ws error");
                if let WsMessage::Text(txt) = msg {
                    let v: Value = serde_json::from_str(&txt).unwrap();
                    assert_listed_frame(&v);
                    if pred(&v) { return v; }
                }
            }
        }
    }
}

/// Wait until the public roster proves that a websocket has completed room
/// registration. A successful websocket handshake only proves transport setup;
/// publishing immediately afterwards can otherwise race the room actor.
async fn wait_for_member(
    client: &reqwest::Client,
    base: &str,
    room: &str,
    name: &str,
    auth: Option<(&str, &str)>,
) {
    tokio::time::timeout(ASYNC_STATE_HANG_GUARD, async {
        loop {
            let mut request = client.get(format!("{base}/rooms/{room}/members"));
            if let Some((header, value)) = auth {
                request = request.header(header, value);
            }
            if let Ok(response) = request.send().await {
                if response.status().is_success() {
                    if let Ok(members) = response.json::<Vec<Value>>().await {
                        if members.iter().any(|member| member["name"] == name) {
                            return;
                        }
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {name} to join {room}"));
}

async fn wait_for_member_absent(client: &reqwest::Client, base: &str, room: &str, name: &str) {
    tokio::time::timeout(ASYNC_STATE_HANG_GUARD, async {
        loop {
            if let Ok(response) = client
                .get(format!("{base}/rooms/{room}/members"))
                .send()
                .await
            {
                if response.status().is_success() {
                    if let Ok(members) = response.json::<Vec<Value>>().await {
                        if !members.iter().any(|member| member["name"] == name) {
                            return;
                        }
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {name} to leave {room}"));
}

async fn wait_for_server_shutdown(port: u16) {
    tokio::time::timeout(ASYNC_STATE_HANG_GUARD, async {
        loop {
            if tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_err()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("server on port {port} did not shut down"));
}

async fn report_ready_runtime(
    client: &reqwest::Client,
    base: &str,
    admin: &str,
    name: &str,
) -> String {
    let member: Value = client
        .post(format!("{base}/members"))
        .header("x-admin-token", admin)
        .json(&serde_json::json!({ "name": name, "kind": "agent" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = member["token"].as_str().unwrap().to_string();
    let response = client
        .post(format!("{base}/runtime/health"))
        .header("x-room-token", &token)
        .json(&serde_json::json!({ "wake": "IDLE", "ack": "IDLE" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    token
}

// ============================================================================
// SMOKE SUITE — the door, across every role × loca × channel × mode.
// Written after a night where per-feature tests passed but the features broke
// each other. These exercise the combinations, not the pieces.
// ============================================================================

/// One place to ask the REST door a yes/no, over a real socket.
async fn rest_can(base: &str, room: &str, headers: &[(&str, &str)]) -> u16 {
    let client = reqwest::Client::new();
    let mut req = client.get(format!("{base}/rooms/{room}/messages"));
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    req.send().await.unwrap().status().as_u16()
}

/// One place to ask the WS door — returns true if the handshake upgrades.
async fn ws_can(port: u16, room: &str, query: &str) -> bool {
    let url = format!("ws://127.0.0.1:{port}/ws?room={room}&name=probe&type=agent&{query}");
    tokio_tungstenite::connect_async(url).await.is_ok()
}

/// Admit `name` to the building (master action) — the founding act an invite
/// now requires. Idempotent for tests: admits only when the name is unknown.
async fn admit(base: &str, admin: &str, name: &str, kind: &str) {
    let client = reqwest::Client::new();
    let known: Vec<Value> = client
        .get(format!("{base}/members"))
        .header("x-admin-token", admin)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap_or_default();
    if known.iter().any(|m| m["name"] == name) {
        return;
    }
    let r = client
        .post(format!("{base}/members"))
        .header("x-admin-token", admin)
        .json(&serde_json::json!({ "name": name, "kind": kind }))
        .send()
        .await
        .unwrap();
    assert!(
        r.status().is_success(),
        "admit {name} failed: {}",
        r.status()
    );
}

/// Mint an invite for `name` into `room` (master action). An invite seats an
/// existing member, so this admits the name first when needed — the exact
/// two-step the model now demands.
async fn invite_for(base: &str, admin: &str, room: &str, name: &str) -> String {
    admit(base, admin, name, "agent").await;
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{base}/rooms/{room}/invites"))
        .header("x-admin-token", admin)
        .json(&serde_json::json!({ "name": name }))
        .send()
        .await
        .unwrap();
    assert!(
        response.status().is_success(),
        "invite fixture for {room}: {}",
        response.status()
    );
    let v: Value = response.json().await.unwrap();
    v["token"].as_str().unwrap().to_string()
}

/// The management id the server derives for an invite (`ivid_`) or a member
/// key (`mbid_`). Management URLs (`DELETE /rooms/:id/invites/:ref`,
/// `DELETE /members/:ref`) take these ids only, never the key itself.
fn management_id(prefix: &str, source: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(source.as_bytes());
    let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
    format!("{prefix}{hex}")
}

fn invite_ref(invite: &str) -> String {
    management_id("ivid_", &format!("invite:{invite}"))
}

fn member_ref(key: &str) -> String {
    management_id("mbid_", &format!("member:{key}"))
}

/// Take a session with a given key, optionally scoped to a loca.
async fn session_with(
    base: &str,
    key_header: (&str, &str),
    name: &str,
    loca: Option<&str>,
) -> String {
    let client = reqwest::Client::new();
    let mut body = serde_json::json!({ "name": name, "kind": "agent" });
    if let Some(l) = loca {
        body["loca"] = serde_json::json!(l);
    }
    let v: Value = client
        .post(format!("{base}/sessions"))
        .header(key_header.0, key_header.1)
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    v["session_token"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn profile_credentials_are_principal_bound_one_time_and_live_revocable() {
    let root = "root-profile-credential-test";
    let db_dir = tempfile::tempdir().unwrap();
    let db_path = db_dir.path().join("profile-credentials.sqlite3");
    let (port, guard) =
        spawn_server_env(root, &[("DB_PATH", db_path.to_string_lossy().into_owned())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let initial: Vec<Value> = client
        .get(format!("{base}/profile/credentials"))
        .header("x-admin-token", root)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let recovery = initial
        .iter()
        .find(|credential| credential["root_recovery"] == true)
        .expect("migration exposes a safe recovery summary");
    assert!(
        recovery.get("secret").is_none(),
        "lists never return secrets"
    );

    let appointed: Value = client
        .post(format!("{base}/smasters"))
        .header("x-admin-token", root)
        .json(&serde_json::json!({ "name": "alice" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let smaster_key = appointed["token"].as_str().unwrap();
    let smaster_created: Value = client
        .post(format!("{base}/profile/credentials"))
        .header("x-admin-token", smaster_key)
        .json(&serde_json::json!({ "label": "Alice laptop" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let smaster_credential_id = smaster_created["credential"]["id"].as_str().unwrap();
    let smaster_spare_secret = smaster_created["secret"].as_str().unwrap().to_string();
    let cross_principal_revoke = client
        .delete(format!(
            "{base}/profile/credentials/{smaster_credential_id}"
        ))
        .header("x-admin-token", root)
        .send()
        .await
        .unwrap();
    assert_eq!(
        cross_principal_revoke.status(),
        reqwest::StatusCode::NOT_FOUND,
        "a credential id never grants authority over another principal"
    );

    let smaster_credentials: Vec<Value> = client
        .get(format!("{base}/profile/credentials"))
        .header("x-admin-token", smaster_key)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let legacy_smaster_id = smaster_credentials
        .iter()
        .find(|row| row["label"] == "Smaster access")
        .and_then(|row| row["id"].as_str())
        .unwrap()
        .to_string();
    let revoke_legacy_smaster = client
        .delete(format!("{base}/profile/credentials/{legacy_smaster_id}"))
        .header("x-admin-token", smaster_key)
        .send()
        .await
        .unwrap();
    assert_eq!(
        revoke_legacy_smaster.status(),
        reqwest::StatusCode::NO_CONTENT
    );
    for response in [
        client
            .get(format!("{base}/profile"))
            .header("x-admin-token", smaster_key)
            .send()
            .await
            .unwrap(),
        client
            .post(format!("{base}/sessions"))
            .header("x-admin-token", smaster_key)
            .json(&serde_json::json!({ "name": "alice", "kind": "user" }))
            .send()
            .await
            .unwrap(),
        client
            .get(format!("{base}/rooms"))
            .header("x-admin-token", smaster_key)
            .send()
            .await
            .unwrap(),
    ] {
        assert_eq!(
            response.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "revoked legacy Smaster bearer must not regain authority through a fallback"
        );
    }
    assert_eq!(
        client
            .get(format!("{base}/profile"))
            .header("x-admin-token", &smaster_spare_secret)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::OK,
        "revoking one Smaster credential keeps another credential and the principal active"
    );

    let member: Value = client
        .post(format!("{base}/members"))
        .header("x-admin-token", root)
        .json(&serde_json::json!({ "name": "bob", "kind": "user" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let member_key = member["token"].as_str().unwrap().to_string();
    let member_credentials: Vec<Value> = client
        .get(format!("{base}/profile/credentials"))
        .header("x-admin-token", &member_key)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let legacy_member_id = member_credentials
        .iter()
        .find(|row| row["label"] == "Member access")
        .and_then(|row| row["id"].as_str())
        .unwrap();
    let member_spare: Value = client
        .post(format!("{base}/profile/credentials"))
        .header("x-admin-token", &member_key)
        .json(&serde_json::json!({ "label": "Bob laptop" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let member_spare_secret = member_spare["secret"].as_str().unwrap().to_string();
    assert_eq!(
        client
            .delete(format!("{base}/profile/credentials/{legacy_member_id}"))
            .header("x-admin-token", &member_key)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::NO_CONTENT
    );
    for response in [
        client
            .get(format!("{base}/profile"))
            .header("x-admin-token", &member_key)
            .send()
            .await
            .unwrap(),
        client
            .post(format!("{base}/sessions"))
            .header("x-admin-token", &member_key)
            .json(&serde_json::json!({ "name": "bob", "kind": "user" }))
            .send()
            .await
            .unwrap(),
        client
            .get(format!("{base}/whoami"))
            .header("x-room-token", &member_key)
            .send()
            .await
            .unwrap(),
    ] {
        assert_eq!(
            response.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "revoked legacy Member bearer must not pass profile, session, or membership gates"
        );
    }
    assert_eq!(
        client
            .get(format!("{base}/whoami"))
            .header("x-room-token", &member_spare_secret)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::OK,
        "another Member credential must keep the same Building principal active"
    );

    let created: Value = client
        .post(format!("{base}/profile/credentials"))
        .header("x-admin-token", root)
        .json(&serde_json::json!({ "label": "Workstation" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let credential_id = created["credential"]["id"].as_str().unwrap().to_string();
    let secret = created["secret"].as_str().unwrap().to_string();
    assert!(secret.starts_with("ak_"));

    let spare: Value = client
        .post(format!("{base}/profile/credentials"))
        .header("x-admin-token", root)
        .json(&serde_json::json!({ "label": "Spare active key" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let spare_secret = spare["secret"].as_str().unwrap().to_string();

    let listed: Vec<Value> = client
        .get(format!("{base}/profile/credentials"))
        .header("x-admin-token", &secret)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(listed.iter().any(|credential| {
        credential["id"] == credential_id && credential["label"] == "Workstation"
    }));
    assert!(listed
        .iter()
        .all(|credential| credential.get("secret").is_none()));

    let session_response: Value = client
        .post(format!("{base}/sessions"))
        .header("x-admin-token", &secret)
        .json(&serde_json::json!({ "name": "spoofed", "kind": "agent" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(session_response["name"], "Master");
    assert_eq!(session_response["admin"], true);
    let session = session_response["session_token"].as_str().unwrap();

    let root_revoke = client
        .delete(format!(
            "{base}/profile/credentials/{}",
            recovery["id"].as_str().unwrap()
        ))
        .header("x-admin-token", root)
        .send()
        .await
        .unwrap();
    assert_eq!(root_revoke.status(), reqwest::StatusCode::CONFLICT);

    let revoked = client
        .delete(format!("{base}/profile/credentials/{credential_id}"))
        .header("x-admin-token", root)
        .send()
        .await
        .unwrap();
    assert_eq!(revoked.status(), reqwest::StatusCode::NO_CONTENT);

    let credential_after_revoke = client
        .get(format!("{base}/profile/credentials"))
        .header("x-admin-token", &secret)
        .send()
        .await
        .unwrap();
    assert_eq!(
        credential_after_revoke.status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    let session_after_revoke = client
        .get(format!("{base}/profile/credentials"))
        .header("x-session-token", session)
        .send()
        .await
        .unwrap();
    assert_eq!(
        session_after_revoke.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "revoking one credential must also end only its linked sessions"
    );

    drop(guard);
    let (restart_port, _restart_guard) =
        spawn_server_env(root, &[("DB_PATH", db_path.to_string_lossy().into_owned())]).await;
    let restarted = format!("http://127.0.0.1:{restart_port}");
    let resurrected_session = client
        .get(format!("{restarted}/profile/credentials"))
        .header("x-session-token", session)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resurrected_session.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "restart must not rebind a revoked credential session to root recovery"
    );
    for still_live in [root, spare_secret.as_str()] {
        let response = client
            .get(format!("{restarted}/profile/credentials"))
            .header("x-admin-token", still_live)
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            reqwest::StatusCode::OK,
            "root recovery and unrelated active credentials must survive"
        );
    }
}

#[tokio::test]
async fn revoked_legacy_building_member_key_cannot_reenter_ws_but_spare_can() {
    let root = "root-member-ws-revoke-test";
    let db_dir = tempfile::tempdir().unwrap();
    let db_path = db_dir.path().join("member-ws-revoke.sqlite3");
    let db_value = db_path.to_string_lossy().into_owned();

    // First establish a durable Member principal and a second device
    // credential. The following boot deliberately uses that legacy member key
    // as the transitional ROOM_TOKEN, reproducing an upgraded installation.
    let (setup_port, setup_guard) = spawn_server_env(root, &[("DB_PATH", db_value.clone())]).await;
    create_test_room(setup_port, root, "credential-room").await;
    let setup_base = format!("http://127.0.0.1:{setup_port}");
    let client = reqwest::Client::new();
    let member: Value = client
        .post(format!("{setup_base}/members"))
        .header("x-admin-token", root)
        .json(&serde_json::json!({ "name": "bob", "kind": "user" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let legacy = member["token"].as_str().unwrap().to_string();
    let spare: Value = client
        .post(format!("{setup_base}/profile/credentials"))
        .header("x-admin-token", &legacy)
        .json(&serde_json::json!({ "label": "Bob spare" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let spare = spare["secret"].as_str().unwrap().to_string();
    drop(setup_guard);

    let runtime_env = [
        ("DB_PATH", db_value.clone()),
        ("ROOM_TOKEN", legacy.clone()),
        ("REQUIRE_INVITE", "1".to_string()),
    ];
    let (port, guard) = spawn_server_env(root, &runtime_env).await;
    let base = format!("http://127.0.0.1:{port}");
    assert!(ws_protocol_can(port, "credential-room", &legacy).await);
    assert!(
        ws_protocol_can(port, "credential-room", &spare).await,
        "an active device credential inherits the principal's transitional building-key access"
    );

    let credentials: Vec<Value> = client
        .get(format!("{base}/profile/credentials"))
        .header("x-admin-token", &spare)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let legacy_id = credentials
        .iter()
        .find(|credential| credential["label"] == "Member access")
        .and_then(|credential| credential["id"].as_str())
        .unwrap();
    client
        .delete(format!("{base}/profile/credentials/{legacy_id}"))
        .header("x-admin-token", &spare)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    assert!(
        !ws_protocol_can(port, "credential-room", &legacy).await,
        "revoked legacy Member bearer must fail the room WebSocket handshake"
    );
    assert!(ws_protocol_can(port, "credential-room", &spare).await);
    assert_eq!(
        rest_can(&base, "credential-room", &[("x-room-token", &legacy)]).await,
        401
    );
    assert_eq!(
        rest_can(&base, "credential-room", &[("x-room-token", &spare)]).await,
        200
    );

    drop(guard);
    let (restart_port, _restart_guard) = spawn_server_env(root, &runtime_env).await;
    assert!(
        !ws_protocol_can(restart_port, "credential-room", &legacy).await,
        "restart must not resurrect a revoked legacy room WebSocket bearer"
    );
    assert!(
        ws_protocol_can(restart_port, "credential-room", &spare).await,
        "restart must preserve the principal's access through another active credential"
    );
}

#[tokio::test]
async fn loca_operator_is_one_human_principal_scoped_to_one_room() {
    let root = "root-loca-operator-test";
    let db_dir = tempfile::tempdir().unwrap();
    let db_path = db_dir.path().join("loca-operator.sqlite3");
    let (port, guard) =
        spawn_server_env(root, &[("DB_PATH", db_path.to_string_lossy().into_owned())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let mut bob_membership = None;
    for (name, kind) in [("bob", "user"), ("carol", "user"), ("worker", "agent")] {
        let member: Value = client
            .post(format!("{base}/members"))
            .header("x-admin-token", root)
            .json(&serde_json::json!({ "name": name, "kind": kind }))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        if name == "bob" {
            bob_membership = member["token"].as_str().map(str::to_owned);
        }
    }
    for room in ["alpha", "beta", "race"] {
        create_test_room(port, root, room).await;
        // Tasks live on the Loca Board, which needs a Lead.
        loca_board::turn_board_on(&base, root, room, "board-lead").await;
        // Exercise delegation into a deliberately vacant seat, not replacement
        // of the founder appointed by explicit creation.
        client
            .delete(format!("{base}/rooms/{room}/operators"))
            .header("x-admin-token", root)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        client
            .put(format!("{base}/rooms/{room}/settings"))
            .header("x-admin-token", root)
            .json(&serde_json::json!({ "live": false }))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
    }
    let profiles: Vec<Value> = client
        .get(format!("{base}/profiles"))
        .header("x-admin-token", root)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let profile_id = |name: &str| {
        profiles
            .iter()
            .find(|profile| profile["display_name"] == name)
            .and_then(|profile| profile["id"].as_str())
            .unwrap()
            .to_string()
    };
    let bob_id = profile_id("bob");
    let carol_id = profile_id("carol");
    let worker_id = profile_id("worker");

    let agent_rejected = client
        .post(format!("{base}/rooms/alpha/operators"))
        .header("x-admin-token", root)
        .json(&serde_json::json!({ "principal_id": worker_id }))
        .send()
        .await
        .unwrap();
    assert_eq!(agent_rejected.status(), reqwest::StatusCode::BAD_REQUEST);

    let smaster: Value = client
        .post(format!("{base}/smasters"))
        .header("x-admin-token", root)
        .json(&serde_json::json!({ "name": "alice" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let smaster_key = smaster["token"].as_str().unwrap();

    // The empty-seat rule belongs in the Store transaction, not in a Hub
    // preflight. These requests begin together: exactly one may create the
    // seat and the loser must not replace/revoke the winner.
    let race_bob = client
        .post(format!("{base}/rooms/race/operators"))
        .header("x-admin-token", smaster_key)
        .json(&serde_json::json!({ "principal_id": bob_id }));
    let race_carol = client
        .post(format!("{base}/rooms/race/operators"))
        .header("x-admin-token", smaster_key)
        .json(&serde_json::json!({ "principal_id": carol_id }));
    let (race_bob, race_carol) = tokio::join!(race_bob.send(), race_carol.send());
    let mut statuses = [
        race_bob.unwrap().status().as_u16(),
        race_carol.unwrap().status().as_u16(),
    ];
    statuses.sort_unstable();
    assert_eq!(
        statuses,
        [201, 409],
        "one concurrent Smaster wins an empty seat and one fails closed"
    );
    let race_view: Value = client
        .get(format!("{base}/rooms/race/operators"))
        .header("x-admin-token", root)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(race_view["appointed"].is_object());
    assert_eq!(
        race_view["history"].as_array().unwrap().len(),
        2,
        "only the revoked founder and winning appointment belong in history"
    );

    let appointed = client
        .post(format!("{base}/rooms/alpha/operators"))
        .header("x-admin-token", smaster_key)
        .json(&serde_json::json!({ "principal_id": bob_id }))
        .send()
        .await
        .unwrap();
    assert_eq!(appointed.status(), reqwest::StatusCode::CREATED);

    let smaster_replace = client
        .post(format!("{base}/rooms/alpha/operators"))
        .header("x-admin-token", smaster_key)
        .json(&serde_json::json!({ "principal_id": carol_id }))
        .send()
        .await
        .unwrap();
    assert_eq!(smaster_replace.status(), reqwest::StatusCode::CONFLICT);

    let bob_membership = bob_membership.expect("bob receives a one-time membership secret");
    let task_in_alpha = client
        .post(format!("{base}/rooms/alpha/tasks"))
        .header("x-admin-token", &bob_membership)
        .json(&serde_json::json!({ "title": "alpha only", "by": "spoofed" }))
        .send()
        .await
        .unwrap();
    assert_eq!(task_in_alpha.status(), reqwest::StatusCode::CREATED);
    let task: Value = task_in_alpha.json().await.unwrap();
    assert_eq!(task["created_by"], "bob");
    let task_in_beta = client
        .post(format!("{base}/rooms/beta/tasks"))
        .header("x-admin-token", &bob_membership)
        .json(&serde_json::json!({ "title": "must fail", "by": "bob" }))
        .send()
        .await
        .unwrap();
    assert_eq!(task_in_beta.status(), reqwest::StatusCode::FORBIDDEN);

    client
        .post(format!("{base}/rooms/alpha/lead"))
        .header("x-admin-token", root)
        .json(&serde_json::json!({ "lead": "bob" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let bob_session: Value = client
        .post(format!("{base}/sessions"))
        .header("x-admin-token", &bob_membership)
        .json(&serde_json::json!({ "name": "spoofed", "kind": "agent" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(bob_session["name"], "bob");
    let bob_session_token = bob_session["session_token"].as_str().unwrap();
    let profile: Value = client
        .get(format!("{base}/profile?room=alpha"))
        .header("x-session-token", bob_session_token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(profile["principal"]["display_name"], "bob");
    assert_eq!(profile["principal"]["kind"], "user");
    assert_eq!(profile["building_role"], "member");
    assert_eq!(profile["loca"]["operator_source"], "appointed");
    assert_eq!(
        profile["loca"]["roles"],
        serde_json::json!(["operator", "lead", "participant"])
    );
    assert_eq!(profile["session"]["bounded"], true);
    assert!(profile["session"]["credential_id"].is_string());
    assert!(
        serde_json::to_string(&profile)
            .unwrap()
            .find(&bob_membership)
            .is_none(),
        "profile output must not echo credentials"
    );
    let credentials: Vec<Value> = client
        .get(format!("{base}/profile/credentials"))
        .header("x-session-token", bob_session_token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        credentials
            .iter()
            .filter(|row| row["current"] == true)
            .count(),
        1
    );
    assert!(credentials.iter().all(|row| row.get("secret").is_none()));

    let master_replace = client
        .post(format!("{base}/rooms/alpha/operators"))
        .header("x-admin-token", root)
        .json(&serde_json::json!({ "principal_id": carol_id }))
        .send()
        .await
        .unwrap();
    assert_eq!(master_replace.status(), reqwest::StatusCode::CREATED);
    let protected = client
        .delete(format!("{base}/rooms/alpha/operators"))
        .header("x-admin-token", smaster_key)
        .send()
        .await
        .unwrap();
    assert_eq!(protected.status(), reqwest::StatusCode::FORBIDDEN);

    let authority_view: Value = client
        .get(format!("{base}/rooms/alpha/operators"))
        .header("x-admin-token", root)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(authority_view["appointed"]["principal_id"], carol_id);
    assert_eq!(authority_view["history"].as_array().unwrap().len(), 3);
    assert_eq!(authority_view["building_master"]["source"], "building");

    drop(guard);
    let (restart_port, _restart_guard) =
        spawn_server_env(root, &[("DB_PATH", db_path.to_string_lossy().into_owned())]).await;
    let restarted: Value = client
        .get(format!(
            "http://127.0.0.1:{restart_port}/rooms/alpha/operators"
        ))
        .header("x-admin-token", root)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(restarted["appointed"]["principal_id"], carol_id);
}

#[tokio::test]
async fn legacy_operator_names_migrate_only_when_one_human_principal_is_unambiguous() {
    let root = "root-legacy-operator-migration";
    let db_dir = tempfile::tempdir().unwrap();
    let db_path = db_dir.path().join("legacy-operator.sqlite3");
    let (port, guard) =
        spawn_server_env(root, &[("DB_PATH", db_path.to_string_lossy().into_owned())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let mut tokens = std::collections::HashMap::new();
    for name in ["bob", "carol", "duplicate"] {
        let member: Value = client
            .post(format!("{base}/members"))
            .header("x-admin-token", root)
            .json(&serde_json::json!({ "name": name, "kind": "user" }))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        tokens
            .entry(name.to_string())
            .or_insert_with(Vec::new)
            .push(member["token"].as_str().unwrap().to_string());
    }
    // Building role and display label are independent. A Smaster and a Member
    // that share a label make legacy name authority ambiguous even though
    // each principal is individually valid. No door admits that pair any
    // more, so the Smaster is renamed in storage below, as an older server
    // that let `POST /smasters` take a member's name would have left it.
    client
        .post(format!("{base}/smasters"))
        .header("x-admin-token", root)
        .json(&serde_json::json!({ "name": "duplicate-seed" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    for (room, operators) in [
        ("single", serde_json::json!(["bob"])),
        ("multiple", serde_json::json!(["bob", "carol"])),
        ("ambiguous", serde_json::json!(["duplicate"])),
    ] {
        create_test_room(port, root, room).await;
        client
            .delete(format!("{base}/rooms/{room}/operators"))
            .header("x-admin-token", root)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        client
            .put(format!("{base}/rooms/{room}/settings"))
            .header("x-admin-token", root)
            .json(&serde_json::json!({ "operators": operators }))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
    }

    drop(guard);
    wait_for_server_shutdown(port).await;
    let connection = rusqlite::Connection::open(&db_path).unwrap();
    for sql in [
        "UPDATE smasters SET name = 'duplicate' WHERE name = 'duplicate-seed'",
        "UPDATE principals SET display_name = 'duplicate' WHERE display_name = 'duplicate-seed'",
    ] {
        assert_eq!(connection.execute(sql, []).unwrap(), 1, "{sql}");
    }
    drop(connection);
    let (restart_port, _restart_guard) =
        spawn_server_env(root, &[("DB_PATH", db_path.to_string_lossy().into_owned())]).await;
    let restarted = format!("http://127.0.0.1:{restart_port}");
    // Tasks live on the Loca Board, which needs a Lead.
    for room in ["single", "multiple", "ambiguous"] {
        loca_board::turn_board_on(&restarted, root, room, "board-lead").await;
    }
    let profiles: Vec<Value> = client
        .get(format!("{restarted}/profiles"))
        .header("x-admin-token", root)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let bob_id = profiles
        .iter()
        .find(|profile| profile["display_name"] == "bob")
        .unwrap()["id"]
        .as_str()
        .unwrap();

    let single: Value = client
        .get(format!("{restarted}/rooms/single/operators"))
        .header("x-admin-token", root)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(single["appointed"]["principal_id"], bob_id);
    let settings: Value = client
        .get(format!("{restarted}/rooms/single/settings"))
        .header("x-admin-token", root)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(settings["operators"], serde_json::json!([]));

    let bob_task = client
        .post(format!("{restarted}/rooms/single/tasks"))
        .header("x-admin-token", &tokens["bob"][0])
        .json(&serde_json::json!({ "title": "migrated authority", "by": "spoof" }))
        .send()
        .await
        .unwrap();
    assert_eq!(bob_task.status(), reqwest::StatusCode::CREATED);

    for (room, principals) in [
        ("multiple", vec![&tokens["bob"][0], &tokens["carol"][0]]),
        ("ambiguous", vec![&tokens["duplicate"][0]]),
    ] {
        let view: Value = client
            .get(format!("{restarted}/rooms/{room}/operators"))
            .header("x-admin-token", root)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(view["appointed"].is_null(), "{room} must fail closed");
        for principal in principals {
            let denied = client
                .post(format!("{restarted}/rooms/{room}/tasks"))
                .header("x-admin-token", principal)
                .json(&serde_json::json!({ "title": "must fail", "by": "spoof" }))
                .send()
                .await
                .unwrap();
            assert_eq!(denied.status(), reqwest::StatusCode::FORBIDDEN);
        }
    }
}
// ---------------------------------------------------------------------------
// P0#1 — a direct reply re-wakes the waiter it answers.
//
// A waiter A (Wait A->B) must be reliably re-woken when B posts an explicit
// direct reply targeting A: live immediately, or — offline — durably replayed
// on reconnect with the same delivery id, at most once, never self-woken, the
// wait never auto-completed, and the reply suppresses the current generation's
// overdue while resetting the wait's age.
// ---------------------------------------------------------------------------

/// Connect an agent that only hears events addressed to it (the runtime's real
/// filter). A wake meant for one waiter is then asserted against that waiter's
/// own owner-filtered stream, exactly as a live agent would receive it.
async fn connect_agent_mentions(
    port: u16,
    room: &str,
    name: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let url = format!(
        "ws://127.0.0.1:{port}/ws?room={room}&name={name}&type=agent&filter=mentions&turn_max=1"
    );
    let (ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    ws
}

async fn declare_wait(
    client: &reqwest::Client,
    base: &str,
    room: &str,
    by: &str,
    waiting_for: &str,
) -> Value {
    let response = client
        .post(format!("{base}/rooms/{room}/waits"))
        .json(&serde_json::json!({
            "by": by, "waiting_for": waiting_for, "reason": "needs the other side"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201, "declare wait {by} -> {waiting_for}");
    response.json().await.unwrap()
}

async fn post_direct(
    client: &reqwest::Client,
    base: &str,
    room: &str,
    sender: &str,
    target: Option<&str>,
    text: &str,
) {
    let mut body = serde_json::json!({ "sender": sender, "sender_type": "agent", "text": text });
    if let Some(target) = target {
        body["target"] = serde_json::json!(target);
    }
    client
        .post(format!("{base}/rooms/{room}/messages"))
        .json(&body)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
}

/// Scan a socket for up to `ms` milliseconds, returning `true` if a `care`
/// frame satisfying `pred` arrives (non-text frames are skipped). A timeout
/// yields `false` — "no such care was delivered in the window".
async fn saw_care_within<F: Fn(&Value) -> bool>(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    ms: u64,
    pred: F,
) -> bool {
    tokio::time::timeout(Duration::from_millis(ms), async {
        loop {
            let Some(Ok(message)) = ws.next().await else {
                return false;
            };
            if let WsMessage::Text(text) = message {
                let value: Value = serde_json::from_str(&text).unwrap();
                if value["t"] == "care" && pred(&value) {
                    return true;
                }
            }
        }
    })
    .await
    .unwrap_or(false)
}

async fn get_json(client: &reqwest::Client, url: String) -> Vec<Value> {
    client.get(url).send().await.unwrap().json().await.unwrap()
}

#[path = "ws/access.rs"]
mod access;
#[path = "ws/attention.rs"]
mod attention;
#[path = "ws/authority_matrix.rs"]
mod authority_matrix;
#[path = "ws/board_off.rs"]
mod board_off;
#[path = "ws/building_key.rs"]
mod building_key;
#[path = "ws/claims.rs"]
mod claims;
#[path = "ws/content.rs"]
mod content;
#[path = "ws/delivery.rs"]
mod delivery;
#[path = "ws/dm.rs"]
mod dm;
#[path = "ws/downloads.rs"]
mod downloads;
#[path = "ws/extensions.rs"]
mod extensions;
#[path = "ws/groups.rs"]
mod groups;
#[path = "ws/hidden_locas.rs"]
mod hidden_locas;
#[path = "ws/key_ids.rs"]
mod key_ids;
#[path = "ws/keys.rs"]
mod keys;
#[path = "ws/kit_version.rs"]
mod kit_version;
#[path = "ws/lead_tasks.rs"]
mod lead_tasks;
#[path = "ws/lobby_delivery.rs"]
mod lobby_delivery;
#[path = "ws/loca_board.rs"]
mod loca_board;
#[path = "ws/loca_board_name.rs"]
mod loca_board_name;
#[path = "ws/loca_delete.rs"]
mod loca_delete;
#[path = "ws/loca_identity.rs"]
mod loca_identity;
#[path = "ws/loca_names.rs"]
mod loca_names;
#[path = "ws/member_keys.rs"]
mod member_keys;
#[path = "ws/membership.rs"]
mod membership;
#[path = "ws/notifications.rs"]
mod notifications;
#[path = "ws/principles.rs"]
mod principles;
#[path = "ws/profile_card.rs"]
mod profile_card;
#[path = "ws/profile_rename.rs"]
mod profile_rename;
#[path = "ws/rank_attribution.rs"]
mod rank_attribution;
#[path = "ws/reissue.rs"]
mod reissue;
#[path = "ws/review_history.rs"]
mod review_history;
#[path = "ws/review_regressions.rs"]
mod review_regressions;
#[path = "ws/rooms.rs"]
mod rooms;
#[path = "ws/server.rs"]
mod server;
#[path = "ws/sessions.rs"]
mod sessions;
#[path = "ws/suspension.rs"]
mod suspension;
#[path = "ws/task_board.rs"]
mod task_board;
#[path = "ws/task_view.rs"]
mod task_view;
#[path = "ws/typing.rs"]
mod typing;
#[path = "ws/upgrade.rs"]
mod upgrade;
#[path = "ws/vault.rs"]
mod vault;
#[path = "ws/work.rs"]
mod work;

/// Lower-case hex of `bytes` (a SHA-256 digest no longer formats with `{:x}`).
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
