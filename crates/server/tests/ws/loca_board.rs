//! The Loca Board stands alone: it owns the loca's tasks (`/rooms/:id/tasks…`
//! and the `task` frame), needs a Lead and no longer Focus, and turning it
//! off keeps every stored task for when it comes back. Focus keeps the goal.

use super::lead_tasks::{answer, id_of, Loca, ROOT};
use super::*;

/// One request as the Building's root, to `proj`.
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

async fn extension(loca: &Loca, method: reqwest::Method, id: &str) -> (u16, Value) {
    root(loca, method, &format!("/extensions/{id}"), None).await
}

/// Turn the Loca Board on in `proj`, whose Lead is already named.
pub(super) async fn switch_board_on(loca: &Loca) {
    let (status, view) = extension(loca, reqwest::Method::PUT, "board").await;
    assert_eq!(status, 200, "turn the Loca Board on: {view}");
}

/// Turn the Loca Board on in `proj`, with `lead` as its Lead.
pub(super) async fn board_on(loca: &Loca, lead: &str) {
    loca.name_lead(lead).await;
    let (status, view) = extension(loca, reqwest::Method::PUT, "board").await;
    assert_eq!(status, 200, "turn the Loca Board on: {view}");
}

/// For tests elsewhere that use tasks: name `lead` the Lead of `room` and
/// turn its Loca Board on, as the Building's root `token`. Tasks live on the
/// board, so every task in it has the review step.
pub(super) async fn turn_board_on(base: &str, token: &str, room: &str, lead: &str) {
    let client = reqwest::Client::new();
    let named = client
        .post(format!("{base}/rooms/{room}/lead"))
        .header("x-admin-token", token)
        .json(&serde_json::json!({ "lead": lead }))
        .send()
        .await
        .unwrap();
    assert_eq!(named.status(), 200, "name {room}'s Lead");
    let (status, view) = answer(
        client
            .put(format!("{base}/rooms/{room}/extensions/board"))
            .header("x-admin-token", token),
    )
    .await;
    assert_eq!(status, 200, "turn {room}'s Loca Board on: {view}");
}

fn enabled(view: &Value) -> Vec<String> {
    view["enabled"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap().to_string())
        .collect()
}

fn says_board_is_off(refused: &Value) -> bool {
    refused
        .as_str()
        .is_some_and(|text| text.starts_with("The Loca Board is off in this loca"))
}

/// Without Focus the Loca Board turns on (with a Lead) and serves tasks.
#[tokio::test]
async fn the_board_turns_on_without_focus_and_serves_tasks() {
    let loca = Loca::open().await;
    let ada = loca.seat("lead-ada").await;
    let (status, view) = root(
        &loca,
        reqwest::Method::PUT,
        "/extensions",
        Some(serde_json::json!({"enabled": ["notes"]})),
    )
    .await;
    assert_eq!(status, 200, "{view}");
    board_on(&loca, "lead-ada").await;
    let (_, view) = root(&loca, reqwest::Method::GET, "/extensions", None).await;
    assert_eq!(enabled(&view), ["notes", "board"]);

    let (status, task) = loca
        .declare(Some(&ada), "write the upgrade note", None)
        .await;
    assert_eq!(status, 201, "{task}");
    assert_eq!(loca.tasks_count().await, 1);
    // Focus is still off: its goal routes are refused, the board's are not.
    let (status, refused) = root(&loca, reqwest::Method::GET, "/goals", None).await;
    assert_eq!(status, 403, "{refused}");
}

/// With Focus on and the board off (every loca from before extensions), the
/// task routes are the board's and refused, reads and writes alike, saying
/// plainly that the Loca Board is off.
#[tokio::test]
async fn the_task_routes_are_the_boards() {
    let loca = Loca::open().await;
    let bob = loca.seat("bob").await;
    for (method, path, body) in [
        (reqwest::Method::GET, "/tasks", None),
        (
            reqwest::Method::POST,
            "/tasks",
            Some(serde_json::json!({"title": "refused", "by": "x"})),
        ),
        (
            reqwest::Method::PATCH,
            "/tasks/1",
            Some(serde_json::json!({"status": "taken", "by": "x"})),
        ),
    ] {
        let (status, refused) = root(&loca, method.clone(), path, body.clone()).await;
        assert_eq!(status, 403, "{method} {path}: {refused}");
        assert!(says_board_is_off(&refused), "{method} {path}: {refused}");
        let mut request = loca
            .client
            .request(method.clone(), format!("{}/rooms/proj{path}", loca.base));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let (status, refused) = answer(bob.on(request)).await;
        assert_eq!(status, 403, "bob {method} {path}: {refused}");
    }
    // Focus still serves the goal.
    let (status, goals) = root(&loca, reqwest::Method::GET, "/goals", None).await;
    assert_eq!(status, 200, "{goals}");
}

/// The Lead rules are as before: no board without a Lead, no clearing the
/// Lead under it, no new loca opened with it. Removing Focus keeps it.
#[tokio::test]
async fn the_board_needs_a_lead_and_survives_focus() {
    let loca = Loca::open().await;
    let _ada = loca.seat("lead-ada").await;
    let (status, refused) = extension(&loca, reqwest::Method::PUT, "board").await;
    assert_eq!(status, 409, "{refused}");
    board_on(&loca, "lead-ada").await;
    let (status, refused) = root(
        &loca,
        reqwest::Method::POST,
        "/lead",
        Some(serde_json::json!({"lead": null})),
    )
    .await;
    assert_eq!(status, 409, "the Lead cleared under the board: {refused}");

    let (status, view) = extension(&loca, reqwest::Method::DELETE, "focus").await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["removed"], serde_json::json!(["focus"]));
    assert!(enabled(&view).contains(&"board".to_string()), "{view}");

    let (status, refused) = answer(
        loca.client
            .post(format!("{}/rooms", loca.base))
            .header("x-admin-token", ROOT)
            .json(&serde_json::json!({"name": "fresh", "extensions": ["board"]})),
    )
    .await;
    assert_eq!(status, 409, "a new loca opened with the board: {refused}");
}

/// Turning the board off keeps every task as it was; the loca's extension
/// view says how many are stored; turning it on serves them again.
#[tokio::test]
async fn stored_tasks_are_kept_while_the_board_is_off_and_come_back() {
    let loca = Loca::open().await;
    let ada = loca.seat("lead-ada").await;
    let bob = loca.seat("bob").await;
    board_on(&loca, "lead-ada").await;
    let (_, first) = loca.declare(Some(&ada), "first", Some("bob")).await;
    let (_, _second) = loca.declare(Some(&ada), "second", None).await;
    let (status, taken) = loca
        .patch(
            Some(&bob),
            id_of(&first),
            serde_json::json!({"status": "taken", "by": "x"}),
        )
        .await;
    assert_eq!(status, 200, "{taken}");
    let before: Vec<Value> =
        get_json(&loca.client, format!("{}/rooms/proj/tasks", loca.base)).await;
    let (_, view) = root(&loca, reqwest::Method::GET, "/extensions", None).await;
    assert_eq!(view["stored_tasks"], 2, "{view}");

    let (status, view) = extension(&loca, reqwest::Method::DELETE, "board?open_tasks=keep").await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(
        view["stored_tasks"], 2,
        "the view counts the kept tasks: {view}"
    );
    let (status, refused) = root(&loca, reqwest::Method::GET, "/tasks", None).await;
    assert_eq!(status, 403, "{refused}");

    let (status, _) = extension(&loca, reqwest::Method::PUT, "board").await;
    assert_eq!(status, 200);
    let after: Vec<Value> = get_json(&loca.client, format!("{}/rooms/proj/tasks", loca.base)).await;
    assert_eq!(after, before, "the same tasks, unchanged");
}

/// The catalog: the board has its own tab, after Focus; Focus is the goal.
#[tokio::test]
async fn the_catalog_gives_the_board_a_tab_after_focus() {
    let loca = Loca::open().await;
    let catalog: Value = loca
        .client
        .get(format!("{}/extensions", loca.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let list = catalog["catalog"].as_array().unwrap();
    let ids: Vec<&str> = list
        .iter()
        .map(|spec| spec["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        ["notes", "focus", "board", "journal", "wiki", "live", "vault"]
    );
    let board = &list[2];
    assert_eq!(board["name"], "Loca Board");
    assert_eq!(board["tab"], "Loca Board");
    assert_eq!(board["tab_key"], "board");
    let focus = &list[1];
    assert!(
        !focus["summary"].as_str().unwrap().contains("task"),
        "{focus}"
    );
}

/// While the board is off, a goal that would close by its tasks cannot be
/// set up, since its tasks cannot move; a manual goal can.
#[tokio::test]
async fn a_goal_closed_by_its_tasks_needs_the_board() {
    let loca = Loca::open().await;
    let ada = loca.seat("lead-ada").await;
    board_on(&loca, "lead-ada").await;
    let (_, task) = loca.declare(Some(&ada), "linked", None).await;
    let (status, _) = extension(&loca, reqwest::Method::DELETE, "board?open_tasks=keep").await;
    assert_eq!(status, 200);

    let (status, refused) = root(
        &loca,
        reqwest::Method::POST,
        "/goals",
        Some(serde_json::json!({
            "outcome": "ship", "completion": "all_tasks", "task_ids": [id_of(&task)], "by": "x"
        })),
    )
    .await;
    assert_eq!(status, 409, "{refused}");
    assert!(
        refused.as_str().unwrap_or_default().contains("Loca Board"),
        "{refused}"
    );
    let (status, goal) = root(
        &loca,
        reqwest::Method::POST,
        "/goals",
        Some(serde_json::json!({"outcome": "ship", "by": "x"})),
    )
    .await;
    assert_eq!(status, 201, "{goal}");
}
