//! Building settings on the real binary: the ranks read what the Building
//! runs on, under each environment variable's name with the value in force;
//! only the Master names the Building; the name survives a restart; a
//! memory-only Building refuses to pretend it kept one.
use serde_json::json;

use super::*;

const ROOT: &str = "bsettings-fixture-root";

async fn boot(db: Option<&str>) -> (u16, ServerGuard) {
    let mut env = vec![
        ("REQUIRE_INVITE", "1".to_string()),
        ("REQUIRE_SESSIONS", "1".to_string()),
        ("LOCA_CAPACITY", "5".to_string()),
        ("LOCA_CAPACITY_MAX", "12".to_string()),
        ("RATE_LIMIT", "20".to_string()),
        ("RATE_WINDOW_SECS", "40".to_string()),
        ("NOTIFICATION_RATE_LIMIT", "9".to_string()),
        ("PUBLIC_SERVER_URL", "https://loca.example.test".to_string()),
    ];
    if let Some(db) = db {
        env.push(("DB_PATH", db.to_string()));
    }
    spawn_server_env(ROOT, &env).await
}

async fn send(
    base: &str,
    method: &str,
    header: Option<(&str, &str)>,
    body: Option<Value>,
) -> (reqwest::StatusCode, Value) {
    let mut request = reqwest::Client::new()
        .request(method.parse().unwrap(), format!("{base}/building/settings"));
    if let Some((name, value)) = header {
        request = request.header(name, value);
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.unwrap();
    let status = response.status();
    let text = response.text().await.unwrap();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

async fn smaster_session(base: &str) -> String {
    let client = reqwest::Client::new();
    let smaster: Value = client
        .post(format!("{base}/smasters"))
        .header("x-admin-token", ROOT)
        .json(&json!({ "name": "deputy" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let session: Value = client
        .post(format!("{base}/sessions"))
        .header("x-admin-token", smaster["token"].as_str().unwrap())
        .json(&json!({ "name": "deputy", "kind": "user" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    session["session_token"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn ranks_read_the_environment_and_only_the_master_names_the_building() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("loca.db");
    let db = db.to_str().unwrap();
    let master = ("x-admin-token", ROOT);
    let (port, guard) = boot(Some(db)).await;
    let base = format!("http://127.0.0.1:{port}");

    // The Master reads every value in force, under its variable's name.
    let (status, view) = send(&base, "GET", Some(master), None).await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["name"], Value::Null);
    assert_eq!(view["editable"], json!(["name"]));
    assert_eq!(view["persistent"], true);
    assert_eq!(view["version"], env!("CARGO_PKG_VERSION"));
    let env = &view["environment"];
    assert_eq!(env["PUBLIC_SERVER_URL"], "https://loca.example.test");
    assert_eq!(env["REQUIRE_INVITE"], true);
    assert_eq!(env["REQUIRE_SESSIONS"], true);
    assert_eq!(env["LOCA_CAPACITY"], 5);
    assert_eq!(env["LOCA_CAPACITY_MAX"], 12);
    assert_eq!(env["RATE_LIMIT"], 20);
    assert_eq!(env["RATE_WINDOW_SECS"], 40);
    assert_eq!(env["NOTIFICATION_RATE_LIMIT"], 9);
    assert_eq!(env["LOCA_CARETAKERS"], json!(["loca-care"]));
    assert_eq!(env["LOCA_AGENT_ROOM"], "iye");
    assert_eq!(view["backup"]["download"], false);
    assert_eq!(view["backup"]["doc"], "docs/backup-restore.md");
    // No secret rides along: neither the root key nor the database path.
    let text = view.to_string();
    assert!(!text.contains(ROOT));
    assert!(!text.contains(db));

    // Nobody without Building rank reads it.
    let (status, _) = send(&base, "GET", None, None).await;
    assert!(matches!(status.as_u16(), 401 | 403), "{status}");

    // The Master names the Building: runs of spaces become one.
    let (status, saved) = send(
        &base,
        "PUT",
        Some(master),
        Some(json!({ "name": "  Omnelo   Tower " })),
    )
    .await;
    assert_eq!(status, 200, "{saved}");
    assert_eq!(saved["name"], "Omnelo Tower");

    // A Smaster reads the same view, with nothing to change, and is refused
    // the change.
    let deputy = smaster_session(&base).await;
    let as_deputy = ("x-session-token", deputy.as_str());
    let (status, view) = send(&base, "GET", Some(as_deputy), None).await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["name"], "Omnelo Tower");
    assert_eq!(view["editable"], json!([]));
    let (status, _) = send(
        &base,
        "PUT",
        Some(as_deputy),
        Some(json!({ "name": "Deputy's" })),
    )
    .await;
    assert_eq!(status, 403);

    // Refused on its merits: only the name is set here, and as text.
    for body in [
        json!({ "name": "line\nbreak" }),
        json!({ "name": "a\u{202E}b" }),
        json!({ "name": "x".repeat(65) }),
        json!({ "name": 7 }),
        json!({ "LOCA_CAPACITY": 9 }),
        json!({}),
        json!("Omnelo"),
    ] {
        let (status, answer) = send(&base, "PUT", Some(master), Some(body.clone())).await;
        assert_eq!(status, 400, "{body}: {answer}");
    }
    let (_, view) = send(&base, "GET", Some(master), None).await;
    assert_eq!(view["name"], "Omnelo Tower");
    assert_eq!(view["environment"]["LOCA_CAPACITY"], 5);

    // The name is kept across a restart.
    drop(guard);
    wait_for_server_shutdown(port).await;
    let (port, _guard) = boot(Some(db)).await;
    let base = format!("http://127.0.0.1:{port}");
    let (_, view) = send(&base, "GET", Some(master), None).await;
    assert_eq!(view["name"], "Omnelo Tower");

    // Null (or an empty string) clears it.
    let (status, saved) = send(&base, "PUT", Some(master), Some(json!({ "name": null }))).await;
    assert_eq!(status, 200, "{saved}");
    assert_eq!(saved["name"], Value::Null);
    let (_, view) = send(&base, "GET", Some(master), None).await;
    assert_eq!(view["name"], Value::Null);
}

#[tokio::test]
async fn a_memory_only_building_does_not_pretend_to_keep_a_name() {
    let (port, _guard) = boot(None).await;
    let base = format!("http://127.0.0.1:{port}");
    let master = ("x-admin-token", ROOT);
    let (_, view) = send(&base, "GET", Some(master), None).await;
    assert_eq!(view["persistent"], false);
    let (status, answer) = send(
        &base,
        "PUT",
        Some(master),
        Some(json!({ "name": "Omnelo" })),
    )
    .await;
    assert_eq!(status, 409, "{answer}");
    let (_, view) = send(&base, "GET", Some(master), None).await;
    assert_eq!(view["name"], Value::Null);
}
