//! The Loca Board (`board` extension) over the real routes: the review step
//! with go / no-go, who may do what, and the lines that wake. Its
//! preconditions and what it keeps while off are in `loca_board.rs`.

use super::lead_tasks::{answer, id_of, Loca, Seat, ROOT};
use super::*;

impl Loca {
    async fn extension(&self, method: reqwest::Method, id: &str) -> (u16, Value) {
        answer(
            self.client
                .request(method, format!("{}/rooms/proj/extensions/{id}", self.base))
                .header("x-admin-token", ROOT),
        )
        .await
    }

    async fn board_on(&self) {
        let (status, view) = self.extension(reqwest::Method::PUT, "board").await;
        assert_eq!(status, 200, "{view}");
    }

    /// A board-ready loca: the Lead `lead-ada`, the holder `bob`, a bystander
    /// `carol`, and the board on.
    async fn with_board() -> (Loca, Seat, Seat, Seat) {
        let loca = Loca::open().await;
        let ada = loca.seat("lead-ada").await;
        let bob = loca.seat("bob").await;
        let carol = loca.seat("carol").await;
        loca.name_lead("lead-ada").await;
        loca.board_on().await;
        (loca, ada, bob, carol)
    }

    async fn status(&self, who: Option<&Seat>, id: u64, body: Value) -> (u16, Value) {
        let mut body = body;
        body["by"] = "x".into();
        self.patch(who, id, body).await
    }
}

/// The whole review step: the holder's done asks the Lead for review with a
/// note, a no-go needs a reason and sends the task back, the next request
/// clears the reason, the go finishes it. Each move is one line addressed to
/// the one who must act (a go to nobody). An `all_tasks` Goal is closed by
/// the go, not by the request for review.
#[tokio::test]
async fn finishing_on_the_board_is_a_request_for_review_and_the_lead_says_go_or_no_go() {
    let (loca, ada, bob, carol) = Loca::with_board().await;
    let (_, task) = loca.declare(Some(&ada), "fix the login", Some("bob")).await;
    let id = id_of(&task);
    let goal = loca
        .client
        .post(format!("{}/rooms/proj/goals", loca.base))
        .header("x-admin-token", ROOT)
        .json(&serde_json::json!({
            "outcome": "login works", "completion": "all_tasks", "task_ids": [id], "by": "x"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(goal.status(), 201);

    let (status, _) = loca
        .status(Some(&bob), id, serde_json::json!({"status": "taken"}))
        .await;
    assert_eq!(status, 200);
    let (status, asked) = loca
        .status(
            Some(&bob),
            id,
            serde_json::json!({"status": "done", "note": "the session now renews; see #42"}),
        )
        .await;
    assert_eq!(status, 200, "{asked}");
    assert_eq!(asked["status"], "review");
    assert_eq!(asked["review_note"], "the session now renews; see #42");
    let goals: Vec<Value> = get_json(&loca.client, format!("{}/rooms/proj/goals", loca.base)).await;
    assert_eq!(
        goals[0]["status"], "active",
        "a task in review closed the Goal"
    );

    // Nobody but the Lead or the Operator reviews.
    for who in [&bob, &carol] {
        let (status, _) = loca
            .status(Some(who), id, serde_json::json!({"status": "done"}))
            .await;
        assert_eq!(status, 403);
    }
    let (status, _) = loca
        .status(Some(&ada), id, serde_json::json!({"status": "taken"}))
        .await;
    assert_eq!(status, 400, "a no-go without a reason");
    assert_eq!(loca.task(id).await["status"], "review");
    let (status, back) = loca
        .status(
            Some(&ada),
            id,
            serde_json::json!({"status": "taken", "reason": "the expired-session path is untested"}),
        )
        .await;
    assert_eq!(status, 200, "{back}");
    assert_eq!(back["status"], "taken");
    assert_eq!(
        back["review_reason"],
        "the expired-session path is untested"
    );

    let (_, again) = loca
        .status(Some(&bob), id, serde_json::json!({"status": "done"}))
        .await;
    assert_eq!(again["status"], "review");
    assert!(again.get("review_reason").is_none(), "{again}");
    assert!(again.get("review_note").is_none(), "{again}");
    let (status, done) = loca
        .status(Some(&ada), id, serde_json::json!({"status": "done"}))
        .await;
    assert_eq!(status, 200, "{done}");
    assert_eq!(done["status"], "done");
    let goals: Vec<Value> = get_json(&loca.client, format!("{}/rooms/proj/goals", loca.base)).await;
    assert_eq!(goals[0]["status"], "achieved", "the go closes the Goal");

    let lines = loca.announcements().await;
    for (target, text) in [
        ("lead-ada", format!("bob asks @lead-ada to review task #{id}.")),
        (
            "bob",
            format!("Lead lead-ada: task #{id} is not done yet, back to @bob (the reason is on the task)."),
        ),
        ("all", format!("Lead lead-ada: task #{id} is done.")),
    ] {
        assert!(
            lines.contains(&(target.to_string(), text.clone())),
            "missing {text:?} for {target}: {lines:?}"
        );
    }
    // The reason is not in the line: an `@name` in it wakes nobody.
    assert!(!lines.iter().any(|(_, text)| text.contains("untested")));
}

/// The Lead does not review its own task; the Operator does. The Operator
/// may finish without a review and reverses a Lead's go.
#[tokio::test]
async fn the_lead_does_not_review_its_own_task_and_the_operator_reverses_it() {
    let (loca, ada, bob, _carol) = Loca::with_board().await;
    let (_, own) = loca
        .declare(Some(&ada), "the Lead's own", Some("lead-ada"))
        .await;
    let own = id_of(&own);
    loca.status(Some(&ada), own, serde_json::json!({"status": "taken"}))
        .await;
    let (_, asked) = loca
        .status(Some(&ada), own, serde_json::json!({"status": "done"}))
        .await;
    assert_eq!(
        asked["status"], "review",
        "the Lead's own done is a request too"
    );
    for body in [
        serde_json::json!({"status": "done"}),
        serde_json::json!({"status": "taken", "reason": "self"}),
    ] {
        let (status, _) = loca.status(Some(&ada), own, body).await;
        assert_eq!(status, 403, "the Lead reviewed its own task");
    }
    let (status, _) = loca
        .status(None, own, serde_json::json!({"status": "done"}))
        .await;
    assert_eq!(status, 200, "the Operator reviews the Lead's task");

    // The Operator finishes a taken task without a review, and takes back a
    // task the Lead passed; neither the holder nor the Lead can.
    let (_, task) = loca.declare(Some(&ada), "another", Some("bob")).await;
    let id = id_of(&task);
    loca.status(Some(&bob), id, serde_json::json!({"status": "taken"}))
        .await;
    loca.status(Some(&bob), id, serde_json::json!({"status": "done"}))
        .await;
    loca.status(Some(&ada), id, serde_json::json!({"status": "done"}))
        .await;
    for who in [&bob, &ada] {
        for status_asked in ["taken", "open"] {
            let (status, _) = loca
                .status(Some(who), id, serde_json::json!({"status": status_asked}))
                .await;
            assert_eq!(status, 403, "{status_asked} on a passed task");
        }
    }
    let (status, reopened) = loca
        .status(None, id, serde_json::json!({"status": "open"}))
        .await;
    assert_eq!(status, 200, "{reopened}");
    let (_, operators) = loca.declare(None, "operator's", Some("bob")).await;
    let operators = id_of(&operators);
    loca.status(
        Some(&bob),
        operators,
        serde_json::json!({"status": "taken"}),
    )
    .await;
    let (status, finished) = loca
        .status(None, operators, serde_json::json!({"status": "done"}))
        .await;
    assert_eq!(status, 200, "{finished}");
    assert_eq!(
        finished["status"], "done",
        "the Operator's veto needs no review"
    );
}

/// The lines reach the people they are addressed to on a mentions filter,
/// once: the request for review wakes the Lead, a no-go wakes the holder, a
/// go wakes nobody. (A memory-only server: names stand for principals there.)
#[tokio::test]
async fn board_lines_wake_who_must_act_and_a_go_wakes_nobody() {
    let (port, _guard) = spawn_server_with("bigmaster").await;
    create_test_room(port, "bigmaster", "proj").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let admin = |request: reqwest::RequestBuilder| request.header("x-admin-token", "bigmaster");
    assert_eq!(
        admin(client.post(format!("{base}/rooms/proj/lead")))
            .json(&serde_json::json!({"lead": "lead-ada"}))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        admin(client.put(format!("{base}/rooms/proj/extensions/board")))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let mut bob = connect_agent_mentions(port, "proj", "bob").await;
    let mut carol = connect_agent_mentions(port, "proj", "carol").await;
    wait_for(&mut bob, |frame| frame["t"] == "memory").await;
    wait_for(&mut carol, |frame| frame["t"] == "memory").await;
    let task: Value = client
        .post(format!("{base}/rooms/proj/tasks"))
        .json(&serde_json::json!({"title": "t", "by": "lead-ada", "assigned_to": "bob"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = id_of(&task);
    let wake = wait_for(&mut bob, |frame| frame["t"] == "msg").await;
    assert_eq!(wake["message"]["target"], "bob");
    let patch = |body: Value| {
        client
            .patch(format!("{base}/rooms/proj/tasks/{id}"))
            .json(&body)
            .send()
    };
    for body in [
        serde_json::json!({"status": "taken", "by": "bob"}),
        serde_json::json!({"status": "done", "by": "bob"}),
    ] {
        assert_eq!(patch(body).await.unwrap().status(), 200);
    }
    assert_eq!(
        patch(serde_json::json!({"status": "taken", "reason": "@carol look", "by": "lead-ada"}))
            .await
            .unwrap()
            .status(),
        200
    );
    let back = wait_for(&mut bob, |frame| frame["t"] == "msg").await;
    assert_eq!(back["message"]["target"], "bob");
    assert!(back["message"]["text"]
        .as_str()
        .unwrap()
        .contains("not done yet"));
    for body in [
        serde_json::json!({"status": "done", "by": "bob"}),
        serde_json::json!({"status": "done", "by": "lead-ada"}),
    ] {
        assert_eq!(patch(body).await.unwrap().status(), 200);
    }
    for (name, socket) in [("bob", &mut bob), ("carol", &mut carol)] {
        while let Ok(Some(Ok(WsMessage::Text(text)))) =
            tokio::time::timeout(Duration::from_millis(300), socket.next()).await
        {
            let frame: Value = serde_json::from_str(&text).unwrap();
            assert!(
                frame["t"] != "msg" && frame["t"] != "turn",
                "{name} was woken: {frame}"
            );
        }
    }
}

/// The review belongs to whoever gave the task: a task the Operator gave
/// asks the Operator for review, and the Lead, who did not give it, cannot
/// say go or no-go on it. The Operator can.
#[tokio::test]
async fn whoever_gave_the_task_reviews_it_not_any_lead() {
    let (loca, ada, bob, _carol) = Loca::with_board().await;
    let (status, task) = loca.declare(None, "rotate the key", Some("bob")).await;
    assert_eq!(status, 201, "{task}");
    let id = id_of(&task);
    let given_by = task["assigned_by"]
        .as_str()
        .expect("assigned_by")
        .to_string();
    let (_, asked) = loca
        .status(Some(&bob), id, serde_json::json!({"status": "done"}))
        .await;
    assert_eq!(asked["status"], "review");
    for reason in [None, Some("not like this")] {
        let mut body = serde_json::json!({"status": "taken"});
        if let Some(reason) = reason {
            body["reason"] = reason.into();
        }
        let (status, _) = loca.status(Some(&ada), id, body).await;
        assert_eq!(status, 403, "the Lead reviewed a task it did not give");
    }
    let (status, _) = loca
        .status(Some(&ada), id, serde_json::json!({"status": "done"}))
        .await;
    assert_eq!(status, 403, "the Lead approved a task it did not give");
    let (status, done) = loca
        .status(None, id, serde_json::json!({"status": "done"}))
        .await;
    assert_eq!(status, 200, "{done}");
    assert_eq!(done["status"], "done");
    let lines = loca.announcements().await;
    let asks = format!("bob asks @{given_by} to review task #{id}.");
    assert!(
        lines.contains(&(given_by.clone(), asks.clone())),
        "missing {asks:?}: {lines:?}"
    );
}
