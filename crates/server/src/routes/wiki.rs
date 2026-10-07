use crate::store::WikiError;
use crate::*;

fn error(e: WikiError) -> axum::response::Response {
    match e {
        WikiError::PersistenceUnavailable | WikiError::Storage => {
            (StatusCode::SERVICE_UNAVAILABLE, "wiki storage unavailable").into_response()
        }
        WikiError::NotConfigured => (StatusCode::NOT_FOUND, "wiki not configured").into_response(),
        WikiError::NotEditor => {
            (StatusCode::FORBIDDEN, "assigned wiki editor only").into_response()
        }
        WikiError::Conflict => {
            (StatusCode::CONFLICT, "reread wiki before retrying").into_response()
        }
        WikiError::InvalidInput => (StatusCode::BAD_REQUEST, "invalid wiki update").into_response(),
    }
}

pub(crate) async fn get_wiki(State(hub): State<Hub>, access: RoomAccess) -> impl IntoResponse {
    match hub.wiki_snapshot(&access.room) {
        Ok(snapshot) => Json(snapshot).into_response(),
        Err(e) => error(e),
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct WikiConfiguration {
    editor: String,
    #[serde(default)]
    enabled: bool,
    interval_messages: u64,
}

pub(crate) async fn configure_wiki(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    Json(body): Json<WikiConfiguration>,
) -> impl IntoResponse {
    if !hub.is_writable(&access.room) {
        return (StatusCode::CONFLICT, "loca is read-only").into_response();
    }
    if !hub.is_loca_operator(
        &access.room,
        admin_token_of(&headers),
        session_of(&headers),
        "",
    ) {
        return (StatusCode::FORBIDDEN, "loca operator required").into_response();
    }
    // Fail closed until the maintenance scheduler is wired. Saving an enabled
    // flag must never promise model work that no runtime can perform.
    if body.enabled {
        return (
            StatusCode::CONFLICT,
            "automatic wiki maintenance is not available yet",
        )
            .into_response();
    }
    match hub.configure_wiki(
        &access.room,
        body.editor.trim(),
        body.enabled,
        body.interval_messages,
    ) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => error(e),
    }
}

pub(crate) async fn commit_wiki(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    Json(body): Json<crate::store::WikiCommit>,
) -> impl IntoResponse {
    if !hub.is_writable(&access.room) {
        return (StatusCode::CONFLICT, "loca is read-only").into_response();
    }
    let Some(actor) = hub.session_identity(session_of(&headers)) else {
        return (StatusCode::UNAUTHORIZED, "valid session required").into_response();
    };
    let Some(principal) = actor.principal_id else {
        return (StatusCode::FORBIDDEN, "principal-bound editor required").into_response();
    };
    match hub.commit_wiki(&access.room, &principal, &body) {
        Ok(revision) => Json(serde_json::json!({"revision": revision})).into_response(),
        Err(e) => error(e),
    }
}
