//! The Vault over HTTP (PRINCIPLES "Vault"), routes of the `vault` extension;
//! the extension fence refuses all of them while it is off.
//!
//! - `GET /rooms/:id/vault` — the loca's rules, its revision, who wrote it
//!   and when, and whether the caller may write it. Anyone who may enter.
//! - `PUT /rooms/:id/vault {"body", "base_rev"}` — a new revision, written
//!   against the revision the writer read (409 when it is not the newest).
//!   Only the loca's appointed Operator, the Master and Smasters.
//! - `GET /rooms/:id/vault/history` — older revisions, newest first.
use super::extensions::{journal_actor, may_manage};
use crate::{
    admin_token_of, hub, session_of, HeaderMap, Hub, IntoResponse, Json, RoomAccess, State,
    StatusCode,
};

fn unavailable() -> axum::response::Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "could not read the rules — try again",
    )
        .into_response()
}

/// `GET /rooms/:id/vault`.
pub(crate) async fn get_vault(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
) -> axum::response::Response {
    let Ok(doc) = hub.vault(&access.room) else {
        return unavailable();
    };
    let mut view = serde_json::to_value(doc).unwrap_or_default();
    view["can_edit"] = serde_json::json!(may_manage(&hub, &access.room, &headers));
    Json(view).into_response()
}

/// `GET /rooms/:id/vault/history`.
pub(crate) async fn get_vault_history(
    State(hub): State<Hub>,
    access: RoomAccess,
) -> axum::response::Response {
    match hub.vault_history(&access.room) {
        Ok(revisions) => Json(serde_json::json!({
            "room": access.room,
            "revisions": revisions,
        }))
        .into_response(),
        Err(_) => unavailable(),
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct PutVault {
    body: String,
    base_rev: u64,
}

/// `PUT /rooms/:id/vault`. What the rules say changes no authority: the
/// check below is the loca's management check, never anything in the text.
pub(crate) async fn put_vault(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    Json(asked): Json<PutVault>,
) -> axum::response::Response {
    let room = &access.room;
    if hub.is_dm(room) {
        return (StatusCode::FORBIDDEN, hub::VaultError::Dm.message()).into_response();
    }
    if !may_manage(&hub, room, &headers) {
        return (
            StatusCode::FORBIDDEN,
            "only this loca's operator, the Master or a Smaster writes its Vault",
        )
            .into_response();
    }
    let (by, by_type) = journal_actor(&hub, &headers);
    let principal = hub
        .resolve_authority(admin_token_of(&headers), session_of(&headers))
        .principal_id;
    match hub.write_vault(
        room,
        &asked.body,
        asked.base_rev,
        &by,
        by_type,
        principal.as_deref(),
    ) {
        Ok(doc) => {
            let mut view = serde_json::to_value(doc).unwrap_or_default();
            view["can_edit"] = serde_json::json!(true);
            Json(view).into_response()
        }
        Err(error) => {
            let status = match error {
                hub::VaultError::Invalid(_) => StatusCode::BAD_REQUEST,
                hub::VaultError::TooLong => StatusCode::PAYLOAD_TOO_LARGE,
                hub::VaultError::Stale { .. } | hub::VaultError::NotWritable => {
                    StatusCode::CONFLICT
                }
                hub::VaultError::Dm => StatusCode::FORBIDDEN,
                hub::VaultError::NoStorage | hub::VaultError::Storage => {
                    StatusCode::SERVICE_UNAVAILABLE
                }
            };
            (status, error.message()).into_response()
        }
    }
}
