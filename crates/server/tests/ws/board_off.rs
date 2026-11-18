//! Turning the Loca Board off while tasks are not done: the server asks
//! first (409 with the counts), and the caller chooses to keep them (they
//! come back with the board), to mark them done, or to remove them.

use super::lead_tasks::{answer, id_of, Loca, ROOT};
use super::loca_board::board_on;
use super::*;

async fn root(
    loca: &Loca,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let mut request = loca
        .client
        .request(method, format!("{}/rooms/proj{path}", loca.base))
        .header("x-admin-token", ROOT);
    if let Some(body) = body {
        request = request.json(&body);
    }
    answer(request).await
}

/// A board with one task in each unfinished stage and one done: `proj`,
/// the Lead `lead-ada`, the holder `bob`. Answers the four task ids.
async fn busy_board() -> (Loca, [u64; 4]) {
    let loca = Loca::open().await;
    let ada = loca.seat("lead-ada").await;
    let bob = loca.seat("bob").await;
    board_on(&loca, "lead-ada").await;
    let mut ids = [0; 4];
    for (i, title) in ["to do", "doing", "in review", "finished"]
        .iter()
        .enumerate()
    {
        let (status, task) = loca.declare(Some(&ada), title, Some("bob")).await;
        assert_eq!(status, 201, "{task}");
        ids[i] = id_of(&task);
    }
    for id in &ids[1..] {
        let (status, v) = loca
            .patch(
                Some(&bob),
                *id,
                serde_json::json!({"status": "taken", "by": "x"}),
            )
            .await;
        assert_eq!(status, 200, "{v}");
    }
    for id in &ids[2..] {
        let (status, v) = loca
            .patch(
                Some(&bob),
                *id,
                serde_json::json!({"status": "done", "by": "x"}),
            )
            .await;
        assert_eq!(status, 200, "{v}");
    }
    let (status, v) = loca
        .patch(
            None,
            ids[3],
            serde_json::json!({"status": "done", "by": "x"}),
        )
        .await;
    assert_eq!(status, 200, "{v}");
    (loca, ids)
}

async fn statuses(loca: &Loca) -> Vec<String> {
    let tasks: Vec<Value> = get_json(&loca.client, format!("{}/rooms/proj/tasks", loca.base)).await;
    tasks
        .iter()
        .map(|task| task["status"].as_str().unwrap().to_string())
        .collect()
}

async fn board_is_on(loca: &Loca) -> bool {
    let (_, view) = root(loca, reqwest::Method::GET, "/extensions", None).await;
    view["enabled"]
        .as_array()
        .unwrap()
        .iter()
        .any(|id| id == "board")
}

async fn journal(loca: &Loca) -> Vec<String> {
    let lines: Vec<Value> =
        get_json(&loca.client, format!("{}/rooms/proj/journal", loca.base)).await;
    lines
        .iter()
        .map(|line| line["text"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Asked first: removing the board, or a set without it, answers 409 with
/// the counts and changes nothing.
#[tokio::test]
async fn turning_the_board_off_with_unfinished_tasks_asks_first() {
    let (loca, _) = busy_board().await;
    for (method, path, body) in [
        (reqwest::Method::DELETE, "/extensions/board", None),
        (
            reqwest::Method::PUT,
            "/extensions",
            Some(serde_json::json!({"enabled": ["notes", "focus", "journal"]})),
        ),
    ] {
        let (status, asked) = root(&loca, method.clone(), path, body).await;
        assert_eq!(status, 409, "{method} {path}: {asked}");
        assert_eq!(
            asked["unfinished"],
            serde_json::json!({"open": 1, "taken": 1, "review": 1}),
            "{asked}"
        );
        assert_eq!(
            asked["choices"],
            serde_json::json!(["keep", "done", "remove"])
        );
        assert!(
            asked["error"].as_str().unwrap().contains("3 tasks"),
            "{asked}"
        );
    }
    assert!(board_is_on(&loca).await);
    assert_eq!(statuses(&loca).await, ["open", "taken", "review", "done"]);
    // A choice that is not one of the three is refused.
    let (status, _) = root(
        &loca,
        reqwest::Method::DELETE,
        "/extensions/board?open_tasks=later",
        None,
    )
    .await;
    assert_eq!(status, 400);
}

/// Keep: the board goes, the tasks stay exactly as they were, and come back.
#[tokio::test]
async fn keep_turns_the_board_off_and_the_tasks_come_back_unchanged() {
    let (loca, _) = busy_board().await;
    let (status, view) = root(
        &loca,
        reqwest::Method::DELETE,
        "/extensions/board?open_tasks=keep",
        None,
    )
    .await;
    assert_eq!(status, 200, "{view}");
    assert!(!board_is_on(&loca).await);
    assert!(
        journal(&loca)
            .await
            .iter()
            .any(|line| line == "extensions: removed Loca Board (3 unfinished tasks kept)"),
        "{:?}",
        journal(&loca).await
    );
    let (status, _) = root(&loca, reqwest::Method::PUT, "/extensions/board", None).await;
    assert_eq!(status, 200);
    assert_eq!(statuses(&loca).await, ["open", "taken", "review", "done"]);
}

/// Done and remove close every unfinished task, then the board goes.
#[tokio::test]
async fn done_or_remove_closes_every_unfinished_task_first() {
    for (choice, closed, line) in [
        (
            "done",
            "done",
            "extensions: removed Loca Board (3 tasks marked done)",
        ),
        (
            "remove",
            "cancelled",
            "extensions: removed Loca Board (3 tasks removed)",
        ),
    ] {
        let (loca, _) = busy_board().await;
        let (status, view) = root(
            &loca,
            reqwest::Method::DELETE,
            &format!("/extensions/board?open_tasks={choice}"),
            None,
        )
        .await;
        assert_eq!(status, 200, "{choice}: {view}");
        assert!(!board_is_on(&loca).await);
        assert!(
            journal(&loca).await.iter().any(|text| text == line),
            "{choice}: {:?}",
            journal(&loca).await
        );
        let (status, _) = root(&loca, reqwest::Method::PUT, "/extensions/board", None).await;
        assert_eq!(status, 200);
        assert_eq!(
            statuses(&loca).await,
            [closed, closed, closed, "done"],
            "{choice}"
        );
    }
}

/// With nothing unfinished there is nothing to ask.
#[tokio::test]
async fn a_board_with_only_finished_tasks_turns_off_without_a_question() {
    let loca = Loca::open().await;
    let ada = loca.seat("lead-ada").await;
    board_on(&loca, "lead-ada").await;
    let (_, task) = loca.declare(Some(&ada), "finished", None).await;
    let (status, _) = loca
        .patch(
            None,
            id_of(&task),
            serde_json::json!({"status": "done", "by": "x"}),
        )
        .await;
    assert_eq!(status, 200);
    let (status, view) = root(&loca, reqwest::Method::DELETE, "/extensions/board", None).await;
    assert_eq!(status, 200, "{view}");
    assert!(!board_is_on(&loca).await);
    assert!(journal(&loca)
        .await
        .iter()
        .any(|line| line == "extensions: removed Loca Board"));
}
