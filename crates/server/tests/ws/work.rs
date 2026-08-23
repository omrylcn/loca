//! Goal, task, journal, and atomic work-state lifecycle.

use super::*;

#[tokio::test]
async fn gorev_lifecycle_and_operator_authority() {
    // Admin token set -> authority is real, not dev-open.
    let (port, _guard) = spawn_server_with("buyuk").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // An agent cannot declare a task — proposing happens in chat.
    let r = client
        .post(format!("{base}/rooms/proj/tasks"))
        .json(&serde_json::json!({ "title": "x", "by": "some-agent" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);

    // The grand operator appoints a loca operator for this loca only.
    client
        .put(format!("{base}/rooms/proj/settings"))
        .header("x-admin-token", "buyuk")
        .json(&serde_json::json!({ "operators": ["alt-op"] }))
        .send()
        .await
        .unwrap();

    // The loca operator declares a görev, assigned to an agent.
    let t: Value = client
        .post(format!("{base}/rooms/proj/tasks"))
        .json(
            &serde_json::json!({ "title": "login bug", "by": "alt-op", "assigned_to": "backend" }),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(t["status"], "open");
    let tid = t["id"].as_u64().unwrap();

    // Assignment is ownership, not a suggestion: an unrelated agent cannot
    // steal an already assigned task by naming itself in the patch.
    let steal = client
        .patch(format!("{base}/rooms/proj/tasks/{tid}"))
        .json(&serde_json::json!({
            "assigned_to": "stranger",
            "by": "stranger"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(steal.status(), 403);
    assert_eq!(
        client
            .get(format!("{base}/rooms/proj/tasks"))
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()[0]["assigned_to"],
        "backend",
        "a rejected steal must not mutate the assignment"
    );

    // ...but NOT in someone else's loca.
    let other = client
        .post(format!("{base}/rooms/other/tasks"))
        .json(&serde_json::json!({ "title": "x", "by": "alt-op" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        other.status(),
        403,
        "a loca operator's authority ends at their loca"
    );

    // The assigned agent takes it, then finishes it.
    for st in ["taken", "done"] {
        let r: Value = client
            .patch(format!("{base}/rooms/proj/tasks/{tid}"))
            .json(&serde_json::json!({ "status": st, "by": "backend" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(r["status"], st);
    }

    // Another agent may not cancel; the operator may reopen (contest).
    let no = client
        .patch(format!("{base}/rooms/proj/tasks/{tid}"))
        .json(&serde_json::json!({ "status": "cancelled", "by": "stranger" }))
        .send()
        .await
        .unwrap();
    assert_eq!(no.status(), 403);
    let re: Value = client
        .patch(format!("{base}/rooms/proj/tasks/{tid}"))
        .json(&serde_json::json!({ "status": "open", "by": "alt-op" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(re["status"], "open");
}

#[tokio::test]
async fn one_active_goal_can_close_when_its_explicit_task_set_is_done() {
    let (port, _guard) = spawn_server_with("buyuk").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    client
        .put(format!("{base}/rooms/proj/settings"))
        .header("x-admin-token", "buyuk")
        .json(&serde_json::json!({ "operators": ["alt-op"] }))
        .send()
        .await
        .unwrap();
    client
        .post(format!("{base}/rooms/proj/lead"))
        .header("x-admin-token", "buyuk")
        .json(&serde_json::json!({ "lead": "alt-op" }))
        .send()
        .await
        .unwrap();

    let mut task_ids = Vec::new();
    for title in ["listener stable", "smoke green"] {
        let task: Value = client
            .post(format!("{base}/rooms/proj/tasks"))
            .json(&serde_json::json!({
                "title": title, "by": "alt-op", "assigned_to": "worker"
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        task_ids.push(task["id"].as_u64().unwrap());
    }
    let goal: Value = client
        .post(format!("{base}/rooms/proj/goals"))
        .json(&serde_json::json!({
            "outcome": "public release ready",
            "completion": "all_tasks",
            "task_ids": task_ids,
            "by": "alt-op"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(goal["status"], "active");
    let progress_at = goal["progress_at"].as_u64().unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let reordered: Value = client
        .patch(format!(
            "{base}/rooms/proj/goals/{}",
            goal["id"].as_u64().unwrap()
        ))
        .json(&serde_json::json!({
            "task_ids": [task_ids[1], task_ids[0]], "by": "alt-op"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(reordered["task_ids"], serde_json::json!(task_ids));
    assert_eq!(
        reordered["progress_at"], progress_at,
        "canonical-equivalent task ordering must not manufacture goal progress"
    );

    let second = client
        .post(format!("{base}/rooms/proj/goals"))
        .json(&serde_json::json!({
            "outcome": "competing focus", "by": "alt-op"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), 409, "a loca has only one active goal");

    for (index, id) in task_ids.iter().enumerate() {
        let updated: Value = client
            .patch(format!("{base}/rooms/proj/tasks/{id}"))
            .json(&serde_json::json!({ "status": "done", "by": "worker" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(updated["status"], "done");
        let goals: Value = client
            .get(format!("{base}/rooms/proj/goals"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            goals[0]["status"],
            if index + 1 == task_ids.len() {
                "achieved"
            } else {
                "active"
            }
        );
    }
}

/// The journal is the record of what was already done: written by whoever did
/// it, never assigned, never closed, and never rewritten. A restart must not
/// be able to forget it — that is the whole difference between a record and a
/// status board.
#[tokio::test]
async fn journal_records_work_and_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("j.db").to_string_lossy().to_string();
    let client = reqwest::Client::new();

    let entry: Value = {
        let (port, _g) = spawn_server_env("master", &[("DB_PATH", db.clone())]).await;
        let base = format!("http://127.0.0.1:{port}");
        let e: Value = client.post(format!("{base}/rooms/general/journal"))
            .json(&serde_json::json!({ "text": "davet sistemi prod'a çıktı", "by": "loca-dev", "by_type": "agent" }))
            .send().await.unwrap().json().await.unwrap();
        assert_eq!(e["by"], "loca-dev");
        assert_eq!(e["id"], 1, "entries are numbered from one, per loca");

        // An empty entry says nothing and is refused.
        let bad = client
            .post(format!("{base}/rooms/general/journal"))
            .json(&serde_json::json!({ "text": "   " }))
            .send()
            .await
            .unwrap();
        assert_eq!(bad.status(), 400);
        e
    };

    // Same DB, new process: the record is still there.
    let (port, _g) = spawn_server_env("master", &[("DB_PATH", db)]).await;
    let base = format!("http://127.0.0.1:{port}");
    let all: Value = client
        .get(format!("{base}/rooms/general/journal"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let rows = all.as_array().unwrap();
    assert_eq!(rows.len(), 1, "the journal outlives the process");
    assert_eq!(rows[0]["text"], entry["text"]);
}

/// A loca may legitimately begin with declared work or a journal entry before
/// anyone speaks or creates a note. Those records must be enough to restore
/// the loca: boot cannot discover rooms only from messages/notes/room settings.
#[tokio::test]
async fn work_state_only_rooms_survive_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir
        .path()
        .join("work-only.db")
        .to_string_lossy()
        .to_string();
    let client = reqwest::Client::new();

    {
        let (port, _guard) = spawn_server_env("MASTER", &[("DB_PATH", db.clone())]).await;
        let base = format!("http://127.0.0.1:{port}");

        let task: Value = client
            .post(format!("{base}/rooms/task-only/tasks"))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({
                "title": "restart güvenini doğrula",
                "by": "operator"
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        client
            .post(format!("{base}/rooms/task-only/lead"))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({ "lead": "operator" }))
            .send()
            .await
            .unwrap();
        let goal = client
            .post(format!("{base}/rooms/task-only/goals"))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({
                "outcome": "restart-safe work state",
                "completion": "all_tasks",
                "task_ids": [task["id"].as_u64().unwrap()],
                "by": "operator"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(goal.status(), 201);
        let wait = client
            .post(format!("{base}/rooms/wait-only/waits"))
            .json(&serde_json::json!({
                "by": "worker", "waiting_for": "reviewer",
                "reason": "review contract"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(wait.status(), 201);

        let journal = client
            .post(format!("{base}/rooms/journal-only/journal"))
            .json(&serde_json::json!({
                "text": "ilk kayıt konuşmadan önce yazıldı",
                "by": "loca-dev",
                "by_type": "agent"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(journal.status(), 201);
    }

    let (port, _guard) = spawn_server_env("MASTER", &[("DB_PATH", db)]).await;
    let base = format!("http://127.0.0.1:{port}");

    let tasks: Value = client
        .get(format!("{base}/rooms/task-only/tasks"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        tasks.as_array().map(Vec::len),
        Some(1),
        "task-only loca disappeared across restart"
    );
    let goals: Value = client
        .get(format!("{base}/rooms/task-only/goals"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(goals[0]["status"], "active", "goal disappeared on restart");
    let waits: Value = client
        .get(format!("{base}/rooms/wait-only/waits"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(waits[0]["waiting_for"], "reviewer");

    let journal: Value = client
        .get(format!("{base}/rooms/journal-only/journal"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        journal.as_array().map(Vec::len),
        Some(1),
        "journal-only loca disappeared across restart"
    );
}

/// A successful HTTP mutation means the write reached SQLite. When storage is
/// unavailable the server must answer 503 and leave its in-memory view alone;
/// otherwise a restart would erase a change the caller was told had succeeded.
#[tokio::test]
async fn storage_failures_are_503_and_do_not_mutate_memory() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir
        .path()
        .join("broken-writes.db")
        .to_string_lossy()
        .to_string();
    let (port, _guard) = spawn_server_env("MASTER", &[("DB_PATH", db.clone())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let sql = rusqlite::Connection::open(&db).unwrap();

    sql.execute_batch("DROP TABLE tasks").unwrap();
    let task = client
        .post(format!("{base}/rooms/task-fail/tasks"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "title": "must persist", "by": "operator" }))
        .send()
        .await
        .unwrap();
    assert_eq!(task.status(), 503);
    let tasks: Value = client
        .get(format!("{base}/rooms/task-fail/tasks"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(tasks.as_array().map(Vec::len), Some(0));

    sql.execute_batch("DROP TABLE goals").unwrap();
    client
        .post(format!("{base}/rooms/goal-fail/lead"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "lead": "operator" }))
        .send()
        .await
        .unwrap();
    let goal = client
        .post(format!("{base}/rooms/goal-fail/goals"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({
            "outcome": "must persist", "by": "operator"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(goal.status(), 503);
    let goals: Value = client
        .get(format!("{base}/rooms/goal-fail/goals"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(goals.as_array().map(Vec::len), Some(0));

    sql.execute_batch("DROP TABLE waits").unwrap();
    let wait = client
        .post(format!("{base}/rooms/wait-fail/waits"))
        .json(&serde_json::json!({
            "by": "worker", "waiting_for": "reviewer", "reason": "must persist"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(wait.status(), 503);
    let waits: Value = client
        .get(format!("{base}/rooms/wait-fail/waits"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(waits.as_array().map(Vec::len), Some(0));

    sql.execute_batch("DROP TABLE journal").unwrap();
    let journal = client
        .post(format!("{base}/rooms/journal-fail/journal"))
        .json(&serde_json::json!({ "text": "must persist", "by": "loca-dev" }))
        .send()
        .await
        .unwrap();
    assert_eq!(journal.status(), 503);
    let journal_rows: Value = client
        .get(format!("{base}/rooms/journal-fail/journal"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(journal_rows.as_array().map(Vec::len), Some(0));

    let created = client
        .post(format!("{base}/rooms/note-fail/notes"))
        .json(&serde_json::json!({
            "key": "k", "title": "before", "body": "old", "by": "operator"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);
    sql.execute_batch("DROP TABLE note_revisions").unwrap();
    let update = client
        .put(format!("{base}/rooms/note-fail/notes/k"))
        .json(&serde_json::json!({ "title": "after", "by": "operator" }))
        .send()
        .await
        .unwrap();
    assert_eq!(update.status(), 503);
    let unchanged: Value = client
        .get(format!("{base}/rooms/note-fail/notes/k"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(unchanged["title"], "before");

    let member: Value = client
        .post(format!("{base}/members"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "cyber", "kind": "agent" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(member["token"].as_str().is_some());
    sql.execute_batch("DROP TABLE invites").unwrap();
    let call = client
        .post(format!("{base}/rooms/invite-fail/call"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "cyber" }))
        .send()
        .await
        .unwrap();
    assert_eq!(call.status(), 503);
    let invites: Value = client
        .get(format!("{base}/rooms/invite-fail/invites"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(invites.as_array().map(Vec::len), Some(0));

    sql.execute_batch("DROP TABLE members").unwrap();
    let admit = client
        .post(format!("{base}/members"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "debug", "kind": "agent" }))
        .send()
        .await
        .unwrap();
    assert_eq!(admit.status(), 503);

    sql.execute_batch("DROP TABLE bans").unwrap();
    let moderation = client
        .post(format!("{base}/rooms/mod-fail/moderate"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "action": "mute", "name": "cyber" }))
        .send()
        .await
        .unwrap();
    assert_eq!(moderation.status(), 503);
    let state: Value = client
        .get(format!("{base}/rooms/mod-fail/moderate"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(state["muted"].as_array().map(Vec::len), Some(0));

    let room = client
        .post(format!("{base}/rooms/seal-fail/messages"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({
            "sender": "operator", "sender_type": "user", "text": "keep me"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(room.status(), 201);
    let archived = client
        .put(format!("{base}/rooms/seal-fail/settings"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "archived": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(archived.status(), 200);
    sql.execute_batch("DROP TABLE rooms").unwrap();
    let seal = client
        .delete(format!("{base}/rooms/seal-fail"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap();
    assert_eq!(seal.status(), 503);
    let history: Value = client
        .get(format!("{base}/rooms/seal-fail/messages"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(history.as_array().map(Vec::len), Some(1));
}

/// A linked task transition is one logical write: task, goal progress, and
/// care-marker resets either all commit or all remain unchanged. This guards
/// the failure window where a task used to advance before a goal trigger
/// rejected the second autocommit.
#[tokio::test]
async fn linked_task_and_goal_progress_commit_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir
        .path()
        .join("atomic-task-goal.db")
        .to_string_lossy()
        .to_string();
    let (port, _guard) = spawn_server_env("MASTER", &[("DB_PATH", db.clone())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let task: Value = client
        .post(format!("{base}/rooms/proj/tasks"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({
            "title": "atomic release", "assigned_to": "worker", "by": "operator"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    client
        .post(format!("{base}/rooms/proj/lead"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "lead": "operator" }))
        .send()
        .await
        .unwrap();
    let goal: Value = client
        .post(format!("{base}/rooms/proj/goals"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({
            "outcome": "atomic release is live",
            "completion": "all_tasks",
            "task_ids": [task["id"]],
            "by": "operator"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let task_progress = task["progress_at"].as_u64().unwrap();
    let goal_progress = goal["progress_at"].as_u64().unwrap();
    let sql = rusqlite::Connection::open(&db).unwrap();
    sql.execute_batch(
        "CREATE TRIGGER reject_linked_goal_progress
         BEFORE INSERT ON goals
         BEGIN SELECT RAISE(FAIL, 'injected goal failure'); END;",
    )
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;

    let failed = client
        .patch(format!("{base}/rooms/proj/tasks/{}", task["id"]))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "status": "taken", "by": "worker" }))
        .send()
        .await
        .unwrap();
    assert_eq!(failed.status(), 503);
    let tasks: Value = client
        .get(format!("{base}/rooms/proj/tasks"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let goals: Value = client
        .get(format!("{base}/rooms/proj/goals"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(tasks[0]["status"], "open");
    assert_eq!(tasks[0]["progress_at"], task_progress);
    assert_eq!(goals[0]["progress_at"], goal_progress);

    sql.execute_batch("DROP TRIGGER reject_linked_goal_progress")
        .unwrap();
    let repaired: Value = client
        .patch(format!("{base}/rooms/proj/tasks/{}", task["id"]))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "status": "taken", "by": "worker" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let goals: Value = client
        .get(format!("{base}/rooms/proj/goals"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(repaired["status"], "taken");
    assert!(repaired["progress_at"].as_u64().unwrap() > task_progress);
    assert!(goals[0]["progress_at"].as_u64().unwrap() > goal_progress);
}

#[tokio::test]
async fn goal_stale_override_can_inherit_set_disable_and_clear() {
    let (port, _guard) = spawn_server_with("MASTER").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    client
        .post(format!("{base}/rooms/proj/lead"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "lead": "operator" }))
        .send()
        .await
        .unwrap();
    let created: Value = client
        .post(format!("{base}/rooms/proj/goals"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({
            "outcome": "ship", "completion": "manual", "by": "operator"
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(created["stale_after_secs"].is_null());
    let id = created["id"].as_u64().unwrap();

    for (payload, expected) in [
        (
            serde_json::json!({"stale_after_secs": 60, "by": "operator"}),
            Some(60),
        ),
        (
            serde_json::json!({"stale_after_secs": 0, "by": "operator"}),
            Some(0),
        ),
    ] {
        let updated: Value = client
            .patch(format!("{base}/rooms/proj/goals/{id}"))
            .header("x-admin-token", "MASTER")
            .json(&payload)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(updated["stale_after_secs"].as_u64(), expected);
    }
    let inherited: Value = client
        .patch(format!("{base}/rooms/proj/goals/{id}"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({
            "stale_after_secs": null, "by": "operator"
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(inherited["stale_after_secs"].is_null());
}

/// With REQUIRE_SESSIONS a task cannot be declared under a spoofed operator
/// name — a mutation needs a session-bound identity (defence in depth for the
/// prod config).
#[tokio::test]
async fn require_sessions_blocks_task_spoof() {
    let (port, _g) = spawn_server_env("MASTER", &[("REQUIRE_SESSIONS", "1".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    // No session token, claiming to be "operator".
    let r = reqwest::Client::new()
        .post(format!("{base}/rooms/oda/tasks"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "title": "do this", "by": "operator" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        401,
        "a task mutation needs a real session, not a claimed name"
    );
}
