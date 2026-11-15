//! Durable notes, memory search, and canonical content identity.

use super::*;

#[tokio::test]
async fn wiki_http_editor_revision_and_private_room_boundaries() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("wiki.db");
    let (port, _guard) = spawn_server_env(
        "MASTER",
        &[
            ("DB_PATH", db.to_string_lossy().into_owned()),
            ("REQUIRE_INVITE", "1".into()),
            ("REQUIRE_SESSIONS", "1".into()),
        ],
    )
    .await;
    create_test_room(port, "MASTER", "general").await;
    create_test_room(port, "MASTER", "private").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let mut sessions = Vec::new();
    for name in ["editor", "reader"] {
        client
            .post(format!("{base}/members"))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({"name":name,"kind":"agent"}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        let invite = invite_for(&base, "MASTER", "general", name).await;
        let response: Value = client
            .post(format!("{base}/sessions"))
            .header("x-room-token", invite)
            .json(&serde_json::json!({"name":name,"kind":"agent","loca":"general"}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        sessions.push(response["session_token"].as_str().unwrap().to_owned());
    }
    assert_eq!(
        client
            .put(format!("{base}/rooms/general/wiki/config"))
            .header("x-session-token", &sessions[0])
            .json(&serde_json::json!({"editor":"editor","interval_messages":30}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    client
        .put(format!("{base}/rooms/general/wiki/config"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({"editor":"editor","interval_messages":30}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let update = serde_json::json!({"expected_revision":0,"reviewed_through":0,"reason":"review",
        "pages":[{"slug":"working","title":"Working area","body":"proposal","sources":[]}]});
    assert_eq!(
        client
            .post(format!("{base}/rooms/general/wiki/review"))
            .header("x-session-token", &sessions[1])
            .json(&update)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let receipt: Value = client
        .post(format!("{base}/rooms/general/wiki/review"))
        .header("x-session-token", &sessions[0])
        .json(&update)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(receipt["revision"], 1);
    assert_eq!(
        client
            .post(format!("{base}/rooms/general/wiki/review"))
            .header("x-session-token", &sessions[0])
            .json(&update)
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    let snapshot: Value = client
        .get(format!("{base}/rooms/general/wiki"))
        .header("x-session-token", &sessions[1])
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(snapshot["revision"], 1);
    assert_eq!(snapshot["editor_name"], "editor");
    let selected: Value = client
        .get(format!("{base}/rooms/general/wiki/pages/working"))
        .header("x-session-token", &sessions[1])
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(selected["room"], "general");
    assert_eq!(selected["page"]["body"], "proposal");
    assert!(selected.get("pages").is_none());
    assert_eq!(
        client
            .get(format!("{base}/rooms/general/wiki/pages/missing"))
            .header("x-session-token", &sessions[1])
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        client
            .get(format!("{base}/rooms/private/wiki/pages/working"))
            .header("x-session-token", &sessions[1])
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert!(snapshot["pages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["body"] == "proposal"));
    let topic = serde_json::json!({"expected_revision":1,"reviewed_through":0,"reason":"topic",
        "pages":[{"slug":"obsolete","title":"Obsolete","body":"retained in history","sources":[]}]});
    assert!(client
        .post(format!("{base}/rooms/general/wiki/review"))
        .header("x-session-token", &sessions[0])
        .json(&topic)
        .send()
        .await
        .unwrap()
        .status()
        .is_success());
    let removal = serde_json::json!({"expected_revision":2,"reason":"merged into working"});
    let url = format!("{base}/rooms/general/wiki/pages/obsolete");
    assert_eq!(
        client
            .delete(&url)
            .header("x-session-token", &sessions[1])
            .json(&removal)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        client
            .delete(&url)
            .header("x-session-token", &sessions[0])
            .json(&serde_json::json!({"expected_revision":1,"reason":"stale"}))
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    assert_eq!(
        client
            .delete(format!("{base}/rooms/private/wiki/pages/obsolete"))
            .header("x-session-token", &sessions[0])
            .json(&removal)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let receipt: Value = client
        .delete(&url)
        .header("x-session-token", &sessions[0])
        .json(&removal)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(receipt["revision"], 3);
    assert_eq!(
        client
            .get(&url)
            .header("x-session-token", &sessions[0])
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    let after: Value = client
        .get(format!("{base}/rooms/general/wiki"))
        .header("x-session-token", &sessions[1])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(after["pages"].as_array().unwrap().len(), 2);
    assert_eq!(
        client
            .get(format!("{base}/rooms/private/wiki"))
            .header("x-session-token", &sessions[1])
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        client
            .put(format!("{base}/rooms/general/wiki/config"))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({"editor":"editor","enabled":true,"interval_messages":30}))
            .send()
            .await
            .unwrap()
            .status(),
        409
    );

    // Protocol E2E only: deliberately seed an enabled bounded batch in this
    // disposable database. Public configuration remains fail-closed until
    // native isolated-worker conformance is available.
    let source: Value = client
        .post(format!("{base}/rooms/general/messages"))
        .header("x-session-token", &sessions[1])
        .json(
            &serde_json::json!({"sender":"reader","sender_type":"agent","text":"sourced finding"}),
        )
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let source_id = source["id"].as_u64().unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute("UPDATE loca_wiki SET enabled=1 WHERE room='general'", [])
        .unwrap();
    conn.execute("INSERT INTO loca_wiki_batches(room,editor_principal,after_id,through_id,reason,created_at)
        SELECT room,editor_principal,reviewed_through,?1,'test-source',1 FROM loca_wiki WHERE room='general'",
        [source_id]).unwrap();
    drop(conn);
    assert_eq!(
        client
            .post(format!("{base}/rooms/general/wiki/maintenance/claim"))
            .header("x-session-token", &sessions[1])
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        client
            .get(format!("{base}/rooms/private/wiki/maintenance"))
            .header("x-session-token", &sessions[1])
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let batch: Value = client
        .post(format!("{base}/rooms/general/wiki/maintenance/claim"))
        .header("x-session-token", &sessions[0])
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(batch["messages"][0]["text"], "sourced finding");
    assert_eq!(batch["through_id"], source_id);
    assert_eq!(
        client
            .post(format!("{base}/rooms/general/wiki/maintenance/renew"))
            .header("x-session-token", &sessions[1])
            .json(&batch["lease"])
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    let renewal: Value = client
        .post(format!("{base}/rooms/general/wiki/maintenance/renew"))
        .header("x-session-token", &sessions[0])
        .json(&batch["lease"])
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        renewal["lease_expires_at"].as_u64().unwrap()
            >= batch["lease_expires_at"].as_u64().unwrap()
    );
    let mut finish = serde_json::json!({"lease":batch["lease"],"review":{
        "expected_revision":3,"reviewed_through":source_id,"reason":"delegated bounded review",
        "pages":[{"slug":"working","title":"Working area","body":"supported finding","sources":[source_id]}]}});
    finish["lease"]["epoch"] = serde_json::json!(99);
    assert_eq!(
        client
            .post(format!("{base}/rooms/general/wiki/maintenance/finish"))
            .header("x-session-token", &sessions[0])
            .json(&finish)
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    finish["lease"] = batch["lease"].clone();
    let result: Value = client
        .post(format!("{base}/rooms/general/wiki/maintenance/finish"))
        .header("x-session-token", &sessions[0])
        .json(&finish)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(result["revision"], 4);
    let retry: Value = client
        .post(format!("{base}/rooms/general/wiki/maintenance/finish"))
        .header("x-session-token", &sessions[0])
        .json(&finish)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(retry, result);
    let mut divergent = finish.clone();
    divergent["review"]["reason"] = serde_json::json!("different retry");
    assert_eq!(
        client
            .post(format!("{base}/rooms/general/wiki/maintenance/finish"))
            .header("x-session-token", &sessions[0])
            .json(&divergent)
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    let readback: Value = client
        .get(format!("{base}/rooms/general/wiki"))
        .header("x-session-token", &sessions[1])
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(readback["reviewed_through"], source_id);
    assert!(readback["pages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["body"] == "supported finding"));
    let state: Value = client
        .get(format!("{base}/rooms/general/wiki/maintenance"))
        .header("x-session-token", &sessions[1])
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(state["status"], "completed");
    // A human page edit has separate operator authority and does not pretend
    // to review more messages or leave editor citations attached to new prose.
    let edit = serde_json::json!({"expected_revision":readback["revision"],"title":"Working area","body":"operator correction"});
    for session in &sessions {
        assert_eq!(
            client
                .put(format!("{base}/rooms/general/wiki/pages/working"))
                .header("x-session-token", session)
                .json(&edit)
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
    }
    assert_eq!(client.put(format!("{base}/rooms/general/wiki/pages/working"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({"expected_revision":0,"title":"Working area","body":"stale edit"}))
        .send().await.unwrap().status(), 409);
    let edited: Value = client
        .put(format!("{base}/rooms/general/wiki/pages/working"))
        .header("x-admin-token", "MASTER")
        .json(&edit)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(edited["room"], "general");
    assert_eq!(edited["slug"], "working");
    assert_eq!(
        edited["revision"].as_u64().unwrap(),
        readback["revision"].as_u64().unwrap() + 1
    );
    let after_edit: Value = client
        .get(format!("{base}/rooms/general/wiki"))
        .header("x-session-token", &sessions[1])
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(after_edit["reviewed_through"], readback["reviewed_through"]);
    assert_eq!(after_edit["reviewed_at"], readback["reviewed_at"]);
    let working = after_edit["pages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|page| page["slug"] == "working")
        .unwrap();
    assert_eq!(working["body"], "operator correction");
    assert_eq!(working["sources"], serde_json::json!([]));
    let invalid = serde_json::json!({"expected_revision":edited["revision"],"title":"Working area","body":"bad","reviewed_through":source_id+100});
    assert_eq!(
        client
            .put(format!("{base}/rooms/general/wiki/pages/working"))
            .header("x-admin-token", "MASTER")
            .json(&invalid)
            .send()
            .await
            .unwrap()
            .status(),
        422
    );
    assert_eq!(
        client
            .put(format!("{base}/rooms/private/wiki/pages/working"))
            .header("x-session-token", &sessions[1])
            .json(&edit)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
}

/// PRINCIPLES "Wiki editor": "there is no separate Memory role or second
/// owner assignment. Old records are archives, not new authority." An
/// upgraded Building still holds a memory record that names an owner bound to
/// its principal. That owner writes nothing, the Master assigns nobody, and
/// the archived record stays readable on every read surface, unchanged.
#[tokio::test]
async fn archived_memory_owner_grants_nothing_and_its_record_stays_readable() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("archived-owner.db");
    let (port, _guard) = spawn_server_env(
        "MASTER",
        &[
            ("DB_PATH", db.to_string_lossy().into_owned()),
            ("REQUIRE_INVITE", "1".into()),
            ("REQUIRE_SESSIONS", "1".into()),
        ],
    )
    .await;
    create_test_room(port, "MASTER", "general").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let invite = invite_for(&base, "MASTER", "general", "alice").await;
    let session = session_with(&base, ("x-room-token", &invite), "alice", Some("general")).await;
    let profiles: Value = client
        .get(format!("{base}/profiles"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let alice_id = profiles
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["display_name"] == "alice")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "INSERT INTO loca_memory
         (room, owner, owner_principal_id, short, long, short_updated_at, long_updated_at, version)
         VALUES ('general', 'alice', ?1, 'archived state', 'archived decision',
                 1700000000000, 1700000100000, 3)",
        [&alice_id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO loca_memory_entries (room, text, decided_by, decided_at)
         VALUES ('general', 'archived decision', 'alice', 1700000100000)",
        [],
    )
    .unwrap();
    drop(conn);
    let stored = |db: &std::path::Path| {
        let conn = rusqlite::Connection::open(db).unwrap();
        let row: (Option<String>, String, String, u64) = conn
            .query_row(
                "SELECT owner, short, long, version FROM loca_memory WHERE room = 'general'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        let entries: i64 = conn
            .query_row("SELECT count(*) FROM loca_memory_entries", [], |r| r.get(0))
            .unwrap();
        (row, entries)
    };
    let before = stored(&db);

    // The archived owner, seated with its own invite, writes nothing.
    for (method, path) in [("PUT", "memory/short"), ("POST", "memory/entries")] {
        let url = format!("{base}/rooms/general/{path}");
        let request = match method {
            "PUT" => client.put(url),
            _ => client.post(url),
        };
        let response = request
            .header("x-session-token", &session)
            .json(&serde_json::json!({"text": "new authority"}))
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            reqwest::StatusCode::FORBIDDEN,
            "the archived memory owner must not write {path}"
        );
        assert!(
            response.text().await.unwrap().contains("read-only"),
            "the refusal says plainly that archived memory is read-only"
        );
    }
    // Nobody assigns an owner, the Master included; clearing is no exception.
    for owner in [serde_json::json!("alice"), Value::Null] {
        let response = client
            .put(format!("{base}/rooms/general/memory/owner"))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({ "owner": owner }))
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            reqwest::StatusCode::FORBIDDEN,
            "there is no Memory owner to assign (owner {owner})"
        );
    }
    assert_eq!(stored(&db), before, "the archived record must not change");

    // Every read surface still serves the archive.
    let memory: Value = client
        .get(format!("{base}/rooms/general/memory"))
        .header("x-session-token", &session)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(memory["short"], "archived state");
    assert_eq!(memory["long"], "archived decision");
    assert_eq!(memory["owner"], "alice");
    let entries: Value = client
        .get(format!("{base}/rooms/general/memory/entries"))
        .header("x-session-token", &session)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(entries["entries"][0]["text"], "archived decision");
    assert_eq!(entries["entries"][0]["decided_by"], "alice");
    let snapshot: Value = client
        .get(format!("{base}/rooms/general/memory/snapshot"))
        .header("x-session-token", &session)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(snapshot["status"], "ready");
    assert_eq!(snapshot["short"], "archived state");
    let mut ws = connect_ws_protocols(
        format!("ws://127.0.0.1:{port}/ws?room=general&name=alice&type=agent"),
        &["loca.v1".into(), format!("loca.room.{invite}")],
    )
    .await;
    let frame = wait_for(&mut ws, |frame| frame["t"] == "memory").await;
    assert_eq!(frame["status"], "ready");
    assert_eq!(frame["short"], "archived state");
    assert_eq!(frame["version"], 3);
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
        .json(&serde_json::json!({ "sender": "a", "sender_type": "user", "text": "we settled auth: session-bound" }))
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
        .any(|m| m["text"].as_str().unwrap().contains("settled")));
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
async fn loca_memory_has_no_delete_surface_and_its_archive_stays_readable() {
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
    // the archive after boot, so removing either production DDL makes this
    // fail. Memory is no longer written through the server: an archived
    // record, as an upgraded Building holds one, is seeded directly.
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch(
        "INSERT INTO loca_memory (room, owner, short, long, short_updated_at, long_updated_at, version)
         VALUES ('general', 'alice', 'current context',
                 'we chose durable provenance\n\nand retained every decision',
                 1700000000000, 1700000100000, 3);
         INSERT INTO loca_memory_entries (room, text, decided_by, decided_at)
         VALUES ('general', 'we chose durable provenance', 'alice', 1700000050000),
                ('general', 'and retained every decision', 'alice', 1700000100000);
         INSERT INTO loca_memory (room, owner) VALUES ('ownerless', NULL);",
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

    // A2: nobody writes the archive, neither the owner it names, another
    // seated identity, nor anyone in a loca whose record names no owner.
    for (token, room) in [
        (&alice, "general"),
        (&bob, "general"),
        (&alice, "ownerless"),
    ] {
        for (method, path) in [("PUT", "memory/short"), ("POST", "memory/entries")] {
            let url = format!("{base}/rooms/{room}/{path}");
            let request = match method {
                "PUT" => client.put(url),
                _ => client.post(url),
            };
            let response = request
                .header("x-session-token", token)
                .json(&serde_json::json!({"text": "overwrite"}))
                .send()
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                reqwest::StatusCode::FORBIDDEN,
                "A2 fence: {room} {path} must not be written"
            );
        }
    }

    // A2-EK: an archived loca's memory remains readable.
    client
        .put(format!("{base}/rooms/general/settings"))
        .json(&serde_json::json!({"archived": true}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
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

/// The 32 KiB warning of an archived ledger stays visible, the whole ledger
/// stays readable, and a refused write changes nothing.
#[tokio::test]
async fn archived_memory_budget_stays_visible_and_the_ledger_never_changes() {
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
    // A full archived ledger: eight 8 KiB decisions, 64 KiB with separators.
    let entries: Vec<String> = (0..8u8)
        .map(|i| char::from(b'a' + i).to_string().repeat(8190))
        .collect();
    let long = entries.join("\n\n");
    assert_eq!(long.len(), 64 * 1024 - 2);
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "INSERT INTO loca_memory (room, owner, long, long_updated_at, version)
         VALUES ('budget', 'owner', ?1, 1700000000000, 8)",
        [&long],
    )
    .unwrap();
    for text in &entries {
        conn.execute(
            "INSERT INTO loca_memory_entries (room, text, decided_by, decided_at)
             VALUES ('budget', ?1, 'owner', 1700000000000)",
            [text],
        )
        .unwrap();
    }
    drop(conn);
    let count = |db: &std::path::Path| -> i64 {
        rusqlite::Connection::open(db)
            .unwrap()
            .query_row(
                "SELECT count(*) FROM loca_memory_entries WHERE room = 'budget'",
                [],
                |row| row.get(0),
            )
            .unwrap()
    };

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
    assert_eq!(before["long"], long.as_str());
    let page: Value = client
        .get(format!("{base}/rooms/budget/memory/entries?limit=200"))
        .header("x-session-token", session)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(page["entries"].as_array().unwrap().len(), 8);

    let refused = client
        .post(format!("{base}/rooms/budget/memory/entries"))
        .header("x-session-token", session)
        .json(&serde_json::json!({"text": "x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), reqwest::StatusCode::FORBIDDEN);

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
    assert_eq!(
        after["long"], before["long"],
        "A9 retention fence: a refused write must not change the ledger"
    );
    assert_eq!(after["version"], before["version"]);
    assert_eq!(count(&db), 8);
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
    create_test_room(port, "MASTER", "loca-a").await;
    create_test_room(port, "MASTER", "loca-b").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let b_invite = invite_for(&base, "MASTER", "loca-b", "agent-b").await;
    let b_session = session_with(
        &base,
        ("x-room-token", &b_invite),
        "agent-b",
        Some("loca-b"),
    )
    .await;
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
        .get(format!("{base}/rooms/general/memory"))
        .header(
            "x-session-token",
            session["session_token"].as_str().unwrap(),
        )
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

    // The archive is no longer written; each delivered message is still
    // preceded by its version checkpoint.
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
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
    assert_eq!(checkpoint["version"], 2);
    let message = wait_for(&mut ws, |frame| frame["t"] == "msg").await;
    assert_eq!(message["message"]["text"], "wake");
}

#[tokio::test]
async fn memory_frame_selects_a_bounded_complete_entry_suffix() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("memory-frame-bounded.db");
    let (port, _guard) =
        spawn_server_env("MASTER", &[("DB_PATH", db.to_string_lossy().into_owned())]).await;
    create_test_room(port, "MASTER", "exact-entry").await;
    create_test_room(port, "MASTER", "separator-budget").await;
    create_test_room(port, "MASTER", "legacy-entry").await;
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
    create_test_room(port, "MASTER", "absent-memory").await;
    create_test_room(port, "MASTER", "empty-memory").await;
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
    create_test_room(port, "MASTER", "inconsistent-memory").await;
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
    create_test_room(port, "MASTER", "no-memory-row").await;

    let mut ws = connect_ws(port, "no-memory-row", "agent", "agent").await;
    let frame = wait_for(&mut ws, |frame| frame["t"] == "memory").await;
    assert_eq!(
        frame["status"], "absent",
        "A5 fence: a missing row must still produce an explicit memory frame"
    );
    assert_eq!(frame["version"], 0);
}

#[tokio::test]
async fn caretaker_memory_is_metadata_only_and_nobody_assigns_an_owner() {
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
    create_test_room(port, "MASTER", "general").await;
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

    for (header, credential) in [
        ("x-room-token", ordinary["token"].as_str().unwrap()),
        ("x-admin-token", "MASTER"),
    ] {
        let denied = client
            .put(format!("{base}/rooms/general/memory/owner"))
            .header(header, credential)
            .json(&serde_json::json!({"owner": "ordinary"}))
            .send()
            .await
            .unwrap();
        assert_eq!(
            denied.status(),
            reqwest::StatusCode::FORBIDDEN,
            "A2 owner fence: there is no Memory owner to assign ({header})"
        );
    }

    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "INSERT INTO loca_memory
         (room, owner, short, long, short_updated_at, long_updated_at, version)
         VALUES ('general', 'ordinary', 'PRIVATE SHORT', 'PRIVATE LONG',
                 1700000000000, 1700000100000, 2)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO loca_memory_entries (room, text, decided_by, decided_at)
         VALUES ('general', 'legacy decision', '', 1700000200000)",
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
