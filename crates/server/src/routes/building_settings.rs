//! `GET/PUT /building/settings`: the Building settings view. The Master and
//! Smasters read it; only the Master changes the one runtime setting, the
//! Building's display name. See `hub/building_settings.rs`.
use crate::*;

/// `GET /building/settings` — the Master and Smasters.
pub(crate) async fn get_building_settings(
    State(hub): State<Hub>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !is_admin_req(&hub, &headers) {
        return (StatusCode::FORBIDDEN, "Building authority required").into_response();
    }
    let viewer = hub.resolve_authority(admin_token_of(&headers), session_of(&headers));
    let public_server = std::env::var("PUBLIC_SERVER_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    Json(hub.building_settings(&viewer, public_server)).into_response()
}

/// `PUT /building/settings {"name": "…" | null}` — the Master names the
/// Building; an empty string or null clears the name. Nothing else is
/// accepted: every other setting comes from the environment.
pub(crate) async fn put_building_settings(
    State(hub): State<Hub>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let actor = hub.resolve_authority(admin_token_of(&headers), session_of(&headers));
    if !actor.is_master() {
        return (
            StatusCode::FORBIDDEN,
            "only the Master changes Building settings",
        )
            .into_response();
    }
    let Some(fields) = body.as_object() else {
        return (StatusCode::BAD_REQUEST, "expected a JSON object").into_response();
    };
    if let Some(key) = fields.keys().find(|key| key.as_str() != "name") {
        return (
            StatusCode::BAD_REQUEST,
            format!(
                "unknown field `{key}`: only name is set here; the rest comes from the server's environment"
            ),
        )
            .into_response();
    }
    let raw = match fields.get("name") {
        Some(serde_json::Value::String(name)) => name.as_str(),
        Some(serde_json::Value::Null) => "",
        Some(_) => {
            return (StatusCode::BAD_REQUEST, "name must be a string or null").into_response()
        }
        None => return (StatusCode::BAD_REQUEST, "name is required").into_response(),
    };
    match hub.set_building_name(&actor, raw) {
        Ok(name) => Json(serde_json::json!({ "name": name })).into_response(),
        Err(hub::BuildingNameError::MasterOnly) => (
            StatusCode::FORBIDDEN,
            "only the Master changes Building settings",
        )
            .into_response(),
        Err(hub::BuildingNameError::Invalid(reason)) => {
            (StatusCode::BAD_REQUEST, reason).into_response()
        }
        Err(hub::BuildingNameError::MemoryOnly) => (
            StatusCode::CONFLICT,
            "this Building runs without a database (DB_PATH), so its name cannot be kept",
        )
            .into_response(),
        Err(hub::BuildingNameError::Storage) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save the Building name — try again",
        )
            .into_response(),
    }
}
