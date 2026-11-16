//! The loca's Lead declares and assigns tasks, within the Operator's bounds:
//! it never moves an assignment the Operator or a rank made, it cancels only
//! an untaken task it declared itself, and it is recognised by the principal
//! stored with the title, never by the display name.

use super::*;

pub(super) const ROOT: &str = "lead-tasks-root";

/// One seated agent: its invite and the session that invite opened.
pub(super) struct Seat {
    invite: String,
    session: String,
}

impl Seat {
    pub(super) fn on(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request
            .header("x-room-token", &self.invite)
            .header("x-session-token", &self.session)
    }
}

pub(super) struct Loca {
    pub(super) base: String,
    pub(super) client: reqwest::Client,
    _dir: tempfile::TempDir,
    _guard: ServerGuard,
}

impl Loca {
    /// A closed Building with a database (principals exist) and the loca
    /// `proj`.
    pub(super) async fn open() -> Loca {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("lead-tasks.db");
        let (port, guard) =
            spawn_server_env(ROOT, &[("DB_PATH", db.to_str().unwrap().into())]).await;
        create_test_room(port, ROOT, "proj").await;
        Loca {
            base: format!("http://127.0.0.1:{port}"),
            client: reqwest::Client::new(),
            _dir: dir,
            _guard: guard,
        }
    }

    /// Admit `name` (a fresh membership every time) and seat it in `proj`.
    pub(super) async fn seat(&self, name: &str) -> Seat {
        let admitted = self
            .client
            .post(format!("{}/members", self.base))
            .header("x-admin-token", ROOT)
            .json(&serde_json::json!({ "name": name, "kind": "agent" }))
            .send()
            .await
            .unwrap();
        assert!(admitted.status().is_success(), "admit {name}");
        let invite: Value = self
            .client
            .post(format!("{}/rooms/proj/invites", self.base))
            .header("x-admin-token", ROOT)
            .json(&serde_json::json!({ "name": name }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let invite = invite["token"].as_str().unwrap().to_string();
        let session = session_with(&self.base, ("x-room-token", &invite), name, Some("proj")).await;
        Seat { invite, session }
    }

    pub(super) async fn name_lead(&self, name: &str) -> Value {
        let response = self
            .client
            .post(format!("{}/rooms/proj/lead", self.base))
            .header("x-admin-token", ROOT)
            .json(&serde_json::json!({ "lead": name }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "name the Lead");
        response.json().await.unwrap()
    }

    /// Declare a task as `who` (a seat), or as the Operator's rank (None).
    pub(super) async fn declare(
        &self,
        who: Option<&Seat>,
        title: &str,
        assignee: Option<&str>,
    ) -> (u16, Value) {
        let request = self
            .client
            .post(format!("{}/rooms/proj/tasks", self.base))
            .json(&serde_json::json!({ "title": title, "by": "x", "assigned_to": assignee }));
        let request = match who {
            Some(seat) => seat.on(request),
            None => request.header("x-admin-token", ROOT),
        };
        answer(request).await
    }

    pub(super) async fn patch(&self, who: Option<&Seat>, id: u64, body: Value) -> (u16, Value) {
        let request = self
            .client
            .patch(format!("{}/rooms/proj/tasks/{id}", self.base))
            .json(&body);
        let request = match who {
            Some(seat) => seat.on(request),
            None => request.header("x-admin-token", ROOT),
        };
        answer(request).await
    }

    pub(super) async fn task(&self, id: u64) -> Value {
        get_json(&self.client, format!("{}/rooms/proj/tasks", self.base))
            .await
            .into_iter()
            .find(|task| task["id"] == id)
            .expect("the task")
    }

    pub(super) async fn tasks_count(&self) -> usize {
        get_json(&self.client, format!("{}/rooms/proj/tasks", self.base))
            .await
            .len()
    }

    pub(super) async fn principal_of(&self, seat: &Seat) -> String {
        let who: Value = seat
            .on(self.client.get(format!("{}/whoami", self.base)))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        who["principal_id"]
            .as_str()
            .expect("a principal")
            .to_string()
    }

    /// The server's announcements in `proj`, as (target, text).
    pub(super) async fn announcements(&self) -> Vec<(String, String)> {
        let messages: Vec<Value> =
            get_json(&self.client, format!("{}/rooms/proj/messages", self.base)).await;
        messages
            .iter()
            .filter(|message| message["kind"] == "announce")
            .map(|message| {
                (
                    message["target"].as_str().unwrap_or("").to_string(),
                    message["text"].as_str().unwrap_or("").to_string(),
                )
            })
            .collect()
    }
}

pub(super) async fn answer(request: reqwest::RequestBuilder) -> (u16, Value) {
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    let text = response.text().await.unwrap();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

pub(super) fn id_of(task: &Value) -> u64 {
    task["id"].as_u64().expect("a task id")
}

/// The Lead declares a task with an assignee, declares one without, assigns
/// it and moves its own assignment; every decision records who made it with
/// what authority, and the line that wakes the assignee names the Lead.
#[tokio::test]
async fn the_lead_declares_and_assigns_tasks_and_the_line_names_the_lead() {
    let loca = Loca::open().await;
    let ada = loca.seat("lead-ada").await;
    let _bob = loca.seat("bob").await;
    let _carol = loca.seat("carol").await;
    let settings = loca.name_lead("lead-ada").await;
    let principal = loca.principal_of(&ada).await;
    assert_eq!(settings["lead"], "lead-ada");
    assert_eq!(
        settings["lead_principal"], principal,
        "naming the Lead stores its principal"
    );

    let (status, task) = loca.declare(Some(&ada), "fix the login", Some("bob")).await;
    assert_eq!(status, 201, "{task}");
    assert_eq!(task["created_by"], "lead-ada");
    assert_eq!(task["created_as"], "lead");
    assert_eq!(task["created_principal"], principal);
    assert_eq!(task["assigned_to"], "bob");
    assert_eq!(task["assigned_by"], "lead-ada");
    assert_eq!(task["assigned_as"], "lead");
    let first = id_of(&task);

    let (status, open) = loca.declare(Some(&ada), "write the note", None).await;
    assert_eq!(status, 201, "{open}");
    let second = id_of(&open);
    let (status, given) = loca
        .patch(
            Some(&ada),
            second,
            serde_json::json!({"assigned_to": "carol", "by": "x"}),
        )
        .await;
    assert_eq!(status, 200, "{given}");
    assert_eq!(given["assigned_to"], "carol");
    assert_eq!(given["assigned_as"], "lead");
    // The Lead may change its own decision.
    let (status, moved) = loca
        .patch(
            Some(&ada),
            second,
            serde_json::json!({"assigned_to": "bob", "by": "x"}),
        )
        .await;
    assert_eq!(status, 200, "{moved}");
    assert_eq!(moved["assigned_to"], "bob");

    let lines = loca.announcements().await;
    for (target, text) in [
        (
            "bob",
            format!("Lead lead-ada assigned task #{first} to @bob."),
        ),
        (
            "carol",
            format!("Lead lead-ada assigned task #{second} to @carol."),
        ),
        (
            "bob",
            format!("Lead lead-ada assigned task #{second} to @bob."),
        ),
    ] {
        assert!(
            lines.contains(&(target.to_string(), text.clone())),
            "missing {text:?} for {target}: {lines:?}"
        );
    }
}

/// The Lead never moves an assignment the Operator (or a rank) made, and the
/// Operator reverses a Lead's assignment, which then becomes the Operator's.
#[tokio::test]
async fn the_lead_cannot_move_an_operator_assignment_and_the_operator_reverses_the_lead() {
    let loca = Loca::open().await;
    let ada = loca.seat("lead-ada").await;
    let _bob = loca.seat("bob").await;
    let _carol = loca.seat("carol").await;
    loca.name_lead("lead-ada").await;

    let (_, by_operator) = loca.declare(None, "operator's", Some("bob")).await;
    assert_eq!(by_operator["assigned_as"], "operator");
    let id = id_of(&by_operator);
    let (status, _) = loca
        .patch(
            Some(&ada),
            id,
            serde_json::json!({"assigned_to": "carol", "by": "x"}),
        )
        .await;
    assert_eq!(status, 403, "the Lead moved an Operator's assignment");
    assert_eq!(loca.task(id).await["assigned_to"], "bob");

    let (_, by_lead) = loca.declare(Some(&ada), "lead's", Some("carol")).await;
    let id = id_of(&by_lead);
    let (status, reversed) = loca
        .patch(
            None,
            id,
            serde_json::json!({"assigned_to": "bob", "by": "x"}),
        )
        .await;
    assert_eq!(status, 200, "{reversed}");
    assert_eq!(reversed["assigned_to"], "bob");
    assert_eq!(reversed["assigned_as"], "operator");
    let (status, _) = loca
        .patch(
            Some(&ada),
            id,
            serde_json::json!({"assigned_to": "carol", "by": "x"}),
        )
        .await;
    assert_eq!(status, 403, "the Lead undid the Operator's reversal");
    assert_eq!(loca.task(id).await["assigned_to"], "bob");
    // The Operator removes a task the Lead declared.
    let (status, removed) = loca
        .patch(
            None,
            id,
            serde_json::json!({"status": "cancelled", "by": "x"}),
        )
        .await;
    assert_eq!(status, 200, "{removed}");
    assert_eq!(removed["status"], "cancelled");
}

/// The Lead cancels a task it declared that nobody has taken, and nothing
/// else: not a taken one, not the Operator's, and it never reopens or
/// finishes another's.
#[tokio::test]
async fn the_lead_cancels_only_its_own_untaken_declaration() {
    let loca = Loca::open().await;
    let ada = loca.seat("lead-ada").await;
    let bob = loca.seat("bob").await;
    loca.name_lead("lead-ada").await;

    let (_, untaken) = loca.declare(Some(&ada), "untaken", Some("bob")).await;
    let untaken = id_of(&untaken);
    let (status, cancelled) = loca
        .patch(
            Some(&ada),
            untaken,
            serde_json::json!({"status": "cancelled", "by": "x"}),
        )
        .await;
    assert_eq!(status, 200, "{cancelled}");
    assert_eq!(cancelled["status"], "cancelled");
    let (status, _) = loca
        .patch(
            Some(&ada),
            untaken,
            serde_json::json!({"status": "open", "by": "x"}),
        )
        .await;
    assert_eq!(status, 403, "reopening stays the Operator's");

    let (_, taken) = loca.declare(Some(&ada), "taken", Some("bob")).await;
    let taken = id_of(&taken);
    let (status, _) = loca
        .patch(
            Some(&bob),
            taken,
            serde_json::json!({"status": "taken", "by": "x"}),
        )
        .await;
    assert_eq!(status, 200);
    for status_asked in ["cancelled", "done"] {
        let (status, _) = loca
            .patch(
                Some(&ada),
                taken,
                serde_json::json!({"status": status_asked, "by": "x"}),
            )
            .await;
        assert_eq!(status, 403, "the Lead set {status_asked} on a taken task");
    }
    assert_eq!(loca.task(taken).await["status"], "taken");

    let (_, operators) = loca.declare(None, "operator's", None).await;
    let operators = id_of(&operators);
    let (status, _) = loca
        .patch(
            Some(&ada),
            operators,
            serde_json::json!({"status": "cancelled", "by": "x"}),
        )
        .await;
    assert_eq!(status, 403, "the Lead cancelled an Operator's task");
    assert_eq!(loca.task(operators).await["status"], "open");
}

/// A participant who is not the Lead declares nothing and assigns nothing,
/// exactly as before; a request that only claims the Lead's name in its body
/// is not the Lead either.
#[tokio::test]
async fn a_participant_who_is_not_the_lead_is_refused() {
    let loca = Loca::open().await;
    let ada = loca.seat("lead-ada").await;
    let bob = loca.seat("bob").await;
    let _carol = loca.seat("carol").await;
    loca.name_lead("lead-ada").await;

    let (status, _) = loca.declare(Some(&bob), "bob's", Some("carol")).await;
    assert_eq!(status, 403);
    let claimed = loca
        .client
        .post(format!("{}/rooms/proj/tasks", loca.base))
        .json(&serde_json::json!({ "title": "claimed", "by": "lead-ada" }))
        .send()
        .await
        .unwrap();
    assert_eq!(claimed.status(), 403, "a claimed Lead name declared a task");

    let (_, open) = loca.declare(Some(&ada), "open", None).await;
    let id = id_of(&open);
    let (status, _) = loca
        .patch(
            Some(&bob),
            id,
            serde_json::json!({"assigned_to": "carol", "by": "x"}),
        )
        .await;
    assert_eq!(status, 403, "a participant gave a task to someone else");
    let (status, _) = loca
        .patch(
            Some(&bob),
            id,
            serde_json::json!({"status": "cancelled", "by": "x"}),
        )
        .await;
    assert_eq!(status, 403, "a participant cancelled a task");
    let task = loca.task(id).await;
    assert_eq!(task["status"], "open");
    assert!(task.get("assigned_to").is_none(), "{task}");
    assert_eq!(loca.tasks_count().await, 1, "nothing was declared");
}

/// The Lead's authority follows its principal: it moves with a transfer at
/// once, and someone who now wears the old Lead's name is not the Lead.
#[tokio::test]
async fn the_lead_is_its_principal_not_its_name() {
    let loca = Loca::open().await;
    let ada = loca.seat("lead-ada").await;
    let carol = loca.seat("carol").await;
    loca.name_lead("lead-ada").await;
    let (status, _) = loca.declare(Some(&ada), "before", None).await;
    assert_eq!(status, 201);

    // A transfer moves the authority at once.
    loca.name_lead("carol").await;
    let (status, _) = loca.declare(Some(&ada), "after the transfer", None).await;
    assert_eq!(status, 403, "the former Lead still declares");
    let (status, _) = loca.declare(Some(&carol), "the new Lead's", None).await;
    assert_eq!(status, 201);

    // Back to ada; then ada's membership ends and someone new is admitted
    // under the same name. The title still reads "lead-ada"; the newcomer
    // is not the Lead.
    loca.name_lead("lead-ada").await;
    let listed: Value = loca
        .client
        .get(format!("{}/members", loca.base))
        .header("x-admin-token", ROOT)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|member| member["name"] == "lead-ada")
        .expect("ada's membership")["id"]
        .as_str()
        .unwrap()
        .to_string();
    let removed = loca
        .client
        .delete(format!("{}/members/{id}", loca.base))
        .header("x-admin-token", ROOT)
        .send()
        .await
        .unwrap();
    assert!(removed.status().is_success(), "remove ada");
    let newcomer = loca.seat("lead-ada").await;
    assert_ne!(
        loca.principal_of(&newcomer).await,
        loca.task(1).await["created_principal"].as_str().unwrap(),
        "the newcomer is another principal"
    );
    let settings: Value = loca
        .client
        .get(format!("{}/rooms/proj/settings", loca.base))
        .header("x-admin-token", ROOT)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(settings["lead"], "lead-ada", "the title is kept by name");
    let (status, _) = loca
        .declare(Some(&newcomer), "by the name alone", None)
        .await;
    assert_eq!(status, 403, "wearing the old Lead's name made a Lead");
    let (status, _) = loca
        .patch(
            Some(&newcomer),
            1,
            serde_json::json!({"assigned_to": "carol", "by": "x"}),
        )
        .await;
    assert_eq!(status, 403);
}
