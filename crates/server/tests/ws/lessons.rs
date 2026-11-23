//! Lessons for the Vault: a reviewer's no-go reason and a reopened task
//! propose a lesson by themselves, a seated person or agent may propose one
//! under its verified name, and only those who run the loca accept one. An
//! accepted lesson is a line under the Vault's `## Lessons` heading, a new
//! Vault revision; a dismissed one changes nothing. All of it is refused
//! while the Vault is off.

use super::lead_tasks::{answer, id_of, Loca, Seat, ROOT};
use super::loca_board::board_on;
use super::*;

async fn as_root(
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

async fn as_seat(
    loca: &Loca,
    seat: &Seat,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let mut request = seat.on(loca
        .client
        .request(method, format!("{}/rooms/proj{path}", loca.base)));
    if let Some(body) = body {
        request = request.json(&body);
    }
    answer(request).await
}

async fn lessons(loca: &Loca) -> Vec<Value> {
    let (status, view) = as_root(loca, reqwest::Method::GET, "/vault/lessons", None).await;
    assert_eq!(status, 200, "{view}");
    view["lessons"].as_array().unwrap().clone()
}

async fn vault_on(loca: &Loca) {
    let (status, view) = as_root(loca, reqwest::Method::PUT, "/extensions/vault", None).await;
    assert_eq!(status, 200, "turn the Vault on: {view}");
}

#[tokio::test]
async fn a_no_go_and_a_reopen_propose_and_only_an_accepted_lesson_reaches_the_vault() {
    use reqwest::Method;
    let loca = Loca::open().await;
    let ada = loca.seat("lead-ada").await;
    let bob = loca.seat("bob").await;
    board_on(&loca, "lead-ada").await;
    vault_on(&loca).await;
    let (_, task) = loca.declare(Some(&ada), "fix the login", Some("bob")).await;
    let id = id_of(&task);
    for (who, body) in [
        (&bob, serde_json::json!({"status": "taken", "by": "x"})),
        (
            &bob,
            serde_json::json!({"status": "done", "note": "done", "by": "x"}),
        ),
        (
            &ada,
            serde_json::json!({"status": "taken", "reason": "run the tests\nbefore asking for review", "by": "x"}),
        ),
    ] {
        let (status, v) = loca.patch(Some(who), id, body).await;
        assert_eq!(status, 200, "{v}");
    }

    // The no-go reason waits as a proposal, in one line, with its task.
    let waiting = lessons(&loca).await;
    assert_eq!(waiting.len(), 1, "{waiting:?}");
    let nogo = &waiting[0];
    assert_eq!(nogo["source"], "nogo");
    assert_eq!(nogo["task"], id);
    assert_eq!(nogo["status"], "proposed");
    assert_eq!(nogo["text"], "run the tests before asking for review");
    assert_eq!(nogo["proposed_by"], "lead-ada");
    let lid = nogo["id"].as_u64().unwrap();

    // Nothing is in the Vault until someone who runs the loca accepts it,
    // and the Lead and the holder do not.
    let (_, vault) = as_root(&loca, Method::GET, "/vault", None).await;
    assert_eq!(vault["rev"], 0);
    for seat in [&ada, &bob] {
        let (status, v) = as_seat(
            &loca,
            seat,
            Method::POST,
            &format!("/vault/lessons/{lid}/accept"),
            Some(serde_json::json!({})),
        )
        .await;
        assert_eq!(status, 403, "{v}");
    }
    let (status, accepted) = as_root(
        &loca,
        Method::POST,
        &format!("/vault/lessons/{lid}/accept"),
        Some(serde_json::json!({"text": "Run the tests before asking for review."})),
    )
    .await;
    assert_eq!(status, 200, "{accepted}");
    assert_eq!(accepted["status"], "accepted");
    assert_eq!(accepted["vault_rev"], 1);
    assert_eq!(accepted["text"], "Run the tests before asking for review.");
    let (_, vault) = as_root(&loca, Method::GET, "/vault", None).await;
    assert_eq!(vault["rev"], 1);
    assert_eq!(
        vault["body"],
        format!("## Lessons\n\n- Run the tests before asking for review. (from the no-go on task #{id})\n")
    );
    // Decided once.
    let (status, _) = as_root(
        &loca,
        Method::POST,
        &format!("/vault/lessons/{lid}/accept"),
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(status, 409);

    // A finished task taken back proposes a lesson too; dismissing it
    // leaves the Vault as it was.
    for (who, body) in [
        (
            &bob,
            serde_json::json!({"status": "done", "note": "tests run", "by": "x"}),
        ),
        (&ada, serde_json::json!({"status": "done", "by": "x"})),
    ] {
        let (status, v) = loca.patch(Some(who), id, body).await;
        assert_eq!(status, 200, "{v}");
    }
    let (status, v) = loca
        .patch(
            None,
            id,
            serde_json::json!({"status": "open", "by": "operator"}),
        )
        .await;
    assert_eq!(status, 200, "{v}");
    let waiting = lessons(&loca).await;
    let reopen = waiting
        .iter()
        .find(|l| l["source"] == "reopen")
        .unwrap_or_else(|| panic!("no reopen lesson in {waiting:?}"));
    assert_eq!(reopen["status"], "proposed");
    assert!(
        reopen["text"]
            .as_str()
            .unwrap()
            .contains("was reopened after it was done"),
        "{reopen}"
    );
    let (status, dismissed) = as_root(
        &loca,
        Method::POST,
        &format!("/vault/lessons/{}/dismiss", reopen["id"]),
        None,
    )
    .await;
    assert_eq!(status, 200, "{dismissed}");
    assert_eq!(dismissed["status"], "dismissed");
    let (_, vault) = as_root(&loca, Method::GET, "/vault", None).await;
    assert_eq!(vault["rev"], 1);

    // With the Vault off, lessons are fenced off and a no-go proposes none.
    let (status, _) = as_root(&loca, Method::DELETE, "/extensions/vault", None).await;
    assert_eq!(status, 200);
    let (status, v) = as_root(&loca, Method::GET, "/vault/lessons", None).await;
    assert_eq!(status, 403, "{v}");
}

#[tokio::test]
async fn a_seated_member_proposes_under_its_verified_name() {
    use reqwest::Method;
    let loca = Loca::open().await;
    let bob = loca.seat("bob").await;
    vault_on(&loca).await;
    let (status, lesson) = as_seat(
        &loca,
        &bob,
        Method::POST,
        "/vault/lessons",
        Some(serde_json::json!({"text": "read the Vault before a deploy"})),
    )
    .await;
    assert_eq!(status, 201, "{lesson}");
    assert_eq!(lesson["source"], "agent");
    assert_eq!(lesson["proposed_by"], "bob");
    // The same words wait once.
    let (status, again) = as_seat(
        &loca,
        &bob,
        Method::POST,
        "/vault/lessons",
        Some(serde_json::json!({"text": "read the Vault  before a deploy"})),
    )
    .await;
    assert_eq!(status, 201, "{again}");
    assert_eq!(again["id"], lesson["id"]);
    assert_eq!(lessons(&loca).await.len(), 1);
    // Empty words are refused.
    let (status, _) = as_seat(
        &loca,
        &bob,
        Method::POST,
        "/vault/lessons",
        Some(serde_json::json!({"text": "  "})),
    )
    .await;
    assert_eq!(status, 400);
    // A member may read the lessons but not decide them.
    let (status, view) = as_seat(&loca, &bob, Method::GET, "/vault/lessons", None).await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["can_decide"], false);
    // Accepted in its own words when no text is given.
    let (status, accepted) = as_root(
        &loca,
        Method::POST,
        &format!("/vault/lessons/{}/accept", lesson["id"]),
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(status, 200, "{accepted}");
    let (_, vault) = as_root(&loca, Method::GET, "/vault", None).await;
    assert_eq!(
        vault["body"],
        "## Lessons\n\n- read the Vault before a deploy (proposed by bob)\n"
    );
}
