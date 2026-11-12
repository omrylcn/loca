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
use crate::*;

/// The `/rooms/:id/<first segment>` that belongs to each extension. `live`
/// has no routes of its own; `PUT /settings` refuses to turn it on.
fn extension_of_segment(first: &str) -> Option<&'static str> {
    match first {
        "tasks" | "goals" => Some("focus"),
        "notes" => Some("notes"),
        "journal" => Some("journal"),
        "wiki" => Some("wiki"),
        _ => None,
    }
}

/// The one refusal every disabled extension gives, reads and writes alike.
pub(crate) fn extension_off(id: &str) -> axum::response::Response {
    let name = protocol::extension_spec(id)
        .map(|spec| spec.name)
        .unwrap_or(id);
    (
        StatusCode::FORBIDDEN,
        format!(
            "this extension is off in this loca: {id} ({name}). Its records are kept; \
             the loca's operator, the Master or a Smaster can turn it back on."
        ),
    )
        .into_response()
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
    let path = req.uri().path();
    let Some(rest) = path.strip_prefix("/rooms/") else {
        return next.run(req).await;
    };
    let mut parts = rest.splitn(3, '/');
    let room = parts.next().unwrap_or("").to_string();
    let first = parts.next().unwrap_or("");
    let Some(extension) = extension_of_segment(first) else {
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
/// and Smasters (`is_room_operator_req`). Everyone else reads them.
fn may_manage(hub: &Hub, room: &str, headers: &HeaderMap) -> bool {
    !hub.is_birebir(room) && is_room_operator_req(hub, room, headers)
}

fn room_view(hub: &Hub, room: &str, headers: &HeaderMap) -> serde_json::Value {
    serde_json::json!({
        "room": room,
        "enabled": hub.enabled_extensions(room),
        "catalog": protocol::EXTENSIONS,
        "can_manage": may_manage(hub, room, headers),
        "birebir": hub.is_birebir(room),
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

fn journal_actor(hub: &Hub, headers: &HeaderMap) -> (String, SenderType) {
    let authority = hub.resolve_authority(admin_token_of(headers), session_of(headers));
    if let Some(name) = authority.display_name {
        return (name, authority.kind.unwrap_or(SenderType::User));
    }
    if let Some(identity) = hub.session_identity(session_of(headers)) {
        return (identity.name, identity.kind);
    }
    ("master".to_string(), SenderType::User)
}

fn apply(
    hub: &Hub,
    room: &str,
    headers: &HeaderMap,
    wanted: Vec<String>,
) -> axum::response::Response {
    if hub.is_birebir(room) {
        return (StatusCode::FORBIDDEN, "a birebir has no extensions").into_response();
    }
    if !may_manage(hub, room, headers) {
        return (
            StatusCode::FORBIDDEN,
            "only this loca's operator, the Master or a Smaster changes its extensions",
        )
            .into_response();
    }
    let (by, by_type) = journal_actor(hub, headers);
    match hub.set_extensions(room, &wanted, &by, by_type) {
        Ok(change) => {
            let mut view = room_view(hub, room, headers);
            view["added"] = serde_json::json!(change.added);
            view["removed"] = serde_json::json!(change.removed);
            Json(view).into_response()
        }
        Err(error) => {
            let status = match error {
                hub::ExtensionError::Unknown(_) => StatusCode::BAD_REQUEST,
                hub::ExtensionError::Birebir => StatusCode::FORBIDDEN,
                hub::ExtensionError::NotWritable => StatusCode::CONFLICT,
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
    Json(body): Json<SetExtensions>,
) -> impl IntoResponse {
    apply(&hub, &access.room, &headers, body.enabled)
}

/// `PUT /rooms/:id/extensions/:ext` — add one.
pub(crate) async fn add_extension(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    Path(params): Path<HashMap<String, String>>,
) -> impl IntoResponse {
    let ext = params.get("ext").cloned().unwrap_or_default();
    let mut wanted = hub.enabled_extensions(&access.room);
    wanted.push(ext);
    apply(&hub, &access.room, &headers, wanted)
}

/// `DELETE /rooms/:id/extensions/:ext` — remove one. Its records stay.
pub(crate) async fn remove_extension(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    Path(params): Path<HashMap<String, String>>,
) -> impl IntoResponse {
    let ext = params.get("ext").cloned().unwrap_or_default();
    if protocol::extension_spec(&ext).is_none() {
        return (StatusCode::BAD_REQUEST, format!("unknown extension: {ext}")).into_response();
    }
    let wanted: Vec<String> = hub
        .enabled_extensions(&access.room)
        .into_iter()
        .filter(|id| *id != ext)
        .collect();
    apply(&hub, &access.room, &headers, wanted)
}
