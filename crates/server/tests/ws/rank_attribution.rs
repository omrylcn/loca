//! Who a Building record names as the actor of an admission decision.
//!
//! PRINCIPLES "Groups" ("Every change is recorded with who made it") and the
//! Smaster definition ("Building layer") say a Smaster admits members and a
//! Smaster is not the Master. A Smaster signed in with a session — how the
//! browser and every automation work — must be recorded as that Smaster, the
//! same as one presenting its raw key, never as the Master.
use super::*;

const MASTER: &str = "attr-master-key";

async fn send(
    method: reqwest::Method,
    url: String,
    header: (&str, &str),
    body: Option<Value>,
) -> (u16, Value) {
    let mut req = reqwest::Client::new()
        .request(method, url)
        .header(header.0, header.1);
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

async fn file_join_request(base: &str, name: &str) -> String {
    let r = reqwest::Client::new()
        .post(format!("{base}/join-requests"))
        .json(&serde_json::json!({ "name": name, "kind": "agent" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 201, "join request for {name}");
    let v: Value = r.json().await.unwrap();
    v["request_id"].as_str().unwrap().to_string()
}

fn recorded(db: &std::path::Path, sql: &str, name: &str) -> String {
    rusqlite::Connection::open(db)
        .unwrap()
        .query_row(sql, [name], |r| r.get::<_, String>(0))
        .unwrap()
}

#[tokio::test]
async fn a_smaster_session_is_recorded_as_that_smaster_not_the_master() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("attribution.db");
    let (port, _guard) = spawn_server_env(
        MASTER,
        &[
            ("DB_PATH", db.to_str().unwrap().into()),
            ("REQUIRE_SESSIONS", "1".into()),
            ("REQUIRE_INVITE", "1".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");

    let (status, sm) = send(
        reqwest::Method::POST,
        format!("{base}/smasters"),
        ("x-admin-token", MASTER),
        Some(serde_json::json!({ "name": "deputy" })),
    )
    .await;
    assert_eq!(status, 201, "{sm}");
    let key = sm["token"].as_str().unwrap().to_string();
    let (status, s) = send(
        reqwest::Method::POST,
        format!("{base}/sessions"),
        ("x-admin-token", &key),
        Some(serde_json::json!({ "name": "deputy", "kind": "user" })),
    )
    .await;
    assert_eq!(status, 201, "{s}");
    let session = s["session_token"].as_str().unwrap().to_string();
    let as_session = ("x-session-token", session.as_str());

    // Direct admission through the session.
    let (status, admitted) = send(
        reqwest::Method::POST,
        format!("{base}/members"),
        as_session,
        Some(serde_json::json!({ "name": "by-session", "kind": "agent" })),
    )
    .await;
    assert_eq!(status, 201, "{admitted}");
    assert_eq!(
        admitted["admitted_by"], "smaster:deputy",
        "a Smaster session admits as that Smaster"
    );

    // Approval through the session: the member and the request both name it.
    let (status, _) = send(
        reqwest::Method::POST,
        format!("{base}/admission-stock"),
        as_session,
        Some(serde_json::json!({ "count": 1 })),
    )
    .await;
    assert_eq!(status, 201);
    let approved = file_join_request(&base, "by-approval").await;
    let (status, v) = send(
        reqwest::Method::POST,
        format!("{base}/join-requests/{approved}/approve"),
        as_session,
        None,
    )
    .await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(
        recorded(
            &db,
            "SELECT admitted_by FROM members WHERE name = ?1",
            "by-approval"
        ),
        "smaster:deputy",
        "an approved member names the approving Smaster"
    );
    assert_eq!(
        recorded(
            &db,
            "SELECT decided_by FROM join_requests WHERE name = ?1",
            "by-approval"
        ),
        "smaster:deputy",
        "the approved request names the approving Smaster"
    );

    // Denial through the session.
    let denied = file_join_request(&base, "by-denial").await;
    let (status, v) = send(
        reqwest::Method::POST,
        format!("{base}/join-requests/{denied}/deny"),
        as_session,
        None,
    )
    .await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(
        recorded(
            &db,
            "SELECT decided_by FROM join_requests WHERE name = ?1",
            "by-denial"
        ),
        "smaster:deputy",
        "the denied request names the denying Smaster"
    );

    // The raw key and the Master keep the names they had.
    let (_, raw) = send(
        reqwest::Method::POST,
        format!("{base}/members"),
        ("x-admin-token", &key),
        Some(serde_json::json!({ "name": "by-raw-key", "kind": "agent" })),
    )
    .await;
    assert_eq!(raw["admitted_by"], "smaster:deputy");
    let (_, by_master) = send(
        reqwest::Method::POST,
        format!("{base}/members"),
        ("x-admin-token", MASTER),
        Some(serde_json::json!({ "name": "by-master", "kind": "agent" })),
    )
    .await;
    assert_eq!(by_master["admitted_by"], "master");
}

#[tokio::test]
async fn a_renamed_smaster_session_is_recorded_under_its_current_name() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("attribution-rename.db");
    let (port, _guard) = spawn_server_env(
        MASTER,
        &[
            ("DB_PATH", db.to_str().unwrap().into()),
            ("REQUIRE_SESSIONS", "1".into()),
            ("REQUIRE_INVITE", "1".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let (_, sm) = send(
        reqwest::Method::POST,
        format!("{base}/smasters"),
        ("x-admin-token", MASTER),
        Some(serde_json::json!({ "name": "before" })),
    )
    .await;
    let key = sm["token"].as_str().unwrap().to_string();
    let (_, s) = send(
        reqwest::Method::POST,
        format!("{base}/sessions"),
        ("x-admin-token", &key),
        Some(serde_json::json!({ "name": "before", "kind": "user" })),
    )
    .await;
    let session = s["session_token"].as_str().unwrap().to_string();
    let (status, v) = send(
        reqwest::Method::PUT,
        format!("{base}/profile"),
        ("x-session-token", &session),
        Some(serde_json::json!({ "display_name": "after" })),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    let (status, admitted) = send(
        reqwest::Method::POST,
        format!("{base}/members"),
        ("x-session-token", &session),
        Some(serde_json::json!({ "name": "after-rename", "kind": "agent" })),
    )
    .await;
    assert_eq!(status, 201, "{admitted}");
    assert_eq!(admitted["admitted_by"], "smaster:after");
}
