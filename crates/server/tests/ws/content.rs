//! Durable notes, memory search, and canonical content identity.

use super::*;

#[tokio::test]
async fn memory_ownership_does_not_transfer_to_a_reused_name() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("principal-owner.db");
    let (port, _guard) = spawn_server_env(
        "MASTER",
        &[
            ("DB_PATH", db.to_string_lossy().into_owned()),
            ("REQUIRE_INVITE", "1".into()),
            ("REQUIRE_SESSIONS", "1".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let admitted: Value = client
        .post(format!("{base}/members"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({"name":"alice","kind":"agent"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let davet = davet_for(&base, "MASTER", "general", "alice").await;
    let session = |davet: String| {
        let client = client.clone();
        let base = base.clone();
        async move {
            client
                .post(format!("{base}/sessions"))
                .header("x-room-token", davet)
                .json(&serde_json::json!({"name":"alice","kind":"agent","loca":"general"}))
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()["session_token"]
                .as_str()
                .unwrap()
                .to_string()
        }
    };
    let old_session = session(davet).await;
    client
        .put(format!("{base}/rooms/general/memory/owner"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({"owner":"alice"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    assert_eq!(
        client
            .put(format!("{base}/rooms/general/memory/short"))
            .header("x-session-token", &old_session)
            .json(&serde_json::json!({"text":"original"}))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    client
        .delete(format!(
            "{base}/members/{}",
            admitted["token"].as_str().unwrap()
        ))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let new_davet = davet_for(&base, "MASTER", "general", "alice").await;
    let new_session = session(new_davet).await;
    for (token, expected) in [(&old_session, 401), (&new_session, 403)] {
        let response = client
            .put(format!("{base}/rooms/general/memory/short"))
            .header("x-session-token", token)
            .json(&serde_json::json!({"text":"stolen"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
    client
        .put(format!("{base}/rooms/general/memory/owner"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({"owner":"alice"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    assert_eq!(
        client
            .put(format!("{base}/rooms/general/memory/short"))
            .header("x-session-token", new_session)
            .json(&serde_json::json!({"text":"explicitly reassigned"}))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
}

#[tokio::test]
async fn session_optional_sandbox_still_rejects_a_supplied_invalid_actor() {
    let (port, _guard) = spawn_server().await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let created = client
        .post(format!("{base}/rooms/general/journal"))
        .json(&serde_json::json!({"text":"sandbox compatibility", "by":"dev", "by_type":"user"}))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);
    assert_eq!(
        client
            .post(format!("{base}/rooms/general/journal"))
            .header("x-session-token", "invalid")
            .json(&serde_json::json!({"text":"invalid is not absent", "by":"forged"}))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        client
            .delete(format!("{base}/rooms/general/notes/key"))
            .header("x-session-token", "invalid")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
}

#[tokio::test]
async fn journal_and_note_delete_require_a_valid_actor() {
    let (port, _guard) = spawn_server_env("", &[("REQUIRE_SESSIONS", "1".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let session: Value = client
        .post(format!("{base}/sessions"))
        .json(&serde_json::json!({"name":"alice","kind":"user"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = session["session_token"].as_str().unwrap();
    client
        .post(format!("{base}/rooms/general/notes"))
        .header("x-session-token", token)
        .json(&serde_json::json!({"key":"protected","body":"keep","by":"alice"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    for invalid in [None, Some("invalid-session")] {
        let mut journal = client
            .post(format!("{base}/rooms/general/journal"))
            .json(&serde_json::json!({"text":"spoofed","by":"bob","by_type":"agent"}));
        let mut delete = client.delete(format!("{base}/rooms/general/notes/protected"));
        if let Some(token) = invalid {
            journal = journal.header("x-session-token", token);
            delete = delete.header("x-session-token", token);
        }
        assert_eq!(journal.send().await.unwrap().status(), 401);
        assert_eq!(delete.send().await.unwrap().status(), 401);
    }
    let entry: Value = client
        .post(format!("{base}/rooms/general/journal"))
        .header("x-session-token", token)
        .json(&serde_json::json!({"text":"finished","by":"bob","by_type":"agent"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(entry["by"], "alice");
    assert_eq!(entry["by_type"], "user");
    assert_eq!(
        client
            .delete(format!("{base}/rooms/general/notes/protected"))
            .header("x-session-token", token)
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
}

#[tokio::test]
async fn notes_create_update_and_soft_permission_push_live() {
    let (port, _guard) = spawn_server().await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // A watcher WS connection observes live note frames.
    let mut watcher = connect_ws(port, "general", "watcher", "user").await;
    wait_for_member(&client, &base, "general", "watcher", None).await;

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

#[tokio::test]
async fn loca_memory_has_no_delete_surface_and_only_its_owner_can_write() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("memory-fences.db");
    let (port, _guard) =
        spawn_server_env("", &[("DB_PATH", db.to_string_lossy().into_owned())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let session_for = |name: &'static str| {
        let client = client.clone();
        let base = base.clone();
        async move {
            let value: Value = client
                .post(format!("{base}/sessions"))
                .json(&serde_json::json!({ "name": name, "kind": "agent" }))
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .json()
                .await
                .unwrap();
            value["session_token"].as_str().unwrap().to_string()
        }
    };
    let alice = session_for("alice").await;
    let bob = session_for("bob").await;
    let _alice_seat = connect_ws(port, "general", "alice", "agent").await;
    let _bob_seat = connect_ws(port, "general", "bob", "agent").await;

    // The server must create the production schema. The fixture only seeds
    // owners after boot, so removing either production DDL makes this fail.
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "INSERT INTO loca_memory (room, owner) VALUES ('general', 'alice')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO loca_memory (room, owner) VALUES ('ownerless', NULL)",
        [],
    )
    .unwrap();
    drop(conn);

    // A1: even a valid, seated identity has no DELETE operation on any memory
    // route. Testing all three closes the route-by-route regression surface.
    for path in ["memory", "memory/short", "memory/entries"] {
        let delete = client
            .delete(format!("{base}/rooms/general/{path}"))
            .header("x-session-token", &bob)
            .send()
            .await
            .unwrap();
        assert_eq!(
            delete.status(),
            reqwest::StatusCode::METHOD_NOT_ALLOWED,
            "A1 fence: {path} must expose no DELETE operation"
        );
    }

    // A2(a): the assigned owner writes.
    let owner_write = client
        .put(format!("{base}/rooms/general/memory/short"))
        .header("x-session-token", &alice)
        .json(&serde_json::json!({"text": "current context"}))
        .send()
        .await
        .unwrap();
    assert_eq!(owner_write.status(), reqwest::StatusCode::OK);
    let short_before: Value = owner_write.json().await.unwrap();
    let short_updated_at = short_before["short_updated_at"].as_u64().unwrap();
    tokio::time::sleep(Duration::from_millis(2)).await;

    let long_entry: Value = client
        .post(format!("{base}/rooms/general/memory/entries"))
        .header("x-session-token", &alice)
        .json(&serde_json::json!({"text": "we chose durable provenance"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(long_entry["decided_by"], "alice");
    assert_eq!(long_entry["text"], "we chose durable provenance");
    let after_long: Value = client
        .get(format!("{base}/rooms/general/memory"))
        .header("x-session-token", &alice)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        after_long["short_updated_at"].as_u64(),
        Some(short_updated_at),
        "A6 fence: a long write must not refresh short memory"
    );
    client
        .post(format!("{base}/rooms/general/memory/entries"))
        .header("x-session-token", &alice)
        .json(&serde_json::json!({"text": "and retained every decision"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let memory: Value = client
        .get(format!("{base}/rooms/general/memory"))
        .header("x-session-token", &alice)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        memory["long"], "we chose durable provenance\n\nand retained every decision",
        "long memory must represent the complete ordered decision history"
    );

    // A10-G1/G3: provenance is readable in stable id order, with an id cursor
    // that always names the final row in a non-terminal page.
    let first_page: Value = client
        .get(format!(
            "{base}/rooms/general/memory/entries?after_id=0&limit=1"
        ))
        .header("x-session-token", &alice)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let first_id = first_page["entries"][0]["id"].as_u64().unwrap();
    assert_eq!(first_page["next_after_id"].as_u64(), Some(first_id));
    assert_eq!(first_page["entries"][0]["decided_by"], "alice");
    let fields = first_page["entries"][0]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        fields,
        ["decided_at", "decided_by", "id", "text"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        "the provenance list must not mislabel aggregate room state as entry state"
    );
    let second_page: Value = client
        .get(format!(
            "{base}/rooms/general/memory/entries?after_id={first_id}&limit=1"
        ))
        .header("x-session-token", &alice)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(second_page["entries"][0]["id"].as_u64().unwrap() > first_id);
    assert!(second_page["next_after_id"].is_null());

    // Legacy rows are not assigned invented provenance.
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "UPDATE loca_memory_entries SET decided_by = '', decided_at = 0 WHERE id = ?1",
        [first_id],
    )
    .unwrap();
    drop(conn);
    let legacy_page: Value = client
        .get(format!("{base}/rooms/general/memory/entries?limit=1"))
        .header("x-session-token", &alice)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(legacy_page["entries"][0]["decided_by"].is_null());
    assert!(legacy_page["entries"][0]["decided_at"].is_null());

    let absent = client
        .get(format!("{base}/rooms/unconfigured/memory/entries"))
        .header("x-session-token", &alice)
        .send()
        .await
        .unwrap();
    assert_eq!(absent.status(), reqwest::StatusCode::NOT_FOUND);
    assert_eq!(
        absent.text().await.unwrap(),
        "memory is not configured for this loca"
    );

    // A2(b): another seated identity in the same loca is rejected.
    let other_write = client
        .put(format!("{base}/rooms/general/memory/short"))
        .header("x-session-token", &bob)
        .json(&serde_json::json!({"text": "overwrite"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        other_write.status(),
        reqwest::StatusCode::FORBIDDEN,
        "A2 fence: a seated non-owner must not write loca memory"
    );
    assert_eq!(
        other_write.text().await.unwrap(),
        "only the memory owner may write"
    );
    let oversized_other = client
        .put(format!("{base}/rooms/general/memory/short"))
        .header("x-session-token", &bob)
        .json(&serde_json::json!({"text": "x".repeat(4097)}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        oversized_other.status(),
        reqwest::StatusCode::FORBIDDEN,
        "A2 precedence fence: ownership must be checked before short-memory size"
    );

    // A2(c): absence of an owner is explicit, never a swallowed no-op.
    let ownerless = client
        .put(format!("{base}/rooms/ownerless/memory/short"))
        .header("x-session-token", &alice)
        .json(&serde_json::json!({"text": "lost"}))
        .send()
        .await
        .unwrap();
    assert_eq!(ownerless.status(), reqwest::StatusCode::CONFLICT);
    assert_eq!(
        ownerless.text().await.unwrap(),
        "memory owner is not assigned for this loca"
    );

    // A2-EK: archiving freezes both write surfaces while the existing memory
    // remains readable.
    client
        .put(format!("{base}/rooms/general/settings"))
        .json(&serde_json::json!({"archived": true}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    for (method, path) in [("PUT", "memory/short"), ("POST", "memory/entries")] {
        let request = match method {
            "PUT" => client.put(format!("{base}/rooms/general/{path}")),
            _ => client.post(format!("{base}/rooms/general/{path}")),
        };
        let response = request
            .header("x-session-token", &alice)
            .json(&serde_json::json!({"text": "must stay frozen"}))
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            reqwest::StatusCode::CONFLICT,
            "A2-EK fence: archived {path} write must be rejected"
        );
    }
    assert_eq!(
        client
            .get(format!("{base}/rooms/general/memory"))
            .header("x-session-token", &alice)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::OK,
        "A2-EK fence: archived memory must remain readable"
    );
    assert_eq!(
        client
            .get(format!("{base}/rooms/general/memory/entries"))
            .header("x-session-token", &alice)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::OK,
        "A2-EK fence: archived provenance must remain readable"
    );

    // A10-G1 red-proof: direct/legacy writes to flattened long cannot create
    // a second truth that the provenance endpoint silently serves.
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute(
            "UPDATE loca_memory SET long = long || '\n\nHAYALET' WHERE room = 'general'",
            [],
        )
        .unwrap();
    let divergent = client
        .get(format!("{base}/rooms/general/memory/entries"))
        .header("x-session-token", &alice)
        .send()
        .await
        .unwrap();
    assert_eq!(divergent.status(), reqwest::StatusCode::CONFLICT);
    assert!(divergent.text().await.unwrap().contains("inconsistent"));
}

#[tokio::test]
async fn loca_memory_budget_is_visible_hard_bounded_and_never_discards_history() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("memory-budget.db");
    let (port, _guard) =
        spawn_server_env("", &[("DB_PATH", db.to_string_lossy().into_owned())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let session: Value = client
        .post(format!("{base}/sessions"))
        .json(&serde_json::json!({"name": "owner", "kind": "agent"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let session = session["session_token"].as_str().unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "INSERT INTO loca_memory (room, owner) VALUES ('budget', 'owner')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO loca_memory (room, owner) VALUES ('entry-budget', 'owner')",
        [],
    )
    .unwrap();
    drop(conn);

    let short_at_limit = "ş".repeat(2 * 1024); // 4096 UTF-8 bytes.
    let short_ok = client
        .put(format!("{base}/rooms/budget/memory/short"))
        .header("x-session-token", session)
        .json(&serde_json::json!({"text": short_at_limit}))
        .send()
        .await
        .unwrap();
    assert_eq!(short_ok.status(), reqwest::StatusCode::OK);
    let short_too_large = client
        .put(format!("{base}/rooms/budget/memory/short"))
        .header("x-session-token", session)
        .json(&serde_json::json!({"text": format!("{}x", "ş".repeat(2 * 1024))}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        short_too_large.status(),
        reqwest::StatusCode::PAYLOAD_TOO_LARGE,
        "A9 short fence: UTF-8 byte length beyond 4096 must be rejected"
    );

    let at_entry_limit = client
        .post(format!("{base}/rooms/entry-budget/memory/entries"))
        .header("x-session-token", session)
        .json(&serde_json::json!({"text": "ğ".repeat(4 * 1024)}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        at_entry_limit.status(),
        reqwest::StatusCode::CREATED,
        "K10(a): exactly 8192 UTF-8 bytes must remain writable"
    );
    let over_entry_limit = client
        .post(format!("{base}/rooms/entry-budget/memory/entries"))
        .header("x-session-token", session)
        .json(&serde_json::json!({"text": format!("{}x", "ğ".repeat(4 * 1024))}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        over_entry_limit.status(),
        reqwest::StatusCode::PAYLOAD_TOO_LARGE,
        "K10(b): 8193 UTF-8 bytes must be rejected"
    );
    assert!(over_entry_limit
        .text()
        .await
        .unwrap()
        .contains("wake-injection budget"));

    let mut at_soft = Value::Null;
    for bytes in [8192, 8192, 8192, 8186] {
        at_soft = client
            .post(format!("{base}/rooms/budget/memory/entries"))
            .header("x-session-token", session)
            .json(&serde_json::json!({"text": "a".repeat(bytes)}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
    }
    assert_eq!(at_soft["over_budget"], false);

    let over_soft: Value = client
        .post(format!("{base}/rooms/budget/memory/entries"))
        .header("x-session-token", session)
        .json(&serde_json::json!({"text": "b"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        over_soft["over_budget"], true,
        "A9 soft fence: crossing 32 KiB must be visible while the write succeeds"
    );

    // Reach the 64 KiB aggregate limit using individually injectable entries.
    // Separators count only between entries, never before the first entry.
    for bytes in [8192, 8192, 8192, 8181] {
        client
            .post(format!("{base}/rooms/budget/memory/entries"))
            .header("x-session-token", session)
            .json(&serde_json::json!({"text": "c".repeat(bytes)}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
    }

    let before: Value = client
        .get(format!("{base}/rooms/budget/memory"))
        .header("x-session-token", session)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(before["over_budget"], true);
    let before_long = before["long"].as_str().unwrap().to_string();
    assert_eq!(before_long.len(), 64 * 1024);
    let before_entries: i64 = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row(
            "SELECT count(*) FROM loca_memory_entries WHERE room = 'budget'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    let hard = client
        .post(format!("{base}/rooms/budget/memory/entries"))
        .header("x-session-token", session)
        .json(&serde_json::json!({"text": "x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        hard.status(),
        reqwest::StatusCode::PAYLOAD_TOO_LARGE,
        "A9 hard fence: a write beyond 64 KiB must be rejected"
    );
    let error = hard.text().await.unwrap();
    assert!(error.contains("finite append-only ledger (64 KiB)"));
    assert!(
        !error.contains("consolidate"),
        "never recommend an unavailable recovery command"
    );

    let after: Value = client
        .get(format!("{base}/rooms/budget/memory"))
        .header("x-session-token", session)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let after_long = after["long"].as_str().unwrap();
    assert_eq!(
        after_long.len(),
        before_long.len(),
        "A9 retention fence: rejection must not change old memory length"
    );
    assert_eq!(after_long, before_long);
    let after_entries: i64 = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row(
            "SELECT count(*) FROM loca_memory_entries WHERE room = 'budget'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(after_entries, before_entries);
}

async fn assert_cross_loca_memory_denied(method: &str, path: &str) {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("memory-isolation.db");
    let (port, _guard) = spawn_server_env(
        "MASTER",
        &[
            ("DB_PATH", db.to_string_lossy().into_owned()),
            ("REQUIRE_INVITE", "1".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let b_davet = davet_for(&base, "MASTER", "loca-b", "agent-b").await;
    let b_session =
        session_with(&base, ("x-room-token", &b_davet), "agent-b", Some("loca-b")).await;
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "INSERT INTO loca_memory (room, owner, short) VALUES ('loca-a', 'agent-b', 'secret-a')",
        [],
    )
    .unwrap();
    drop(conn);

    let url = format!("{base}/rooms/loca-a/{path}");
    let request = match method {
        "GET" => client.get(url),
        "PUT" => client.put(url).json(&serde_json::json!({"text": "steal"})),
        _ => client.post(url).json(&serde_json::json!({"text": "steal"})),
    };
    let response = request
        .header("x-session-token", &b_session)
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "A3 fence: loca-b identity must not access loca-a {path}"
    );
}

#[tokio::test]
async fn loca_memory_isolation_rejects_cross_loca_read() {
    assert_cross_loca_memory_denied("GET", "memory").await;
    assert_cross_loca_memory_denied("GET", "memory/entries").await;
    assert_cross_loca_memory_denied("GET", "memory/snapshot").await;
}

#[tokio::test]
async fn loca_memory_isolation_rejects_cross_loca_short_write() {
    assert_cross_loca_memory_denied("PUT", "memory/short").await;
}

#[tokio::test]
async fn loca_memory_isolation_rejects_cross_loca_long_write() {
    assert_cross_loca_memory_denied("POST", "memory/entries").await;
}

#[tokio::test]
async fn memory_only_deployment_reports_permanent_unavailability() {
    let (port, _guard) = spawn_server().await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let session: Value = client
        .post(format!("{base}/sessions"))
        .json(&serde_json::json!({"name": "owner", "kind": "agent"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let response = client
        .put(format!("{base}/rooms/general/memory/short"))
        .header(
            "x-session-token",
            session["session_token"].as_str().unwrap(),
        )
        .json(&serde_json::json!({"text": "cannot persist"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    let message = response.text().await.unwrap();
    assert!(message.contains("requires persistent storage"));
    assert!(
        !message.contains("try again"),
        "A5-EK fence: a permanent deployment limitation must not look transient"
    );
    let entries = client
        .get(format!("{base}/rooms/general/memory/entries"))
        .header(
            "x-session-token",
            session["session_token"].as_str().unwrap(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(entries.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        entries.text().await.unwrap(),
        "loca memory requires persistent storage in this deployment"
    );
}

#[tokio::test]
async fn fresh_connection_automatically_receives_ready_memory() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("memory-frame-ready.db");
    let (port, _guard) =
        spawn_server_env("", &[("DB_PATH", db.to_string_lossy().into_owned())]).await;
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute_batch(
            "INSERT INTO loca_memory
             (room, owner, short, long, short_updated_at, long_updated_at, version)
             VALUES ('remembered', 'owner', 'short fact', 'long decision', 10, 20, 2);
             INSERT INTO loca_memory_entries
             (room, text, decided_by, decided_at)
             VALUES ('remembered', 'long decision', 'owner', 20);",
        )
        .unwrap();

    let mut ws = connect_ws(port, "remembered", "owner", "agent").await;
    let frame = wait_for(&mut ws, |frame| frame["t"] == "memory").await;
    assert_eq!(frame["room"], "remembered");
    assert_eq!(frame["status"], "ready");
    assert_eq!(frame["short"], "short fact");
    assert_eq!(frame["long"], "long decision");
    assert_eq!(frame["version"], 2);

    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let session: Value = client
        .post(format!("{base}/sessions"))
        .json(&serde_json::json!({"name": "owner", "kind": "agent"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    client
        .post(format!("{base}/rooms/remembered/memory/entries"))
        .header(
            "x-session-token",
            session["session_token"].as_str().unwrap(),
        )
        .json(&serde_json::json!({"text": "new decision"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let updated = wait_for(&mut ws, |frame| {
        frame["t"] == "memory" && frame["version"] == 3
    })
    .await;
    assert_eq!(updated["long"], "long decision\n\nnew decision");
    assert!(updated["long_updated_at"].as_u64().is_some());

    client
        .post(format!("{base}/rooms/remembered/messages"))
        .json(&serde_json::json!({
            "sender": "operator",
            "sender_type": "user",
            "target": "owner",
            "text": "wake"
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let checkpoint = wait_for(&mut ws, |frame| frame["t"] == "memoryversion").await;
    assert_eq!(checkpoint["room"], "remembered");
    assert_eq!(checkpoint["version"], 3);
    let message = wait_for(&mut ws, |frame| frame["t"] == "msg").await;
    assert_eq!(message["message"]["text"], "wake");

    let prior_long_clock = updated["long_updated_at"].clone();
    client
        .put(format!("{base}/rooms/remembered/memory/short"))
        .header(
            "x-session-token",
            session["session_token"].as_str().unwrap(),
        )
        .json(&serde_json::json!({"text": "new short"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let short_only = wait_for(&mut ws, |frame| {
        frame["t"] == "memory" && frame["version"] == 4
    })
    .await;
    assert_eq!(short_only["short"], "new short");
    assert_ne!(short_only["short_updated_at"], 10);
    assert_eq!(short_only["long_updated_at"], prior_long_clock);
}

#[tokio::test]
async fn memory_frame_selects_a_bounded_complete_entry_suffix() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("memory-frame-bounded.db");
    let (port, _guard) =
        spawn_server_env("MASTER", &[("DB_PATH", db.to_string_lossy().into_owned())]).await;
    let exact = "ö".repeat(4 * 1024); // exactly 8192 UTF-8 bytes
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "INSERT INTO loca_memory
         (room, owner, long, long_updated_at, version)
         VALUES ('exact-entry', 'owner', ?1, 20, 1)",
        [&exact],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO loca_memory_entries (room, text, decided_by, decided_at)
         VALUES ('exact-entry', ?1, 'owner', 20)",
        [&exact],
    )
    .unwrap();

    let half_old = "a".repeat(4096);
    let half_new = "b".repeat(4096);
    let split_long = format!("{half_old}\n\n{half_new}");
    conn.execute(
        "INSERT INTO loca_memory (room, owner, long, long_updated_at, version)
         VALUES ('separator-budget', 'owner', ?1, 20, 2)",
        [&split_long],
    )
    .unwrap();
    for text in [&half_old, &half_new] {
        conn.execute(
            "INSERT INTO loca_memory_entries (room, text, decided_by, decided_at)
             VALUES ('separator-budget', ?1, 'owner', 20)",
            [text],
        )
        .unwrap();
    }

    let old = "old".repeat(100);
    let legacy = "x".repeat(9 * 1024);
    let newest = "new".repeat(100);
    let legacy_long = format!("{old}\n\n{legacy}\n\n{newest}");
    conn.execute(
        "INSERT INTO loca_memory (room, owner, long, long_updated_at, version)
         VALUES ('legacy-entry', 'owner', ?1, 20, 3)",
        [&legacy_long],
    )
    .unwrap();
    let mut legacy_id = 0u64;
    for text in [&old, &legacy, &newest] {
        conn.execute(
            "INSERT INTO loca_memory_entries (room, text, decided_by, decided_at)
             VALUES ('legacy-entry', ?1, 'owner', 20)",
            [text],
        )
        .unwrap();
        if text.len() > 8 * 1024 {
            legacy_id = conn.last_insert_rowid() as u64;
        }
    }
    drop(conn);

    let mut ws = connect_ws(port, "exact-entry", "owner", "agent").await;
    let frame = wait_for(&mut ws, |frame| frame["t"] == "memory").await;
    assert_eq!(frame["long"].as_str().unwrap().len(), 8 * 1024);
    assert_eq!(frame["long"], exact);
    assert_eq!(frame["long_truncated"], false);
    assert_eq!(frame["long_omitted_bytes"], 0);
    assert_eq!(frame["long_omitted_entries"], 0);

    let mut ws = connect_ws(port, "separator-budget", "owner", "agent").await;
    let frame = wait_for(&mut ws, |frame| frame["t"] == "memory").await;
    assert_eq!(frame["long"], half_new);
    assert_eq!(frame["long_truncated"], true);
    assert_eq!(frame["long_omitted_entries"], 1);

    let mut ws = connect_ws(port, "legacy-entry", "owner", "agent").await;
    let frame = wait_for(&mut ws, |frame| frame["t"] == "memory").await;
    assert_eq!(frame["long"], format!("{old}\n\n{newest}"));
    assert_eq!(frame["long_uninjectable_entries"], 1);
    assert_eq!(
        frame["long_uninjectable_entry_ids"],
        serde_json::json!([legacy_id])
    );
    assert_eq!(frame["long_omitted_entries"], 1);
}

#[tokio::test]
async fn memory_frame_distinguishes_absent_from_empty() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("memory-frame-status.db");
    let (port, _guard) =
        spawn_server_env("MASTER", &[("DB_PATH", db.to_string_lossy().into_owned())]).await;
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute(
            "INSERT INTO loca_memory (room, owner) VALUES ('empty-memory', 'owner')",
            [],
        )
        .unwrap();

    let mut absent = connect_ws(port, "absent-memory", "absent-agent", "agent").await;
    let absent_frame = wait_for(&mut absent, |frame| frame["t"] == "memory").await;
    let mut empty = connect_ws(port, "empty-memory", "owner", "agent").await;
    let empty_frame = wait_for(&mut empty, |frame| frame["t"] == "memory").await;

    assert_eq!(absent_frame["status"], "absent");
    assert_eq!(empty_frame["status"], "empty");
    assert_ne!(
        absent_frame["status"], empty_frame["status"],
        "A5 fence: absent and empty memory must remain distinguishable"
    );
}

#[tokio::test]
async fn memory_frame_exposes_inconsistent_provenance_as_a_red_status() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("memory-frame-inconsistent.db");
    let (port, _guard) =
        spawn_server_env("MASTER", &[("DB_PATH", db.to_string_lossy().into_owned())]).await;
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute(
            "INSERT INTO loca_memory (room, owner, long, version)
             VALUES ('inconsistent-memory', 'owner', 'ghost text', 1)",
            [],
        )
        .unwrap();

    let mut ws = connect_ws(port, "inconsistent-memory", "owner", "agent").await;
    let frame = wait_for(&mut ws, |frame| frame["t"] == "memory").await;
    assert_eq!(frame["status"], "inconsistent");
    assert_eq!(frame["long"], "");
    assert_eq!(frame["long_truncated"], true);
    assert_eq!(frame["long_omitted_bytes"], 10);
    assert_eq!(frame["long_omitted_entries"], 0);
}

#[tokio::test]
async fn absent_memory_is_an_explicit_frame_not_silence() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("memory-frame-absent.db");
    let (port, _guard) =
        spawn_server_env("MASTER", &[("DB_PATH", db.to_string_lossy().into_owned())]).await;

    let mut ws = connect_ws(port, "no-memory-row", "agent", "agent").await;
    let frame = wait_for(&mut ws, |frame| frame["t"] == "memory").await;
    assert_eq!(
        frame["status"], "absent",
        "A5 fence: a missing row must still produce an explicit memory frame"
    );
    assert_eq!(frame["version"], 0);
}

#[tokio::test]
async fn caretaker_memory_is_metadata_only_and_owner_assignment_is_master_only() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("memory-care.db");
    let (port, _guard) = spawn_server_env(
        "MASTER",
        &[
            ("DB_PATH", db.to_string_lossy().into_owned()),
            ("LOCA_CARETAKERS", "loca-care".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let admit = |name: &'static str| {
        let client = client.clone();
        let base = base.clone();
        async move {
            client
                .post(format!("{base}/members"))
                .header("x-admin-token", "MASTER")
                .json(&serde_json::json!({"name": name, "kind": "agent"}))
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()
        }
    };
    let caretaker = admit("loca-care").await;
    let ordinary = admit("ordinary").await;

    let denied = client
        .put(format!("{base}/rooms/general/memory/owner"))
        .header("x-room-token", ordinary["token"].as_str().unwrap())
        .json(&serde_json::json!({"owner": "ordinary"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        denied.status(),
        reqwest::StatusCode::FORBIDDEN,
        "A2 owner fence: an ordinary seat must not assign memory ownership"
    );

    let assigned = client
        .put(format!("{base}/rooms/general/memory/owner"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({"owner": "ordinary"}))
        .send()
        .await
        .unwrap();
    assert_eq!(assigned.status(), reqwest::StatusCode::NO_CONTENT);
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "UPDATE loca_memory SET short='PRIVATE SHORT', long='PRIVATE LONG',
         short_updated_at=10, long_updated_at=20 WHERE room='general'",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO loca_memory_entries (room, text, decided_by, decided_at)
         VALUES ('general', 'legacy decision', '', 30)",
        [],
    )
    .unwrap();
    drop(conn);

    let rows: Vec<Value> = client
        .get(format!("{base}/care/memory"))
        .header("x-room-token", caretaker["token"].as_str().unwrap())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let general = rows.iter().find(|row| row["loca"] == "general").unwrap();
    for field in [
        "loca",
        "owner",
        "short_updated_at",
        "long_updated_at",
        "long_entries_without_provenance",
        "over_budget",
    ] {
        assert!(
            general.get(field).is_some(),
            "metadata field missing: {field}"
        );
    }
    assert_eq!(
        general["long_entries_without_provenance"], 1,
        "A7 caretaker fence: empty provenance must be counted"
    );
    assert!(
        general.get("short").is_none() && general.get("long").is_none(),
        "A3 caretaker fence: memory bodies must never appear in /care/memory"
    );
    let encoded = serde_json::to_string(&rows).unwrap();
    assert!(!encoded.contains("PRIVATE SHORT") && !encoded.contains("PRIVATE LONG"));
}
