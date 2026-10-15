use crate::store::WikiError;
use crate::*;

fn error(e: WikiError) -> axum::response::Response {
    match e {
        WikiError::PersistenceUnavailable | WikiError::Storage => {
            (StatusCode::SERVICE_UNAVAILABLE, "wiki storage unavailable").into_response()
        }
        WikiError::NotConfigured => (StatusCode::NOT_FOUND, "wiki not configured").into_response(),
        WikiError::PageNotFound => (StatusCode::NOT_FOUND, "wiki page not found").into_response(),
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

pub(crate) async fn get_wiki_page(
    State(hub): State<Hub>,
    access: RoomAccess,
    Path(params): Path<HashMap<String, String>>,
) -> impl IntoResponse {
    match hub.wiki_page(&access.room, &params["slug"]) {
        Ok(page) => Json(page).into_response(),
        Err(e) => error(e),
    }
}

pub(crate) async fn remove_wiki_page(
    State(hub): State<Hub>,
    access: RoomAccess,
    Path(params): Path<HashMap<String, String>>,
    headers: HeaderMap,
    Json(body): Json<crate::store::WikiPageRemoval>,
) -> impl IntoResponse {
    if !hub.is_writable(&access.room) {
        return (StatusCode::CONFLICT, "loca is read-only").into_response();
    }
    let Some(principal) = hub
        .session_identity(session_of(&headers))
        .and_then(|a| a.principal_id)
    else {
        return (
            StatusCode::UNAUTHORIZED,
            "principal-bound editor session required",
        )
            .into_response();
    };
    match hub.remove_wiki_page(&access.room, &principal, &params["slug"], &body) {
        Ok(revision) => {
            Json(serde_json::json!({"room":access.room,"slug":params["slug"],"revision":revision}))
                .into_response()
        }
        Err(e) => error(e),
    }
}

pub(crate) async fn edit_wiki_page(
    State(hub): State<Hub>,
    access: RoomAccess,
    Path(params): Path<HashMap<String, String>>,
    headers: HeaderMap,
    Json(body): Json<crate::store::WikiPageEdit>,
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
    let actor = hub
        .session_identity(session_of(&headers))
        .and_then(|identity| identity.principal_id)
        .map(|id| format!("operator:{id}"))
        .unwrap_or_else(|| "operator:master".into());
    match hub.edit_wiki_page(&access.room, &actor, &params["slug"], &body) {
        Ok(revision) => {
            Json(serde_json::json!({"room":access.room,"slug":params["slug"],"revision":revision}))
                .into_response()
        }
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
    // Hub requires a fresh, identity-bound native worker preflight heartbeat.
    // Assignment and enabling remain separate operator decisions.
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

pub(crate) async fn claim_wiki_review(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !hub.is_writable(&access.room) {
        return (StatusCode::CONFLICT, "loca is read-only").into_response();
    }
    let Some(principal) = hub
        .session_identity(session_of(&headers))
        .and_then(|a| a.principal_id)
    else {
        return (StatusCode::UNAUTHORIZED, "principal-bound session required").into_response();
    };
    match hub.claim_wiki_review(&access.room, &principal) {
        Ok(batch) => Json(batch).into_response(),
        Err(e) => error(e),
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct WikiBatchCommit {
    lease: crate::store::WikiLease,
    review: crate::store::WikiCommit,
}

pub(crate) async fn renew_wiki_review(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    Json(lease): Json<crate::store::WikiLease>,
) -> impl IntoResponse {
    if !hub.is_writable(&access.room) {
        return (StatusCode::CONFLICT, "loca is read-only").into_response();
    }
    let Some(principal) = hub
        .session_identity(session_of(&headers))
        .and_then(|a| a.principal_id)
    else {
        return (StatusCode::UNAUTHORIZED, "principal-bound session required").into_response();
    };
    match hub.renew_wiki_review(&access.room, &principal, &lease) {
        Ok(expires) => Json(serde_json::json!({"lease_expires_at":expires})).into_response(),
        Err(e) => error(e),
    }
}

pub(crate) async fn finish_wiki_review(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    Json(body): Json<WikiBatchCommit>,
) -> impl IntoResponse {
    if !hub.is_writable(&access.room) {
        return (StatusCode::CONFLICT, "loca is read-only").into_response();
    }
    let Some(principal) = hub
        .session_identity(session_of(&headers))
        .and_then(|a| a.principal_id)
    else {
        return (StatusCode::UNAUTHORIZED, "principal-bound session required").into_response();
    };
    match hub.finish_wiki_review(&access.room, &principal, &body.review, &body.lease) {
        Ok(revision) => Json(serde_json::json!({"revision":revision})).into_response(),
        Err(e) => error(e),
    }
}

pub(crate) async fn wiki_maintenance_status(
    State(hub): State<Hub>,
    access: RoomAccess,
) -> impl IntoResponse {
    match hub.wiki_batch_status(&access.room) {
        Ok(status) => Json(serde_json::json!({"status":status})).into_response(),
        Err(e) => error(e),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WikiNativeReady {
    runtime: String,
}

pub(crate) async fn wiki_native_ready(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    Json(body): Json<WikiNativeReady>,
) -> impl IntoResponse {
    let Some(actor) = hub.session_identity(session_of(&headers)) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if actor.kind != SenderType::Agent || !hub.is_writable(&access.room) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(principal) = actor.principal_id else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    match hub.wiki_native_ready(&access.room, &principal, &body.runtime) {
        Ok(()) => Json(serde_json::json!({"ready":true})).into_response(),
        Err(e) => error(e),
    }
}

pub(crate) async fn wiki_context(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(principal) = hub
        .session_identity(session_of(&headers))
        .and_then(|a| a.principal_id)
    else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    match hub.wiki_context_for(&access.room, &principal) {
        Ok(value) => Json(value).into_response(),
        Err(e) => error(e),
    }
}

pub(crate) async fn wiki_context_request(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    Json(body): Json<crate::store::WikiContextSelection>,
) -> impl IntoResponse {
    let Some(principal) = hub
        .session_identity(session_of(&headers))
        .and_then(|a| a.principal_id)
    else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !hub.is_writable(&access.room) {
        return StatusCode::CONFLICT.into_response();
    }
    match hub.select_wiki_context(&access.room, &principal, &body) {
        Ok(()) => Json(serde_json::json!({"requested":true})).into_response(),
        Err(e) => error(e),
    }
}

pub(crate) async fn wiki_context_accept(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    Json(body): Json<crate::store::WikiContextAck>,
) -> impl IntoResponse {
    let Some(principal) = hub
        .session_identity(session_of(&headers))
        .and_then(|a| a.principal_id)
    else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !hub.is_writable(&access.room) {
        return StatusCode::CONFLICT.into_response();
    }
    match hub.accept_wiki_context(&access.room, &principal, &body) {
        Ok(()) => Json(serde_json::json!({"accepted":true})).into_response(),
        Err(e) => error(e),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WikiContextOffer {
    target: String,
    slugs: Vec<String>,
    reason: String,
}

pub(crate) async fn wiki_context_offer(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    Json(body): Json<WikiContextOffer>,
) -> impl IntoResponse {
    if !hub.is_writable(&access.room) {
        return StatusCode::CONFLICT.into_response();
    }
    let principal = hub
        .session_identity(session_of(&headers))
        .and_then(|a| a.principal_id);
    let operator = hub.is_loca_operator(
        &access.room,
        admin_token_of(&headers),
        session_of(&headers),
        "",
    );
    let selection = crate::store::WikiContextSelection {
        slugs: body.slugs,
        reason: body.reason,
    };
    match hub.offer_wiki_context(
        &access.room,
        principal.as_deref(),
        operator,
        &body.target,
        &selection,
    ) {
        Ok(()) => Json(serde_json::json!({"offered":true})).into_response(),
        Err(e) => error(e),
    }
}

pub(crate) async fn wiki_context_requests(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
) -> impl IntoResponse {
    let principal = hub
        .session_identity(session_of(&headers))
        .and_then(|a| a.principal_id);
    let operator = hub.is_loca_operator(
        &access.room,
        admin_token_of(&headers),
        session_of(&headers),
        "",
    );
    match hub.wiki_context_requests(&access.room, principal.as_deref(), operator) {
        Ok(value) => Json(value).into_response(),
        Err(e) => error(e),
    }
}
