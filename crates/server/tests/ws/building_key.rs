//! The legacy building key (`ROOM_TOKEN`) and the sessions taken beside it.
//!
//! A session taken with the building key itself reaches every ordinary loca,
//! exactly as the key does. A session taken with a member's own key (`mb_`)
//! or a profile credential (`ak_`) is that member signed in: it reaches what
//! the member is entitled to (an invite their principal holds, their group, their
//! direct message), and nothing from the building key the server also happens to have.
use super::*;

const MASTER: &str = "bk-master-key";
const BUILDING: &str = "bk-building-key";

async fn call(
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

/// `POST /sessions` with one credential header; the body name is a claim.
async fn take_session(base: &str, header: (&str, &str), name: &str) -> Value {
    let (status, v) = call(
        reqwest::Method::POST,
        format!("{base}/sessions"),
        &[header],
        Some(serde_json::json!({ "name": name, "kind": "user" })),
    )
    .await;
    assert_eq!(status, 201, "session for {name}: {v}");
    v
}

async fn admit(base: &str, name: &str) -> String {
    let (status, v) = call(
        reqwest::Method::POST,
        format!("{base}/members"),
        &[("x-admin-token", MASTER)],
        Some(serde_json::json!({ "name": name, "kind": "user" })),
    )
    .await;
    assert!(status < 300, "admit {name}: {status} {v}");
    v["token"].as_str().unwrap().to_string()
}

async fn invite(base: &str, room: &str, name: &str) -> String {
    let (status, v) = call(
        reqwest::Method::POST,
        format!("{base}/rooms/{room}/invites"),
        &[("x-admin-token", MASTER)],
        Some(serde_json::json!({ "name": name })),
    )
    .await;
    assert!(status < 300, "invite {name} to {room}: {status} {v}");
    v["token"].as_str().unwrap().to_string()
}

async fn membership_id(base: &str, name: &str) -> String {
    let (_, members) = call(
        reqwest::Method::GET,
        format!("{base}/members"),
        &[("x-admin-token", MASTER)],
        None,
    )
    .await;
    members
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == name)
        .unwrap_or_else(|| panic!("no member named {name}: {members}"))["id"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn read_status(base: &str, room: &str, session: &str) -> u16 {
    rest_can(base, room, &[("x-session-token", session)]).await
}

async fn post_status(base: &str, room: &str, session: &str) -> u16 {
    call(
        reqwest::Method::POST,
        format!("{base}/rooms/{room}/messages"),
        &[("x-session-token", session)],
        Some(serde_json::json!({ "sender": "x", "sender_type": "user", "text": "hello" })),
    )
    .await
    .0
}

/// Does the room WebSocket upgrade for this session (header subprotocol)?
async fn ws_session_can(port: u16, room: &str, session: &str) -> bool {
    let url = format!("ws://127.0.0.1:{port}/ws?room={room}&name=probe&type=user");
    let mut request = url.into_client_request().unwrap();
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_str(&format!("loca.v1, loca.session.{session}")).unwrap(),
    );
    tokio_tungstenite::connect_async(request).await.is_ok()
}

/// The ordinary sandbox shape `make smoke` and the browser suite use: a
/// persistent store with the legacy building key set.
async fn keyed_building(db: &std::path::Path) -> (u16, ServerGuard) {
    spawn_server_env(
        MASTER,
        &[
            ("DB_PATH", db.to_str().unwrap().into()),
            ("ROOM_TOKEN", BUILDING.into()),
        ],
    )
    .await
}

/// The reported reproduction: a member's own key is refused at a loca it was
/// never invited to, and exchanging that key for a session must not widen it.
#[tokio::test]
async fn a_member_key_exchanged_for_a_session_does_not_reach_uninvited_locas() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _guard) = keyed_building(&dir.path().join("bk-member.db")).await;
    let base = format!("http://127.0.0.1:{port}");
    create_test_room(port, MASTER, "secret").await;
    let key = admit(&base, "ayse").await;

    assert_eq!(
        rest_can(&base, "secret", &[("x-room-token", &key)]).await,
        401,
        "the raw membership key is refused at an uninvited loca"
    );
    let session = take_session(&base, ("x-room-token", &key), "ayse").await;
    let st = session["session_token"].as_str().unwrap();
    assert_eq!(
        read_status(&base, "secret", st).await,
        401,
        "a member session must not read an uninvited loca"
    );
    assert_eq!(
        post_status(&base, "secret", st).await,
        401,
        "a member session must not post to an uninvited loca"
    );
    assert!(
        !ws_session_can(port, "secret", st).await,
        "a member session must not open the socket of an uninvited loca"
    );
    let (_, rooms) = call(
        reqwest::Method::GET,
        format!("{base}/rooms"),
        &[("x-session-token", st)],
        None,
    )
    .await;
    assert!(
        !rooms.to_string().contains("\"secret\""),
        "an uninvited loca is not listed to a member session: {rooms}"
    );
}

/// The same for a session taken with a profile credential (`ak_`), whichever
/// header carries it.
#[tokio::test]
async fn a_profile_credential_session_does_not_reach_uninvited_locas() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _guard) = keyed_building(&dir.path().join("bk-profile.db")).await;
    let base = format!("http://127.0.0.1:{port}");
    create_test_room(port, MASTER, "secret").await;
    let key = admit(&base, "ayse").await;
    let (status, created) = call(
        reqwest::Method::POST,
        format!("{base}/profile/credentials"),
        &[("x-admin-token", &key)],
        Some(serde_json::json!({ "label": "ayse laptop" })),
    )
    .await;
    assert!(status < 300, "profile credential: {status} {created}");
    let spare = created["secret"].as_str().unwrap().to_string();

    for header in ["x-room-token", "x-admin-token"] {
        let session = take_session(&base, (header, &spare), "ayse").await;
        let st = session["session_token"].as_str().unwrap();
        assert_eq!(
            read_status(&base, "secret", st).await,
            401,
            "{header}: a profile-credential session must not read an uninvited loca"
        );
        assert_eq!(
            post_status(&base, "secret", st).await,
            401,
            "{header}: a profile-credential session must not post there"
        );
        assert!(
            !ws_session_can(port, "secret", st).await,
            "{header}: a profile-credential session must not open its socket"
        );
    }
}

/// Guard: the building key itself still signs a person into the whole
/// building, and the browser keeps that reach once it holds only the session.
#[tokio::test]
async fn a_building_key_session_still_reaches_every_ordinary_loca() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _guard) = keyed_building(&dir.path().join("bk-key.db")).await;
    let base = format!("http://127.0.0.1:{port}");
    create_test_room(port, MASTER, "secret").await;
    let session = take_session(&base, ("x-room-token", BUILDING), "sandbox-user").await;
    let st = session["session_token"].as_str().unwrap();
    assert_eq!(session["name"], "sandbox-user");
    assert_eq!(read_status(&base, "secret", st).await, 200);
    assert_eq!(post_status(&base, "secret", st).await, 201);
    assert!(ws_session_can(port, "secret", st).await);
}

/// Guard: what a member is entitled to is unchanged with the building key set
/// — the loca where their principal holds an invite, their group's loca, and
/// their direct message — while an uninvited ordinary loca stays shut.
#[tokio::test]
async fn a_member_session_keeps_its_invite_group_and_dm_beside_the_building_key() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _guard) = keyed_building(&dir.path().join("bk-entitled.db")).await;
    let base = format!("http://127.0.0.1:{port}");
    for room in ["secret", "seated"] {
        create_test_room(port, MASTER, room).await;
    }
    let ayse_key = admit(&base, "ayse").await;
    let mehmet_key = admit(&base, "mehmet").await;
    invite(&base, "seated", "ayse").await;
    invite(&base, "seated", "mehmet").await;
    let ayse_session = take_session(&base, ("x-room-token", &ayse_key), "ayse").await;
    let ayse = ayse_session["session_token"].as_str().unwrap().to_string();
    let mehmet_session = take_session(&base, ("x-room-token", &mehmet_key), "mehmet").await;
    let mehmet = mehmet_session["session_token"]
        .as_str()
        .unwrap()
        .to_string();

    // The invite held by her principal is her seat, whichever credential.
    assert_eq!(read_status(&base, "seated", &ayse).await, 200);
    assert_eq!(post_status(&base, "seated", &ayse).await, 201);
    assert!(ws_session_can(port, "seated", &ayse).await);

    // Her direct message with a tablemate.
    let (status, opened) = call(
        reqwest::Method::POST,
        format!("{base}/birebirs"),
        &[("x-session-token", &ayse)],
        Some(serde_json::json!({ "name": "mehmet" })),
    )
    .await;
    assert_eq!(status, 201, "{opened}");
    let dm = opened["birebir"]["room"].as_str().unwrap().to_string();
    assert_eq!(read_status(&base, &dm, &ayse).await, 200);
    assert_eq!(read_status(&base, &dm, &mehmet).await, 200);
    let building = take_session(&base, ("x-room-token", BUILDING), "sandbox-user").await;
    assert_ne!(
        read_status(&base, &dm, building["session_token"].as_str().unwrap()).await,
        200,
        "the building key never reached a direct message"
    );

    // Her group's loca, founded by a Smaster.
    let (status, smaster) = call(
        reqwest::Method::POST,
        format!("{base}/smasters"),
        &[("x-admin-token", MASTER)],
        Some(serde_json::json!({ "name": "founder" })),
    )
    .await;
    assert!(status < 300, "{smaster}");
    let founder = take_session(
        &base,
        ("x-admin-token", smaster["token"].as_str().unwrap()),
        "founder",
    )
    .await;
    let founder = founder["session_token"].as_str().unwrap().to_string();
    let (status, group) = call(
        reqwest::Method::POST,
        format!("{base}/groups"),
        &[("x-session-token", &founder)],
        Some(serde_json::json!({ "name": "backend" })),
    )
    .await;
    assert_eq!(status, 201, "{group}");
    let gid = group["id"].as_str().unwrap().to_string();
    let (_, profiles) = call(
        reqwest::Method::GET,
        format!("{base}/profiles"),
        &[("x-admin-token", MASTER)],
        None,
    )
    .await;
    let ayse_id = profiles
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["display_name"] == "ayse")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, v) = call(
        reqwest::Method::PUT,
        format!("{base}/groups/{gid}/members/{ayse_id}"),
        &[("x-session-token", &founder)],
        None,
    )
    .await;
    assert_eq!(status, 200, "{v}");
    let (status, v) = call(
        reqwest::Method::POST,
        format!("{base}/rooms"),
        &[("x-session-token", &founder)],
        Some(serde_json::json!({ "name": "team", "group": gid })),
    )
    .await;
    assert!(status < 300, "group loca: {status} {v}");
    assert_eq!(read_status(&base, "team", &ayse).await, 200);
    assert_ne!(
        read_status(&base, "team", &mehmet).await,
        200,
        "outside the group, the building key never reached a group loca"
    );
}

/// Guard: a ban holds against a member's session in the loca it was seated in.
#[tokio::test]
async fn a_banned_member_session_stays_out_beside_the_building_key() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _guard) = keyed_building(&dir.path().join("bk-ban.db")).await;
    let base = format!("http://127.0.0.1:{port}");
    create_test_room(port, MASTER, "seated").await;
    let key = admit(&base, "ayse").await;
    invite(&base, "seated", "ayse").await;
    let session = take_session(&base, ("x-room-token", &key), "ayse").await;
    let st = session["session_token"].as_str().unwrap();
    assert_eq!(read_status(&base, "seated", st).await, 200);
    let (status, v) = call(
        reqwest::Method::POST,
        format!("{base}/rooms/seated/moderate"),
        &[("x-admin-token", MASTER)],
        Some(serde_json::json!({ "action": "ban", "name": "ayse" })),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    assert_ne!(read_status(&base, "seated", st).await, 200);
    assert_ne!(post_status(&base, "seated", st).await, 201);
    assert!(!ws_session_can(port, "seated", st).await);
}

/// Suspending and then revoking a member ends what its session reached.
#[tokio::test]
async fn suspending_or_revoking_a_member_ends_its_session_beside_the_building_key() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _guard) = keyed_building(&dir.path().join("bk-revoke.db")).await;
    let base = format!("http://127.0.0.1:{port}");
    for room in ["secret", "seated"] {
        create_test_room(port, MASTER, room).await;
    }
    let key = admit(&base, "ayse").await;
    invite(&base, "seated", "ayse").await;
    let id = membership_id(&base, "ayse").await;

    let before = take_session(&base, ("x-room-token", &key), "ayse").await;
    let before = before["session_token"].as_str().unwrap().to_string();
    assert_eq!(read_status(&base, "seated", &before).await, 200);
    let (status, v) = call(
        reqwest::Method::POST,
        format!("{base}/members/{id}/suspension"),
        &[("x-admin-token", MASTER)],
        Some(serde_json::json!({ "suspended": true })),
    )
    .await;
    assert!(status < 300, "suspend: {status} {v}");
    for room in ["seated", "secret"] {
        assert_eq!(
            read_status(&base, room, &before).await,
            401,
            "a suspended member's session reaches nothing ({room})"
        );
    }
    let (status, v) = call(
        reqwest::Method::POST,
        format!("{base}/members/{id}/suspension"),
        &[("x-admin-token", MASTER)],
        Some(serde_json::json!({ "suspended": false })),
    )
    .await;
    assert!(status < 300, "resume: {status} {v}");
    invite(&base, "seated", "ayse").await;
    let after = take_session(&base, ("x-room-token", &key), "ayse").await;
    let after = after["session_token"].as_str().unwrap().to_string();
    assert_eq!(read_status(&base, "seated", &after).await, 200);
    let (status, v) = call(
        reqwest::Method::DELETE,
        format!("{base}/members/{id}"),
        &[("x-admin-token", MASTER)],
        None,
    )
    .await;
    assert!(status < 300, "revoke: {status} {v}");
    for room in ["seated", "secret"] {
        assert_eq!(
            read_status(&base, room, &after).await,
            401,
            "a revoked member's session reaches nothing ({room})"
        );
    }
}

/// Memory-only store (no `DB_PATH`) with the building key set: there is no
/// principal to bind, so the session must be bound to the membership itself —
/// it speaks as that member, reaches no uninvited loca, and dies with the
/// membership.
#[tokio::test]
async fn a_memory_only_member_session_is_bound_to_the_member_and_dies_with_it() {
    let (port, _guard) = spawn_server_env(MASTER, &[("ROOM_TOKEN", BUILDING.into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    create_test_room(port, MASTER, "secret").await;
    let key = admit(&base, "ayse").await;
    let id = membership_id(&base, "ayse").await;

    let session = take_session(&base, ("x-room-token", &key), "bob").await;
    let st = session["session_token"].as_str().unwrap().to_string();
    assert_eq!(
        session["name"], "ayse",
        "the membership names the session, not the request body"
    );
    assert_eq!(read_status(&base, "secret", &st).await, 401);
    assert_eq!(post_status(&base, "secret", &st).await, 401);
    assert!(!ws_session_can(port, "secret", &st).await);

    let (status, who) = call(
        reqwest::Method::GET,
        format!("{base}/whoami"),
        &[("x-session-token", &st)],
        None,
    )
    .await;
    assert_eq!(status, 200, "{who}");
    let (status, v) = call(
        reqwest::Method::DELETE,
        format!("{base}/members/{id}"),
        &[("x-admin-token", MASTER)],
        None,
    )
    .await;
    assert!(status < 300, "revoke: {status} {v}");
    let (status, who) = call(
        reqwest::Method::GET,
        format!("{base}/whoami"),
        &[("x-session-token", &st)],
        None,
    )
    .await;
    assert_eq!(
        status, 401,
        "a revoked member's session must not survive: {who}"
    );

    // Guard: the building key itself still signs in building-wide here too.
    let building = take_session(&base, ("x-room-token", BUILDING), "sandbox-user").await;
    let building = building["session_token"].as_str().unwrap();
    assert_eq!(read_status(&base, "secret", building).await, 200);
}

/// Guard: an upgraded installation whose building key IS a member's legacy
/// key. A session taken with that key is a session taken with the building
/// key itself and keeps the whole-building reach; so does one taken with the
/// same member's other device credential, which reaches it unexchanged too.
#[tokio::test]
async fn a_session_taken_with_a_legacy_member_building_key_keeps_its_reach() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("bk-legacy.db");
    let db_value = db.to_string_lossy().into_owned();
    let (setup_port, setup_guard) =
        spawn_server_env(MASTER, &[("DB_PATH", db_value.clone())]).await;
    let setup_base = format!("http://127.0.0.1:{setup_port}");
    create_test_room(setup_port, MASTER, "legacy-room").await;
    let legacy = admit(&setup_base, "bob").await;
    let (status, spare) = call(
        reqwest::Method::POST,
        format!("{setup_base}/profile/credentials"),
        &[("x-admin-token", &legacy)],
        Some(serde_json::json!({ "label": "bob spare" })),
    )
    .await;
    assert!(status < 300, "{spare}");
    let spare = spare["secret"].as_str().unwrap().to_string();
    drop(setup_guard);

    let (port, _guard) = spawn_server_env(
        MASTER,
        &[("DB_PATH", db_value), ("ROOM_TOKEN", legacy.clone())],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    for credential in [&legacy, &spare] {
        assert_eq!(
            rest_can(&base, "legacy-room", &[("x-room-token", credential)]).await,
            200
        );
        let session = take_session(&base, ("x-room-token", credential), "bob").await;
        let st = session["session_token"].as_str().unwrap();
        assert_eq!(read_status(&base, "legacy-room", st).await, 200);
        assert!(ws_session_can(port, "legacy-room", st).await);
    }
}
