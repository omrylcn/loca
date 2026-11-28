//! Loca extensions over HTTP (PRINCIPLES "Loca extensions").
//!
//! - `GET /extensions` — the catalog and the new-loca defaults.
//! - `GET /rooms/:id/extensions` — what is on here, and whether the caller
//!   may change it. Anyone who may enter the loca reads it.
//! - `PUT /rooms/:id/extensions {"enabled": [...]}` — replace the set.
//! - `PUT|DELETE /rooms/:id/extensions/:ext` — add or remove one.
//!
//! Changes are for the loca's appointed Operator, the Master and Smasters.
//! The fence below refuses every route of an extension that is off.
use super::rooms::is_room_operator_req;
use crate::{
    admin_token_of, hub, room_decision_of, session_of, HashMap, HeaderMap, Hub, IntoResponse, Json,
    Path, Query, RoomAccess, SenderType, State, StatusCode,
};

/// The one refusal every disabled extension gives, reads and writes alike.
/// The Loca Board's says plainly what is off, in front of the shared words.
pub(crate) fn extension_off(id: &str) -> axum::response::Response {
    let name = protocol::extension_spec(id)
        .map(|spec| spec.name)
        .unwrap_or(id);
    let text = if id == "board" {
        "The Loca Board is off in this loca — this extension is off in this loca: board \
         (Loca Board). Its tasks are kept; the loca's operator, the Master or a Smaster \
         can turn it on, and it needs a Lead."
            .to_string()
    } else {
        format!(
            "this extension is off in this loca: {id} ({name}). Its records are kept; \
             the loca's operator, the Master or a Smaster can turn it back on."
        )
    };
    (StatusCode::FORBIDDEN, text).into_response()
}

/// The extension fence. A request under `/rooms/:id/...` that belongs to an
/// extension this loca has off answers 403 — reads as well as writes, so an
/// agent learns the reason instead of an empty list it might trust. A caller
/// who may not enter the loca at all passes through to the handler's own gate,
/// so the fence never tells outsiders what a loca has switched on.
pub(crate) async fn extension_fence(
    State(hub): State<Hub>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if !req.uri().path().starts_with("/rooms/") {
        return next.run(req).await;
    }
    let (req, room) = match super::access::decoded_room_request(&hub, req).await {
        Ok(parts) => parts,
        Err(error) => return *error,
    };
    let Some(room) = room else {
        return next.run(req).await;
    };
    // The registry (`protocol::EXTENSIONS`) says which extension owns the
    // first segment after the loca. `live` owns none; `PUT /settings`
    // refuses to turn it on.
    let first = req.uri().path().split('/').nth(3).unwrap_or("");
    let Some(extension) = protocol::extension_of_segment(first).map(|spec| spec.id) else {
        return next.run(req).await;
    };
    if hub.extension_enabled(&room, extension) {
        return next.run(req).await;
    }
    if !matches!(
        room_decision_of(&hub, &room, req.headers()),
        hub::EnterDecision::Allowed
    ) {
        return next.run(req).await;
    }
    extension_off(extension)
}

/// Who may change a loca's extensions: its appointed Operator, the Master
/// and Smasters (`is_room_operator_req`). Everyone else reads them. The
/// loca's identity (description, seats, icon, colour, labels) uses the same
/// check.
pub(crate) fn may_manage(hub: &Hub, room: &str, headers: &HeaderMap) -> bool {
    !hub.is_dm(room) && is_room_operator_req(hub, room, headers)
}

fn room_view(hub: &Hub, room: &str, headers: &HeaderMap) -> serde_json::Value {
    serde_json::json!({
        "room": room,
        "enabled": hub.enabled_extensions(room),
        // Tasks are kept while the Loca Board is off; Focus says so.
        "stored_tasks": hub.stored_task_count(room),
        "catalog": protocol::EXTENSIONS,
        "can_manage": may_manage(hub, room, headers),
        "birebir": hub.is_dm(room),
    })
}

/// `GET /extensions` — the catalog, for the create screen.
pub(crate) async fn extension_catalog() -> impl IntoResponse {
    Json(serde_json::json!({
        "catalog": protocol::EXTENSIONS,
        "defaults": protocol::default_new_extensions(),
    }))
}

/// `GET /rooms/:id/extensions`.
pub(crate) async fn get_extensions(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
) -> impl IntoResponse {
    Json(room_view(&hub, &access.room, &headers))
}

#[derive(serde::Deserialize)]
pub(crate) struct SetExtensions {
    enabled: Vec<String>,
}

/// Who is acting, by a name the server verified: a session or a rank. A
/// request that names nobody gets `None` (a lesson proposes nothing, a mentat
/// request opens nothing).
pub(crate) fn known_actor(hub: &Hub, headers: &HeaderMap) -> Option<(String, SenderType)> {
    let authority = hub.resolve_authority(admin_token_of(headers), session_of(headers));
    if let Some(name) = authority.display_name {
        return Some((name, authority.kind.unwrap_or(SenderType::User)));
    }
    hub.session_identity(session_of(headers))
        .map(|identity| (identity.name, identity.kind))
}

pub(crate) fn journal_actor(hub: &Hub, headers: &HeaderMap) -> (String, SenderType) {
    let authority = hub.resolve_authority(admin_token_of(headers), session_of(headers));
    if let Some(name) = authority.display_name {
        return (name, authority.kind.unwrap_or(SenderType::User));
    }
    if let Some(identity) = hub.session_identity(session_of(headers)) {
        return (identity.name, identity.kind);
    }
    ("master".to_string(), SenderType::User)
}

/// `?open_tasks=keep|done|remove`: what becomes of the Loca Board's
/// unfinished tasks when a change turns it off.
#[derive(serde::Deserialize, Default)]
pub(crate) struct OpenTasksQuery {
    open_tasks: Option<String>,
}

impl OpenTasksQuery {
    /// The choice, or `Err` for a value that is none of the three.
    fn choice(&self) -> Result<Option<hub::OpenTasks>, ()> {
        match self.open_tasks.as_deref() {
            None => Ok(None),
            Some("keep") => Ok(Some(hub::OpenTasks::Keep)),
            Some("done") => Ok(Some(hub::OpenTasks::Done)),
            Some("remove") => Ok(Some(hub::OpenTasks::Remove)),
            Some(_) => Err(()),
        }
    }
}

fn apply(
    hub: &Hub,
    room: &str,
    headers: &HeaderMap,
    edit: hub::ExtensionEdit<'_>,
    open_tasks: &OpenTasksQuery,
) -> axum::response::Response {
    if hub.is_dm(room) {
        return (StatusCode::FORBIDDEN, "a birebir has no extensions").into_response();
    }
    if !may_manage(hub, room, headers) {
        return (
            StatusCode::FORBIDDEN,
            "only this loca's operator, the Master or a Smaster changes its extensions",
        )
            .into_response();
    }
    let Ok(choice) = open_tasks.choice() else {
        return (
            StatusCode::BAD_REQUEST,
            "open_tasks is keep, done or remove",
        )
            .into_response();
    };
    // A mentat room is a place to consult, not to hand out work.
    let asked: Vec<&str> = match edit {
        hub::ExtensionEdit::Replace(list) => list.iter().map(String::as_str).collect(),
        hub::ExtensionEdit::Add(one) => vec![one],
        hub::ExtensionEdit::Remove(_) => Vec::new(),
    };
    if hub.is_mentat_room(room)
        && asked
            .iter()
            .any(|id| hub::MENTAT_NO_EXTENSIONS.contains(id))
    {
        return (
            StatusCode::CONFLICT,
            "a mentat room takes no Loca Board, Focus or Live: it is a place to consult",
        )
            .into_response();
    }
    let (by, by_type) = journal_actor(hub, headers);
    let note = match hub.settle_board_off(room, edit, choice, &by) {
        Ok(note) => note,
        Err(hub::BoardOff::Ask(unfinished)) => {
            return super::board_off::ask(unfinished);
        }
        Err(hub::BoardOff::Storage) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "could not close the board's tasks — try again",
            )
                .into_response();
        }
    };
    match hub.edit_extensions_noting(room, edit, &by, by_type, note.as_deref()) {
        Ok(change) => {
            let mut view = room_view(hub, room, headers);
            view["added"] = serde_json::json!(change.added);
            view["removed"] = serde_json::json!(change.removed);
            Json(view).into_response()
        }
        Err(error) => {
            let status = match error {
                hub::ExtensionError::Unknown(_) => StatusCode::BAD_REQUEST,
                hub::ExtensionError::Dm => StatusCode::FORBIDDEN,
                hub::ExtensionError::NotWritable | hub::ExtensionError::BoardNeedsLead => {
                    StatusCode::CONFLICT
                }
                hub::ExtensionError::Storage => StatusCode::SERVICE_UNAVAILABLE,
            };
            (status, error.message()).into_response()
        }
    }
}

/// `PUT /rooms/:id/extensions {"enabled": [...]}` — replace the whole set.
pub(crate) async fn put_extensions(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    Query(open_tasks): Query<OpenTasksQuery>,
    Json(body): Json<SetExtensions>,
) -> impl IntoResponse {
    apply(
        &hub,
        &access.room,
        &headers,
        hub::ExtensionEdit::Replace(&body.enabled),
        &open_tasks,
    )
}

/// `PUT /rooms/:id/extensions/:ext` — add one.
pub(crate) async fn add_extension(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    Path(params): Path<HashMap<String, String>>,
) -> impl IntoResponse {
    let ext = params.get("ext").cloned().unwrap_or_default();
    apply(
        &hub,
        &access.room,
        &headers,
        hub::ExtensionEdit::Add(&ext),
        &OpenTasksQuery::default(),
    )
}

/// `DELETE /rooms/:id/extensions/:ext` — remove one. Its records stay.
pub(crate) async fn remove_extension(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    Path(params): Path<HashMap<String, String>>,
    Query(open_tasks): Query<OpenTasksQuery>,
) -> impl IntoResponse {
    let ext = params.get("ext").cloned().unwrap_or_default();
    if protocol::extension_spec(&ext).is_none() {
        return (StatusCode::BAD_REQUEST, format!("unknown extension: {ext}")).into_response();
    }
    apply(
        &hub,
        &access.room,
        &headers,
        hub::ExtensionEdit::Remove(&ext),
        &open_tasks,
    )
}
