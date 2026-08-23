use crate::*;

pub(crate) async fn report_runtime_health(
    State(hub): State<Hub>,
    headers: HeaderMap,
    Json(body): Json<protocol::RuntimeHealthUpdate>,
) -> impl IntoResponse {
    let Some(token) = member_token_of(&headers) else {
        return (StatusCode::UNAUTHORIZED, "membership required").into_response();
    };
    let Some(member) = hub.member_for_credential(Some(token)) else {
        return (StatusCode::UNAUTHORIZED, "valid membership required").into_response();
    };
    match hub.report_runtime_health(&member.name, body) {
        Some(health) => Json(health).into_response(),
        None => (StatusCode::BAD_REQUEST, "invalid runtime health").into_response(),
    }
}
// ---- attention: durable focus, distinct from task and delivery ACK ----

pub(crate) async fn list_attentions(
    State(hub): State<Hub>,
    access: RoomAccess,
) -> impl IntoResponse {
    Json(hub.attentions(&access.room))
}
pub(crate) async fn create_attention(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    Json(mut body): Json<CreateAttention>,
) -> impl IntoResponse {
    let id = access.room;
    if !hub.is_writable(&id) {
        return (StatusCode::CONFLICT, "this loca is closed — read-only").into_response();
    }
    body.subject = body.subject.trim().to_string();
    if body.subject.is_empty() {
        return (StatusCode::BAD_REQUEST, "empty attention subject").into_response();
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
            "creating attention takes operator authority in this loca",
        )
            .into_response();
    }
    body.by = by;
    match hub.create_attention(&id, body) {
        Ok(attention) => (StatusCode::CREATED, Json(attention)).into_response(),
        Err(AttentionError::NoRecipient) => (
            StatusCode::CONFLICT,
            "attention has no recipient — name a lead or choose a person/group",
        )
            .into_response(),
        Err(AttentionError::NotFound) => (StatusCode::NOT_FOUND, "no such loca").into_response(),
        Err(AttentionError::Storage) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save attention — try again",
        )
            .into_response(),
        Err(AttentionError::Forbidden | AttentionError::Conflict) => unreachable!(),
    }
}
pub(crate) async fn claim_attention(
    State(hub): State<Hub>,
    Path((id, attention_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<AttentionAction>,
) -> impl IntoResponse {
    // Attention ownership deliberately crosses the privacy boundary for the
    // selected caretaker: loca-care receives a bounded envelope in İye and
    // must be able to claim it without gaining read access to the source loca.
    // The Hub still enforces exact owner identity; no source data is returned.
    if hub.is_deleted(&id) {
        return (StatusCode::NOT_FOUND, "this loca no longer exists").into_response();
    }
    let actor = match actor_of(&hub, &headers, &body.by) {
        Ok(actor) => actor,
        Err(code) => return (code, "invalid or missing session").into_response(),
    };
    match room_decision_of(&hub, &id, &headers) {
        hub::EnterDecision::Allowed => {}
        hub::EnterDecision::Banned => {
            return (StatusCode::FORBIDDEN, "banned from this loca").into_response();
        }
        hub::EnterDecision::Denied if hub.caretaker_owns_attention(&id, &attention_id, &actor) => {}
        hub::EnterDecision::Denied => {
            return (StatusCode::UNAUTHORIZED, "davet required for this loca").into_response();
        }
    }
    if !hub.is_writable(&id) {
        return (StatusCode::CONFLICT, "this loca is closed — read-only").into_response();
    }
    attention_response(hub.claim_attention(&id, &attention_id, &actor))
}
pub(crate) async fn resolve_attention(
    State(hub): State<Hub>,
    Path((id, attention_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<AttentionAction>,
) -> impl IntoResponse {
    if hub.is_deleted(&id) {
        return (StatusCode::NOT_FOUND, "this loca no longer exists").into_response();
    }
    let actor = match actor_of(&hub, &headers, &body.by) {
        Ok(actor) => actor,
        Err(code) => return (code, "invalid or missing session").into_response(),
    };
    let source_access = match room_decision_of(&hub, &id, &headers) {
        hub::EnterDecision::Allowed => true,
        hub::EnterDecision::Banned => {
            return (StatusCode::FORBIDDEN, "banned from this loca").into_response();
        }
        hub::EnterDecision::Denied if hub.caretaker_owns_attention(&id, &attention_id, &actor) => {
            false
        }
        hub::EnterDecision::Denied => {
            return (StatusCode::UNAUTHORIZED, "davet required for this loca").into_response();
        }
    };
    if !hub.is_writable(&id) {
        return (StatusCode::CONFLICT, "this loca is closed — read-only").into_response();
    }
    let is_operator = source_access
        && (is_admin_req(&hub, &headers)
            || hub.is_loca_operator(&id, admin_token_of(&headers), session_of(&headers), &actor));
    attention_response(hub.resolve_attention(&id, &attention_id, &actor, is_operator))
}
pub(crate) fn attention_response(
    result: Result<protocol::Attention, AttentionError>,
) -> axum::response::Response {
    match result {
        Ok(attention) => (StatusCode::OK, Json(attention)).into_response(),
        Err(AttentionError::NotFound) => {
            (StatusCode::NOT_FOUND, "no such attention").into_response()
        }
        Err(AttentionError::Forbidden) => (
            StatusCode::FORBIDDEN,
            "only the selected owner may claim; owner or operator may resolve",
        )
            .into_response(),
        Err(AttentionError::Conflict) => (
            StatusCode::CONFLICT,
            "attention is already claimed or resolved",
        )
            .into_response(),
        Err(AttentionError::Storage) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not update attention — try again",
        )
            .into_response(),
        Err(AttentionError::NoRecipient) => unreachable!(),
    }
}
pub(crate) async fn ack_care(
    State(hub): State<Hub>,
    _access: RoomAccess,
    Path((_id, signal_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<CareAck>,
) -> impl IntoResponse {
    let actor = match actor_of(&hub, &headers, &body.by) {
        Ok(actor) => actor,
        Err(code) => return (code, "invalid or missing session").into_response(),
    };
    match hub.ack_care(&signal_id, &actor) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            "no pending care signal for this identity",
        )
            .into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not acknowledge care signal — reconnect to retry",
        )
            .into_response(),
    }
}
