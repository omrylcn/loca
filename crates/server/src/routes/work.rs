use crate::*;

/// The principal the server verified for this caller: its session's, else
/// the one its key resolves to. None where nothing is verified (the open
/// sandbox, a claimed name); task assignment then falls back to the name.
fn caller_principal(hub: &Hub, headers: &HeaderMap) -> Option<String> {
    let session = session_of(headers);
    hub.session_identity(session)
        .and_then(|identity| identity.principal_id)
        .or_else(|| {
            hub.resolve_authority(admin_token_of(headers), session)
                .principal_id
        })
}

/// The task authority this caller holds in `room`: an operator's (the grand
/// operator anywhere, a loca operator in their own loca), else the Lead's
/// (by principal), else none.
fn task_actor(
    hub: &Hub,
    room: &str,
    headers: &HeaderMap,
    by: &str,
    by_principal: Option<&str>,
) -> Option<protocol::TaskActor> {
    if is_admin_req(hub, headers)
        || hub.is_loca_operator(room, admin_token_of(headers), session_of(headers), by)
    {
        return Some(protocol::TaskActor::Operator);
    }
    hub.is_task_lead(room, by, by_principal)
        .then_some(protocol::TaskActor::Lead)
}

pub(crate) async fn list_tasks(State(hub): State<Hub>, access: RoomAccess) -> impl IntoResponse {
    Json(hub.tasks(&access.room))
}
/// `GET /rooms/:id/tasks/:tid`: one task, for its own view and address.
pub(crate) async fn get_task(
    State(hub): State<Hub>,
    access: RoomAccess,
    Path((_id, tid)): Path<(String, u64)>,
) -> impl IntoResponse {
    match hub.task(&access.room, tid) {
        Some(task) => Json(task).into_response(),
        None => (StatusCode::NOT_FOUND, "no such task").into_response(),
    }
}
pub(crate) async fn create_task(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    Json(mut body): Json<CreateTask>,
) -> impl IntoResponse {
    let id = access.room;
    if !hub.is_writable(&id) {
        return (StatusCode::CONFLICT, "this loca is closed — read-only").into_response();
    }
    if body.title.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "empty title").into_response();
    }
    let by = match actor_of(&hub, &headers, &body.by) {
        Ok(n) => n,
        Err(c) => return (c, "invalid or missing session").into_response(),
    };
    // Declaring work is an operator act — the grand operator anywhere (raw key
    // or admin session), or a loca operator in their own loca — or the
    // loca's Lead, recognised by its principal. Other agents propose in chat.
    let by_principal = caller_principal(&hub, &headers);
    let Some(actor) = task_actor(&hub, &id, &headers, &by, by_principal.as_deref()) else {
        return (
            StatusCode::FORBIDDEN,
            "declaring a task takes operator or Lead authority in this loca",
        )
            .into_response();
    };
    body.by = by;
    match hub.create_task_as(&id, body, by_principal.as_deref(), actor) {
        Ok(task) => (StatusCode::CREATED, Json(task)).into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save task — try again",
        )
            .into_response(),
    }
}
pub(crate) async fn update_task(
    State(hub): State<Hub>,
    access: RoomAccess,
    Path((_id, tid)): Path<(String, u64)>,
    headers: HeaderMap,
    Json(mut body): Json<UpdateTask>,
) -> impl IntoResponse {
    let id = access.room;
    if !hub.is_writable(&id) {
        return (StatusCode::CONFLICT, "this loca is closed — read-only").into_response();
    }
    let by = match actor_of(&hub, &headers, &body.by) {
        Ok(n) => n,
        Err(c) => return (c, "invalid or missing session").into_response(),
    };
    let by_principal = caller_principal(&hub, &headers);
    let actor = task_actor(&hub, &id, &headers, &by, by_principal.as_deref())
        .unwrap_or(protocol::TaskActor::Participant);
    body.by = by;
    match hub.update_task_as(&id, tid, body, actor, by_principal.as_deref()) {
        Ok(t) => (StatusCode::OK, Json(t)).into_response(),
        Err(TaskError::NotFound) => (StatusCode::NOT_FOUND, "no such task").into_response(),
        Err(TaskError::Forbidden) => (
            StatusCode::FORBIDDEN,
            "house rules: agents take/finish their own tasks; cancel/reopen/reassign is the operator's (the Lead's only within its bounds)",
        ).into_response(),
        // The review state is the Loca Board's one door: refused with the
        // refusal every switched-off extension gives.
        Err(TaskError::BoardOff) => super::extensions::extension_off("board"),
        Err(TaskError::ReasonRequired) => (
            StatusCode::BAD_REQUEST,
            "a no-go needs a reason",
        ).into_response(),
        Err(TaskError::TooLong) => (
            StatusCode::BAD_REQUEST,
            "a review note or reason is at most 2000 characters",
        ).into_response(),
        Err(TaskError::Storage) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save task — try again",
        ).into_response(),
    }
}
// ---- goal: one operator-defined outcome, optionally closed by linked tasks ----

pub(crate) async fn list_goals(State(hub): State<Hub>, access: RoomAccess) -> impl IntoResponse {
    Json(hub.goals(&access.room))
}
pub(crate) async fn create_goal(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    Json(mut body): Json<CreateGoal>,
) -> impl IntoResponse {
    let id = access.room;
    if !hub.is_writable(&id) {
        return (StatusCode::CONFLICT, "this loca is closed — read-only").into_response();
    }
    body.outcome = body.outcome.trim().to_string();
    if body.outcome.is_empty() {
        return (StatusCode::BAD_REQUEST, "empty goal outcome").into_response();
    }
    body.checkpoint = body
        .checkpoint
        .map(|checkpoint| checkpoint.trim().to_string())
        .filter(|checkpoint| !checkpoint.is_empty());
    if body
        .stale_after_secs
        .is_some_and(|seconds| seconds > 2_592_000)
    {
        return (
            StatusCode::BAD_REQUEST,
            "stale_after_secs must be at most 30 days",
        )
            .into_response();
    }
    let by = match actor_of(&hub, &headers, &body.by) {
        Ok(name) => name,
        Err(code) => return (code, "invalid or missing session").into_response(),
    };
    if !is_admin_req(&hub, &headers)
        && !hub.is_loca_operator(&id, admin_token_of(&headers), session_of(&headers), &by)
    {
        return (
            StatusCode::FORBIDDEN,
            "defining a goal takes operator authority in this loca",
        )
            .into_response();
    }
    body.by = by;
    match hub.create_goal(&id, body) {
        Ok(goal) => (StatusCode::CREATED, Json(goal)).into_response(),
        Err(GoalError::ActiveExists) => (
            StatusCode::CONFLICT,
            "this loca already has an active goal — close it first",
        )
            .into_response(),
        Err(GoalError::LeadRequired) => (
            StatusCode::CONFLICT,
            "Goal cannot be activated — select a Lead first",
        )
            .into_response(),
        Err(GoalError::InvalidTasks) => (
            StatusCode::BAD_REQUEST,
            "all_tasks requires existing task ids from this loca",
        )
            .into_response(),
        Err(GoalError::BoardOff) => goal_needs_board(),
        Err(GoalError::Storage) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save goal — try again",
        )
            .into_response(),
        Err(GoalError::NotFound) => unreachable!(),
    }
}
pub(crate) async fn update_goal(
    State(hub): State<Hub>,
    access: RoomAccess,
    Path((_id, gid)): Path<(String, u64)>,
    headers: HeaderMap,
    Json(mut body): Json<UpdateGoal>,
) -> impl IntoResponse {
    let id = access.room;
    if !hub.is_writable(&id) {
        return (StatusCode::CONFLICT, "this loca is closed — read-only").into_response();
    }
    if let Some(outcome) = body.outcome.as_mut() {
        *outcome = outcome.trim().to_string();
        if outcome.is_empty() {
            return (StatusCode::BAD_REQUEST, "empty goal outcome").into_response();
        }
    }
    if let Some(checkpoint) = body.checkpoint.as_mut() {
        *checkpoint = checkpoint.trim().to_string();
    }
    if body
        .stale_after_secs
        .flatten()
        .is_some_and(|seconds| seconds > 2_592_000)
    {
        return (
            StatusCode::BAD_REQUEST,
            "stale_after_secs must be at most 30 days",
        )
            .into_response();
    }
    let by = match actor_of(&hub, &headers, &body.by) {
        Ok(name) => name,
        Err(code) => return (code, "invalid or missing session").into_response(),
    };
    if !is_admin_req(&hub, &headers)
        && !hub.is_loca_operator(&id, admin_token_of(&headers), session_of(&headers), &by)
    {
        return (
            StatusCode::FORBIDDEN,
            "changing a goal takes operator authority in this loca",
        )
            .into_response();
    }
    body.by = by;
    match hub.update_goal(&id, gid, body) {
        Ok(goal) => (StatusCode::OK, Json(goal)).into_response(),
        Err(GoalError::NotFound) => (StatusCode::NOT_FOUND, "no such goal").into_response(),
        Err(GoalError::ActiveExists) => {
            (StatusCode::CONFLICT, "another goal is already active").into_response()
        }
        Err(GoalError::LeadRequired) => (
            StatusCode::CONFLICT,
            "Goal cannot be activated — select a Lead first",
        )
            .into_response(),
        Err(GoalError::InvalidTasks) => (
            StatusCode::BAD_REQUEST,
            "all_tasks requires existing task ids from this loca",
        )
            .into_response(),
        Err(GoalError::BoardOff) => goal_needs_board(),
        Err(GoalError::Storage) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save goal — try again",
        )
            .into_response(),
    }
}
/// An `all_tasks` goal set up while the Loca Board is off.
fn goal_needs_board() -> axum::response::Response {
    (
        StatusCode::CONFLICT,
        "a goal closed by its tasks needs the Loca Board on: turn it on (it needs a Lead), or make the goal manual",
    )
        .into_response()
}
pub(crate) async fn list_waits(State(hub): State<Hub>, access: RoomAccess) -> impl IntoResponse {
    Json(hub.waits(&access.room))
}
pub(crate) async fn set_wait(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    Json(mut body): Json<SetWait>,
) -> impl IntoResponse {
    let id = access.room;
    if !hub.is_writable(&id) {
        return (StatusCode::CONFLICT, "this loca is closed — read-only").into_response();
    }
    body.waiting_for = body.waiting_for.trim().to_string();
    body.reason = body.reason.trim().to_string();
    if body.waiting_for.is_empty() || body.reason.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            "waiting_for and reason are required",
        )
            .into_response();
    }
    let by = match actor_of(&hub, &headers, &body.by) {
        Ok(name) => name,
        Err(code) => return (code, "invalid or missing session").into_response(),
    };
    body.by = by;
    match hub.set_wait(&id, body) {
        Ok(wait) => (StatusCode::CREATED, Json(wait)).into_response(),
        Err(WaitError::SelfWait) => (
            StatusCode::BAD_REQUEST,
            "a participant cannot wait for itself",
        )
            .into_response(),
        Err(WaitError::Storage) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save wait — try again",
        )
            .into_response(),
        Err(WaitError::NotFound) => unreachable!(),
    }
}
pub(crate) async fn clear_wait(
    State(hub): State<Hub>,
    access: RoomAccess,
    Path((_id, name)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<ClearWait>,
) -> impl IntoResponse {
    let id = access.room;
    if !hub.is_writable(&id) {
        return (StatusCode::CONFLICT, "this loca is closed — read-only").into_response();
    }
    let actor = match actor_of(&hub, &headers, &body.by) {
        Ok(actor) => actor,
        Err(code) => return (code, "invalid or missing session").into_response(),
    };
    let is_operator = is_admin_req(&hub, &headers)
        || hub.is_loca_operator(&id, admin_token_of(&headers), session_of(&headers), &actor);
    if actor != name && !is_operator {
        return (
            StatusCode::FORBIDDEN,
            "only the waiter or a loca operator may clear this wait",
        )
            .into_response();
    }
    match hub.clear_wait(&id, &name) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(WaitError::NotFound) => (StatusCode::NOT_FOUND, "no such wait").into_response(),
        Err(WaitError::Storage) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not clear wait — try again",
        )
            .into_response(),
        Err(WaitError::SelfWait) => unreachable!(),
    }
}
