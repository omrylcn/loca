//! Durable notes, memory search, and canonical content identity.

use super::*;

#[tokio::test]
async fn notes_create_update_and_soft_permission_push_live() {
    let (port, _guard) = spawn_server().await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // A watcher WS connection observes live note frames.
    let mut watcher = connect_ws(port, "general", "watcher", "user").await;

    // Operator creates a note assigned to "backend".
    let created: Value = client
        .post(format!("{base}/rooms/general/notes"))
        .json(&serde_json::json!({
            "key": "deploy-status", "title": "Deploy", "body": "idle",
            "by": "operator", "by_type": "user", "can_write": ["backend"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(created["rev"], 1);

    // The watcher gets a live note frame.
    let is_note = |v: &Value| v["t"] == "note" && v["note"]["key"] == "deploy-status";
    wait_for(&mut watcher, is_note).await;

    // Creating the same key again -> 409.
    let dup = client
        .post(format!("{base}/rooms/general/notes"))
        .json(&serde_json::json!({ "key": "deploy-status", "title": "x", "by": "operator" }))
        .send()
        .await
        .unwrap();
    assert_eq!(dup.status(), 409);

    // Assigned writer updates -> ok, no warn.
    client
        .put(format!("{base}/rooms/general/notes/deploy-status"))
        .json(&serde_json::json!({ "body": "building", "by": "backend", "by_type": "agent" }))
        .send()
        .await
        .unwrap();
    let updated = |v: &Value| v["t"] == "note" && v["note"]["body"] == "building";
    wait_for(&mut watcher, updated).await;

    // Unassigned writer updates -> still succeeds, but a warn frame is pushed.
    client
        .put(format!("{base}/rooms/general/notes/deploy-status"))
        .json(&serde_json::json!({ "body": "sneaky", "by": "web", "by_type": "agent" }))
        .send()
        .await
        .unwrap();
    let warn = |v: &Value| v["t"] == "notewarn" && v["by"] == "web" && v["key"] == "deploy-status";
    wait_for(&mut watcher, warn).await;

    // Updating a missing key -> 404.
    let missing = client
        .put(format!("{base}/rooms/general/notes/ghost"))
        .json(&serde_json::json!({ "by": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);

    // Delete: existing -> 204, then missing -> 404.
    let del = client
        .delete(format!("{base}/rooms/general/notes/deploy-status"))
        .send()
        .await
        .unwrap();
    assert_eq!(del.status(), 204);
    let del2 = client
        .delete(format!("{base}/rooms/general/notes/deploy-status"))
        .send()
        .await
        .unwrap();
    assert_eq!(del2.status(), 404);
}

#[tokio::test]
async fn note_history_and_room_memory_search() {
    // Persistent DB so revisions and the archive actually go somewhere.
    let db = std::env::temp_dir().join(format!("loca-mem-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let (port, _guard) =
        spawn_server_env("", &[("DB_PATH", db.to_string_lossy().into_owned())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // A note evolves: v1 -> v2 -> v3.
    client
        .post(format!("{base}/rooms/general/notes"))
        .json(&serde_json::json!({ "key": "auth", "title": "Auth", "body": "v1: jwt", "by": "a" }))
        .send()
        .await
        .unwrap();
    for body in ["v2: jwt+refresh", "v3: session-bound"] {
        client
            .put(format!("{base}/rooms/general/notes/auth"))
            .json(&serde_json::json!({ "body": body, "by": "b" }))
            .send()
            .await
            .unwrap();
    }

    // History returns the two superseded versions, newest first.
    let hist: Vec<Value> = client
        .get(format!("{base}/rooms/general/notes/auth/history"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(hist.len(), 2, "two replaced versions must be archived");
    assert_eq!(hist[0]["body"], "v2: jwt+refresh");
    assert_eq!(hist[1]["body"], "v1: jwt");

    // Search the room's memory: matches an old message AND the note.
    client.post(format!("{base}/rooms/general/messages"))
        .json(&serde_json::json!({ "sender": "a", "sender_type": "user", "text": "auth kararini verdik: session-bound" }))
        .send().await.unwrap();
    let res: Value = client
        .get(format!("{base}/rooms/general/search?q=session-bound"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(res["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["text"].as_str().unwrap().contains("kararini")));
    assert!(res["notes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n["key"] == "auth"));

    // Empty q -> 400.
    let bad = client
        .get(format!("{base}/rooms/general/search?q="))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);

    let _ = std::fs::remove_file(&db);
}

/// Notes are durable shared memory: their audit identity must come from the
/// same server-bound session as chat/task/journal, never from a JSON claim.
#[tokio::test]
async fn session_identity_is_canonical_for_notes_and_whoami() {
    let (port, _guard) = spawn_server_env("", &[("REQUIRE_SESSIONS", "1".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let session: Value = client
        .post(format!("{base}/sessions"))
        .json(&serde_json::json!({ "name": "alice", "kind": "user" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = session["session_token"].as_str().unwrap();
    assert_eq!(session["name"], "alice");

    let identity: Value = client
        .get(format!("{base}/whoami"))
        .header("x-session-token", token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(identity["kind"], "session");
    assert_eq!(identity["name"], "alice");

    let created: Value = client
        .post(format!("{base}/rooms/general/notes"))
        .header("x-session-token", token)
        .json(&serde_json::json!({
            "key": "plan",
            "body": "v1",
            "by": "mallory",
            "can_write": ["alice"]
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(created["updated_by"], "alice");

    let updated: Value = client
        .put(format!("{base}/rooms/general/notes/plan"))
        .header("x-session-token", token)
        .json(&serde_json::json!({ "body": "v2", "by": "mallory" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(updated["updated_by"], "alice");
    assert_eq!(updated["body"], "v2");
}
