//! A task's review history: every request for review, go and no-go, with
//! who, when, and the note or reason, kept in order and across a restart.
//! Read-only: no route writes it; the review moves record it.

use super::lead_tasks::{id_of, Loca};
use super::loca_board::{board_on, turn_board_on};
use super::*;

fn history(task: &Value) -> Vec<(String, String, Option<String>)> {
    task["reviews"]
        .as_array()
        .map(|reviews| {
            reviews
                .iter()
                .map(|review| {
                    (
                        review["kind"].as_str().unwrap_or_default().to_string(),
                        review["by"].as_str().unwrap_or_default().to_string(),
                        review["text"].as_str().map(str::to_string),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn every_ask_go_and_no_go_is_kept_with_who_when_and_why() {
    let loca = Loca::open().await;
    let ada = loca.seat("lead-ada").await;
    let bob = loca.seat("bob").await;
    board_on(&loca, "lead-ada").await;
    let (_, task) = loca.declare(Some(&ada), "fix the login", Some("bob")).await;
    let id = id_of(&task);
    for (who, body) in [
        (&bob, serde_json::json!({"status": "taken"})),
        (
            &bob,
            serde_json::json!({"status": "done", "note": "first try"}),
        ),
        (
            &ada,
            serde_json::json!({"status": "taken", "reason": "the expiry path is untested"}),
        ),
        (
            &bob,
            serde_json::json!({"status": "done", "note": "expiry covered"}),
        ),
        (&ada, serde_json::json!({"status": "done"})),
    ] {
        let mut body = body;
        body["by"] = "x".into();
        let (status, v) = loca.patch(Some(who), id, body).await;
        assert_eq!(status, 200, "{v}");
    }
    let task = loca.task(id).await;
    assert_eq!(task["status"], "done");
    assert_eq!(
        history(&task),
        [
            ("asked".into(), "bob".into(), Some("first try".into())),
            (
                "nogo".into(),
                "lead-ada".into(),
                Some("the expiry path is untested".into())
            ),
            ("asked".into(), "bob".into(), Some("expiry covered".into())),
            ("go".into(), "lead-ada".into(), None),
        ]
    );
    let times: Vec<u64> = task["reviews"]
        .as_array()
        .unwrap()
        .iter()
        .map(|review| review["at"].as_u64().unwrap())
        .collect();
    assert!(times.windows(2).all(|pair| pair[0] <= pair[1]), "{times:?}");
    assert!(times[0] > 0);
}

#[tokio::test]
async fn the_review_history_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("reviews.db").to_string_lossy().to_string();
    let client = reqwest::Client::new();
    let id = {
        let (port, _guard) = spawn_server_env("MASTER", &[("DB_PATH", db.clone())]).await;
        create_test_room(port, "MASTER", "proj").await;
        let base = format!("http://127.0.0.1:{port}");
        turn_board_on(&base, "MASTER", "proj", "operator").await;
        let task: Value = client
            .post(format!("{base}/rooms/proj/tasks"))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({"title": "kept", "by": "operator"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let id = id_of(&task);
        for body in [
            serde_json::json!({"status": "review", "note": "please look", "by": "operator"}),
            serde_json::json!({"status": "taken", "reason": "not yet", "by": "operator"}),
        ] {
            let r = client
                .patch(format!("{base}/rooms/proj/tasks/{id}"))
                .header("x-admin-token", "MASTER")
                .json(&body)
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 200);
        }
        id
    };
    let (port, _guard) = spawn_server_env("MASTER", &[("DB_PATH", db)]).await;
    let task: Value = client
        .get(format!("http://127.0.0.1:{port}/rooms/proj/tasks/{id}"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        history(&task),
        [
            (
                "asked".into(),
                "operator".into(),
                Some("please look".into())
            ),
            ("nogo".into(), "operator".into(), Some("not yet".into())),
        ]
    );
}
