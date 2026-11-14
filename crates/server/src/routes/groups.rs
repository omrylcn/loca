//! Groups HTTP (PRINCIPLES "Groups"): list, found, rename, archive, delete
//! a group; add and remove its members; move a loca between groups; and the
//! fence that keeps a group loca invisible to everyone outside it.
use crate::*;

fn viewer(hub: &Hub, headers: &HeaderMap) -> hub::RequestAuthority {
    hub.resolve_authority(admin_token_of(headers), session_of(headers))
}

pub(crate) fn group_error(error: hub::GroupError) -> axum::response::Response {
    use hub::GroupError::*;
    let status = match error {
        NotFound | NoSuchPerson | NoSuchLoca => StatusCode::NOT_FOUND,
        NotOwner | RankRequired | SelfAdd | MasterNeverMember | FounderStays | Excluded
        | MasterOnly => StatusCode::FORBIDDEN,
        InvalidName => StatusCode::BAD_REQUEST,
        Archived | OpenLocas | CarryOverCapacity => StatusCode::CONFLICT,
        Storage => StatusCode::SERVICE_UNAVAILABLE,
    };
    (status, error.message()).into_response()
}

fn answer(result: Result<hub::GroupView, hub::GroupError>) -> axum::response::Response {
    match result {
        Ok(view) => Json(view).into_response(),
        Err(error) => group_error(error),
    }
}

/// `GET /groups` — the groups the caller may see: the Master every group, a
/// Smaster the ones it founded or belongs to, a member its own.
pub(crate) async fn list_groups(State(hub): State<Hub>, headers: HeaderMap) -> impl IntoResponse {
    Json(hub.groups_for(&viewer(&hub, &headers)))
}

/// `POST /groups {"name": …}` — the Master or a Smaster founds a group.
pub(crate) async fn create_group_route(
    State(hub): State<Hub>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("");
    match hub.create_group(name, &viewer(&hub, &headers)) {
        Ok(view) => (StatusCode::CREATED, Json(view)).into_response(),
        Err(error) => group_error(error),
    }
}

/// `GET /groups/:id`
pub(crate) async fn get_group(
    State(hub): State<Hub>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    answer(hub.group_view(&id, &viewer(&hub, &headers)))
}

/// `PATCH /groups/:id {"name"?: …, "archived"?: bool}` — founder or Master.
pub(crate) async fn update_group_route(
    State(hub): State<Hub>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let who = viewer(&hub, &headers);
    let name = body.get("name").and_then(|v| v.as_str());
    let archived = body.get("archived").and_then(|v| v.as_bool());
    if name.is_none() && archived.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            "nothing to change: name or archived",
        )
            .into_response();
    }
    if let Some(name) = name {
        if let Err(error) = hub.rename_group(&id, name, &who) {
            return group_error(error);
        }
    }
    if let Some(archived) = archived {
        if let Err(error) = hub.set_group_archived(&id, archived, &who) {
            return group_error(error);
        }
    }
    answer(hub.group_view(&id, &who))
}

/// `DELETE /groups/:id` — only when none of its locas is open.
pub(crate) async fn delete_group_route(
    State(hub): State<Hub>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    match hub.delete_group(&id, &viewer(&hub, &headers)) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => group_error(error),
    }
}

/// `GET /groups/:id/log` — every change, by whom and when (founder/Master).
pub(crate) async fn group_log_route(
    State(hub): State<Hub>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    match hub.group_log(&id, &viewer(&hub, &headers)) {
        Ok(log) => Json(log).into_response(),
        Err(error) => group_error(error),
    }
}

/// `PUT /groups/:id/members/:principal_id` — the founder or the Master adds
/// a person. Nobody adds themselves.
pub(crate) async fn add_group_member_route(
    State(hub): State<Hub>,
    Path((id, principal)): Path<(String, String)>,
    headers: HeaderMap,
) -> impl IntoResponse {
    answer(hub.add_group_member(&id, &principal, &viewer(&hub, &headers)))
}

/// `DELETE /groups/:id/members/:principal_id` — access to the group's locas
/// ends at once.
pub(crate) async fn remove_group_member_route(
    State(hub): State<Hub>,
    Path((id, principal)): Path<(String, String)>,
    headers: HeaderMap,
) -> impl IntoResponse {
    answer(hub.remove_group_member(&id, &principal, &viewer(&hub, &headers)))
}

/// `PUT /rooms/:id/group {"group": "<id>" | null}` — the Master moves a loca
/// between the default group and a group, or between groups.
pub(crate) async fn set_loca_group_route(
    State(hub): State<Hub>,
    Path(room): Path<String>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let group = match body.get("group") {
        Some(serde_json::Value::String(id)) if !id.is_empty() => Some(id.as_str()),
        Some(serde_json::Value::Null) | Some(serde_json::Value::String(_)) => None,
        _ => return (StatusCode::BAD_REQUEST, "group: an id, or null for default").into_response(),
    };
    match hub.set_loca_group(&room, group, &viewer(&hub, &headers)) {
        Ok(group) => Json(serde_json::json!({ "room": room, "group": group })).into_response(),
        Err(error) => group_error(error),
    }
}

/// `GET /rooms/:id/group/preview?group=<id>` (no `group` = default) — what
/// a move would do: who loses access, who keeps it, who is carried over by
/// davet, and which titles end. Master only; nothing changes.
pub(crate) async fn preview_loca_group_route(
    State(hub): State<Hub>,
    Path(room): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let group = query
        .get("group")
        .map(String::as_str)
        .filter(|g| !g.is_empty());
    match hub.loca_group_move_preview(&room, group, &viewer(&hub, &headers)) {
        Ok(preview) => Json(preview).into_response(),
        Err(error) => group_error(error),
    }
}

/// The group fence. Every `/rooms/:id/...` request for a group loca passes
/// here first, whatever the handler's own gate: anyone the door would not
/// admit — a Smaster outside the group included — gets exactly the answer a
/// loca that does not exist gets. Handlers gated only by rank (mode,
/// settings, moderation, invites, calls) therefore never act on it for them.
pub(crate) async fn group_fence(
    State(hub): State<Hub>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if !req.uri().path().starts_with("/rooms/") {
        return next.run(req).await;
    }
    // The loca id as the router decoded it: a name with a space or any
    // escaped character must be fenced under the same id the handler sees.
    let (req, room) = match super::access::decoded_room_request(&hub, req).await {
        Ok(parts) => parts,
        Err(error) => return *error,
    };
    let Some(room) = room else {
        return next.run(req).await;
    };
    if hub.is_group_loca(&room) && !room_decision_of(&hub, &room, req.headers()).is_allowed() {
        return (
            StatusCode::NOT_FOUND,
            "loca does not exist; create it first",
        )
            .into_response();
    }
    next.run(req).await
}
