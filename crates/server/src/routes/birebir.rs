//! Birebir HTTP: open one, see one's own, call the Master in, and the
//! Master's existence-only view (PRINCIPLES "Birebir").
use crate::*;

fn caller_principal(hub: &Hub, headers: &HeaderMap) -> Option<String> {
    hub.resolve_authority(admin_token_of(headers), session_of(headers))
        .principal_id
}

fn birebir_error(error: hub::BirebirError) -> axum::response::Response {
    use hub::BirebirError::*;
    let status = match error {
        NotHuman | NoIdentity => StatusCode::FORBIDDEN,
        PeerNotFound | NotParty => StatusCode::NOT_FOUND,
        SelfPair => StatusCode::BAD_REQUEST,
        NoSharedTable | NotCaller => StatusCode::FORBIDDEN,
        Sealed | MasterNotIn => StatusCode::CONFLICT,
        Storage => StatusCode::SERVICE_UNAVAILABLE,
    };
    (status, error.message()).into_response()
}

/// `POST /birebirs {"name": "<person>"}` — open (or return) the birebir with
/// that person. A person signed in with their own session opens it.
pub(crate) async fn open_birebir_route(
    State(hub): State<Hub>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let Some(opener) = hub.session_identity(session_of(&headers)) else {
        return birebir_error(hub::BirebirError::NoIdentity);
    };
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if name.is_empty() {
        return (StatusCode::BAD_REQUEST, "with whom?").into_response();
    }
    match hub.open_birebir(&opener, name) {
        Ok((b, created)) => {
            let me = opener.principal_id.unwrap_or_default();
            let view = hub.birebir_view(&b.room, &me);
            (
                if created {
                    StatusCode::CREATED
                } else {
                    StatusCode::OK
                },
                Json(serde_json::json!({ "created": created, "birebir": view })),
            )
                .into_response()
        }
        Err(error) => birebir_error(error),
    }
}

/// `GET /birebirs/candidates` — whom the caller may start a birebir with.
pub(crate) async fn birebir_candidates_route(
    State(hub): State<Hub>,
    headers: HeaderMap,
) -> impl IntoResponse {
    match hub.session_identity(session_of(&headers)) {
        Some(me) => Json(hub.birebir_candidates(&me)).into_response(),
        None => Json(Vec::<hub::BirebirCandidate>::new()).into_response(),
    }
}

/// `GET /birebirs` — the caller's own birebirs, and nobody else's.
pub(crate) async fn list_my_birebirs(
    State(hub): State<Hub>,
    headers: HeaderMap,
) -> impl IntoResponse {
    match caller_principal(&hub, &headers) {
        Some(me) => Json(hub.birebirs_of(&me)).into_response(),
        None => Json(Vec::<hub::BirebirView>::new()).into_response(),
    }
}

/// `POST /birebirs/:room/master {"action": "call" | "dismiss" | "history"}` —
/// one side calls the Master in, sends the Master out, or allows history.
pub(crate) async fn birebir_master_route(
    State(hub): State<Hub>,
    Path(room): Path<String>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let Some(me) = caller_principal(&hub, &headers) else {
        return birebir_error(hub::BirebirError::NotParty);
    };
    let result = match body.get("action").and_then(|v| v.as_str()) {
        Some("call") => hub.birebir_call_master(&room, &me),
        Some("dismiss") => hub.birebir_dismiss_master(&room, &me),
        Some("history") => hub.birebir_allow_history(&room, &me),
        _ => return (StatusCode::BAD_REQUEST, "action: call, dismiss or history").into_response(),
    };
    match result {
        Ok(b) => Json(hub.birebir_view(&b.room, &me)).into_response(),
        Err(error) => birebir_error(error),
    }
}

/// `GET /building/birebirs` — Master only: that birebirs exist, between whom,
/// and when last active. Never their words.
pub(crate) async fn birebir_overview_route(
    State(hub): State<Hub>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !is_master_req(&hub, &headers) {
        return (StatusCode::FORBIDDEN, "only the Master sees this").into_response();
    }
    Json(hub.birebir_overview()).into_response()
}

/// `GET /building/birebirs/:room/messages` — Master only, and only while
/// called in: from the call on, or all of it once both sides allowed it.
/// Like `GET /rooms/:id/messages`, the bare form is the bounded hot tail and
/// `?since=<id>&limit=<n>` pages the durable archive.
pub(crate) async fn birebir_witness_route(
    State(hub): State<Hub>,
    Path(room): Path<String>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !is_master_req(&hub, &headers) {
        return (StatusCode::FORBIDDEN, "only the Master sees this").into_response();
    }
    let messages = match q.get("since").and_then(|s| s.parse::<u64>().ok()) {
        None => hub.birebir_witness_messages(&room),
        Some(since) => {
            let limit = q
                .get("limit")
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(200)
                .clamp(1, 1_000);
            match hub.birebir_witness_messages_after(&room, since, limit) {
                Some(Ok(messages)) => Some(messages),
                Some(Err(error)) => {
                    tracing::error!(%error, "durable birebir witness read failed");
                    return (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "could not read message archive — retry",
                    )
                        .into_response();
                }
                None => None,
            }
        }
    };
    match messages {
        Some(messages) => Json(serde_json::json!({
            "history_open": hub.birebir(&room).map(|b| b.history_open()).unwrap_or(false),
            "messages": messages,
        }))
        .into_response(),
        None => (StatusCode::FORBIDDEN, "nobody called you into this birebir").into_response(),
    }
}

/// `POST /building/birebirs/:room/close {"archived": bool}` and
/// `POST /building/birebirs/:room/seal` — Master only. Neither reads a word.
pub(crate) async fn birebir_close_route(
    State(hub): State<Hub>,
    Path(room): Path<String>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if !is_master_req(&hub, &headers) {
        return (StatusCode::FORBIDDEN, "only the Master closes a birebir").into_response();
    }
    if !hub.is_birebir(&room) || hub.is_deleted(&room) {
        return (StatusCode::NOT_FOUND, "no such birebir").into_response();
    }
    let archived = body
        .get("archived")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    match hub.set_settings(
        &room,
        None,
        None,
        None,
        Some(archived),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    ) {
        Ok(_) => Json(serde_json::json!({ "room": room, "archived": archived })).into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save — try again",
        )
            .into_response(),
    }
}

pub(crate) async fn birebir_seal_route(
    State(hub): State<Hub>,
    Path(room): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !is_master_req(&hub, &headers) {
        return (StatusCode::FORBIDDEN, "only the Master seals a birebir").into_response();
    }
    if !hub.is_birebir(&room) || hub.is_deleted(&room) {
        return (StatusCode::NOT_FOUND, "no such birebir").into_response();
    }
    if !hub.is_archived(&room) {
        return (StatusCode::CONFLICT, "close the birebir before sealing it").into_response();
    }
    match hub.delete_room(&room) {
        Ok(_) => Json(serde_json::json!({ "room": room, "sealed": true })).into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not seal — try again",
        )
            .into_response(),
    }
}

/// The birebir fence. Every `/rooms/:id/...` request for a birebir passes
/// here first, whatever the handler's own gate:
/// - someone who is not one of the two gets the same answer as a loca that
///   does not exist — a birebir's existence is nobody else's business;
/// - the two may read, and may write only conversation: messages,
///   attachments, reactions. No tasks, goals, notes, wiki, memory, lead,
///   operators, modes, settings, invites, calls or moderation.
pub(crate) async fn birebir_fence(
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
    let sub = req
        .uri()
        .path()
        .splitn(4, '/')
        .nth(3)
        .unwrap_or("")
        .trim_end_matches('/');
    if !hub.is_birebir(&room) {
        return next.run(req).await;
    }
    if !matches!(
        room_decision_of(&hub, &room, req.headers()),
        hub::EnterDecision::Allowed
    ) {
        return (StatusCode::NOT_FOUND, "no such loca").into_response();
    }
    let first = sub.split('/').next().unwrap_or("");
    let method = req.method().clone();
    let readable = matches!(
        first,
        "" | "messages"
            | "attachments"
            | "reactions"
            | "members"
            | "mode"
            | "settings"
            | "search"
            | "notes"
            | "tasks"
            | "goals"
            | "journal"
            | "wiki"
            | "attentions"
            | "waits"
            | "extensions"
    );
    let conversation = matches!(first, "messages" | "attachments");
    let allowed = if method == axum::http::Method::GET || method == axum::http::Method::HEAD {
        readable
    } else {
        conversation
    };
    if !allowed {
        return (StatusCode::FORBIDDEN, "a birebir is conversation only").into_response();
    }
    next.run(req).await
}
