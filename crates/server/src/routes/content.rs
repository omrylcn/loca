use super::access::mutation_actor;
use crate::*;

/// PRINCIPLES "Wiki editor": "there is no separate Memory role or second owner
/// assignment. Old records are archives, not new authority." The memory
/// routes stay readable; the old write and owner routes refuse everyone, the
/// owner an archived record still names included.
const ARCHIVED_MEMORY: &str =
    "loca memory is an archive and read-only; Loca Wiki is the shared knowledge surface";

pub(crate) async fn caretaker_memory(
    State(hub): State<Hub>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let actor = session_of(&headers)
        .and_then(|token| hub.session_identity(Some(token)))
        .map(|identity| identity.name)
        .or_else(|| {
            let token = member_token_of(&headers)?;
            hub.member_for_credential(Some(token))
                .map(|member| member.name)
                .or_else(|| hub.invite_by_token(token).map(|invite| invite.name))
        });
    if actor.as_deref().is_none_or(|name| !hub.is_caretaker(name)) {
        return (StatusCode::FORBIDDEN, "configured caretaker only").into_response();
    }
    // A group loca the caretaker is not a member of is never named to it.
    let hidden = hub.group_locas_hidden_from(&caretaker_viewer(&hub, &headers));
    match hub.loca_memory_metadata() {
        Ok(rows) => Json(
            rows.into_iter()
                .filter(|row| !hidden.contains(&row.loca))
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

pub(crate) async fn set_loca_memory_owner(_access: RoomAccess) -> impl IntoResponse {
    (
        StatusCode::FORBIDDEN,
        "there is no Memory owner to assign: loca memory is an archive and read-only; \
         Loca Wiki is the shared knowledge surface",
    )
}

pub(crate) async fn get_loca_memory(
    State(hub): State<Hub>,
    access: RoomAccess,
) -> impl IntoResponse {
    if !hub.memory_persistence_available() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "loca memory requires persistent storage in this deployment",
        )
            .into_response();
    }
    match hub.loca_memory(&access.room) {
        Ok(Some(memory)) => Json(memory).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            "memory is not configured for this loca",
        )
            .into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

pub(crate) async fn get_loca_memory_snapshot(
    State(hub): State<Hub>,
    access: RoomAccess,
) -> impl IntoResponse {
    match hub.loca_memory_snapshot(&access.room) {
        Ok(snapshot) => Json(ServerFrame::memory(&access.room, snapshot)).into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

pub(crate) async fn write_short_memory(_access: RoomAccess) -> impl IntoResponse {
    (StatusCode::FORBIDDEN, ARCHIVED_MEMORY)
}

pub(crate) async fn append_long_memory(_access: RoomAccess) -> impl IntoResponse {
    (StatusCode::FORBIDDEN, ARCHIVED_MEMORY)
}

pub(crate) async fn list_long_memory_entries(
    State(hub): State<Hub>,
    access: RoomAccess,
    Query(q): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if !hub.memory_persistence_available() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "loca memory requires persistent storage in this deployment",
        )
            .into_response();
    }
    let after_id = q.get("after_id").and_then(|v| v.parse().ok()).unwrap_or(0);
    let limit = q
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(50)
        .clamp(1, 200);
    match hub.loca_memory_entries(&access.room, after_id, limit) {
        Ok(page) => Json(page).into_response(),
        Err(crate::store::MemoryReadError::PersistenceUnavailable) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "loca memory requires persistent storage in this deployment",
        )
            .into_response(),
        Err(crate::store::MemoryReadError::NotConfigured) => (
            StatusCode::NOT_FOUND,
            "memory is not configured for this loca",
        )
            .into_response(),
        Err(crate::store::MemoryReadError::InvariantViolation) => (
            StatusCode::CONFLICT,
            "loca memory provenance is inconsistent with long memory",
        )
            .into_response(),
        Err(crate::store::MemoryReadError::Storage) => {
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
    }
}

/// The loca's journal — what has already been done here.
pub(crate) async fn get_journal(State(hub): State<Hub>, access: RoomAccess) -> impl IntoResponse {
    Json(hub.journal(&access.room)).into_response()
}
/// Record a piece of finished work. Unlike a task, nobody declares this for
/// you — you write it because you did it, and it is never edited afterwards.
pub(crate) async fn post_journal(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    Json(body): Json<protocol::CreateJournalEntry>,
) -> impl IntoResponse {
    let id = access.room;
    if !hub.is_writable(&id) {
        return (StatusCode::CONFLICT, "this loca is closed — read-only").into_response();
    }
    if body.text.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "an entry needs words").into_response();
    }
    let (by, by_type) = match mutation_actor(
        &hub,
        &headers,
        body.by.clone().unwrap_or_else(|| "anon".into()),
        body.by_type.unwrap_or(SenderType::Agent),
    ) {
        Ok(actor) => actor,
        Err(status) => return status.into_response(),
    };
    match hub.append_journal(&id, by, by_type, body.text.trim().to_string()) {
        Ok(entry) => (StatusCode::CREATED, Json(entry)).into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save journal entry — try again",
        )
            .into_response(),
    }
}
pub(crate) async fn post_message(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    Json(mut body): Json<PostMessage>,
) -> impl IntoResponse {
    let id = access.room;
    // Empty text is fine when the message carries attachments (an image with no
    // caption is a real message); it's only empty when BOTH are empty.
    if body.text.trim().is_empty() && body.attachments.is_empty() {
        return (StatusCode::BAD_REQUEST, "empty text").into_response();
    }
    if body.op_id.as_ref().is_some_and(|id| id.len() > 128) {
        return (StatusCode::BAD_REQUEST, "op_id is too long").into_response();
    }
    // Session-derived identity: with a valid X-Session-Token the body's
    // sender/sender_type are overridden by what the token is bound to, so
    // identity can't be spoofed. With REQUIRE_SESSIONS a token is mandatory.
    let sess = headers.get(SESSION_HEADER).and_then(|v| v.to_str().ok());
    let session_identity = hub.session_identity(sess);
    match session_identity.as_ref() {
        Some(idy) => {
            body.sender = idy.name.clone();
            body.sender_type = idy.kind;
        }
        None if sess.is_some() => {
            return (StatusCode::UNAUTHORIZED, "invalid session token").into_response();
        }
        None if hub.require_sessions() => {
            return (
                StatusCode::UNAUTHORIZED,
                "session token required (POST /sessions)",
            )
                .into_response();
        }
        None => {}
    }
    // The same key the WebSocket `send` frame uses for this sender.
    let scope = hub.operation_scope(
        &id,
        admin_token_of(&headers),
        member_token_of(&headers),
        session_identity.as_ref(),
        body.sender_type,
        &body.sender,
    );
    // A post carrying a valid admin token bypasses mode gating. In dev (no
    // ADMIN_TOKEN configured) everyone is the operator, so requiring a header
    // there would mean nobody could name a lead or speak past a mode gate on a
    // local server.
    let is_admin = if hub.admin_open() {
        true
    } else {
        is_admin_req(&hub, &headers)
    };
    // In the Lobby room only a rank calls everyone.
    hub.settle_lobby_call(&id, &mut body, is_admin);
    // Who wrote it, as verified by the session; a rename follows this id.
    // Without a session the body's sender is unverified, so nobody is recorded.
    let sender_principal = session_identity
        .as_ref()
        .and_then(|idy| idy.principal_id.clone());
    match hub.post(&id, body, is_admin, &scope, sender_principal.as_deref()) {
        Ok(msg) => (StatusCode::CREATED, Json(msg)).into_response(),
        Err(reject) => {
            let code = if reject.is_rate_limit() {
                StatusCode::TOO_MANY_REQUESTS
            } else if matches!(reject, hub::PostReject::Storage) {
                // Not the caller's fault — the write failed. 503 so a client
                // may retry rather than treat it as a permanent rejection.
                StatusCode::SERVICE_UNAVAILABLE
            } else if matches!(reject, hub::PostReject::OperationUnclear) {
                // Neither a replay the server can prove nor a new operation
                // it may post: the client must choose a new `op_id`.
                StatusCode::CONFLICT
            } else if reject.is_bad_request() {
                // Malformed caller input (bad/too-many attachment ids) — 400,
                // distinct from the 403 the mode/permission gates return.
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::FORBIDDEN
            };
            (code, reject.message()).into_response()
        }
    }
}

pub(crate) async fn get_reactions(State(hub): State<Hub>, access: RoomAccess) -> impl IntoResponse {
    Json(hub.reactions(&access.room)).into_response()
}

pub(crate) async fn set_reaction(
    State(hub): State<Hub>,
    access: RoomAccess,
    Path((_id, message_id)): Path<(String, u64)>,
    headers: HeaderMap,
    Json(mut body): Json<protocol::SetMessageReaction>,
) -> impl IntoResponse {
    let session = headers.get(SESSION_HEADER).and_then(|v| v.to_str().ok());
    let identity = hub.session_identity(session);
    match identity.as_ref() {
        Some(idy) => {
            body.reactor = idy.name.clone();
            body.reactor_type = Some(idy.kind);
        }
        None if session.is_some() => {
            return (StatusCode::UNAUTHORIZED, "invalid session token").into_response()
        }
        None if hub.require_sessions() => {
            return (StatusCode::UNAUTHORIZED, "session token required").into_response()
        }
        None => {}
    }
    if body.reactor.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "reactor required").into_response();
    }
    // A session's membership is its id (`mb:<mbid_>`), never its key.
    let principal = identity
        .as_ref()
        .and_then(|idy| idy.member.as_ref().map(|m| format!("mb:{m}")))
        .unwrap_or_else(|| format!("reactor:{}", body.reactor));
    match hub.set_reaction(
        &access.room,
        message_id,
        &principal,
        &body.reactor,
        &body.emoji,
        body.active,
    ) {
        Ok(event) => Json(event).into_response(),
        Err(hub::ReactionReject::InvalidEmoji) => {
            (StatusCode::BAD_REQUEST, "unsupported reaction").into_response()
        }
        Err(hub::ReactionReject::NotFound) => {
            (StatusCode::NOT_FOUND, "message not found").into_response()
        }
        Err(hub::ReactionReject::OwnMessage) => {
            (StatusCode::CONFLICT, "cannot react to your own message").into_response()
        }
        Err(hub::ReactionReject::ReadOnly) => {
            (StatusCode::CONFLICT, "this loca is closed — read-only").into_response()
        }
        Err(hub::ReactionReject::Storage) => {
            (StatusCode::SERVICE_UNAVAILABLE, "could not save reaction").into_response()
        }
    }
}
// ---- notes ----

pub(crate) async fn get_notes(State(hub): State<Hub>, access: RoomAccess) -> impl IntoResponse {
    Json(hub.notes(&access.room)).into_response()
}
pub(crate) async fn get_note(
    State(hub): State<Hub>,
    access: RoomAccess,
    Path((_id, key)): Path<(String, String)>,
) -> impl IntoResponse {
    match hub.note(&access.room, &key) {
        Some(note) => Json(note).into_response(),
        None => (StatusCode::NOT_FOUND, "no such note").into_response(),
    }
}
pub(crate) async fn create_note(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    Json(mut body): Json<CreateNote>,
) -> impl IntoResponse {
    let id = access.room;
    if !hub.is_writable(&id) {
        return (StatusCode::CONFLICT, "this loca is closed — read-only").into_response();
    }
    if body.key.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "empty key").into_response();
    }
    body.by = match actor_of(&hub, &headers, &body.by) {
        Ok(actor) => actor,
        Err(status) => return status.into_response(),
    };
    match hub.create_note(&id, body) {
        Ok(note) => (StatusCode::CREATED, Json(note)).into_response(),
        Err(NoteError::Exists) => {
            (StatusCode::CONFLICT, "note exists — use PUT to update").into_response()
        }
        Err(NoteError::NotFound) => (StatusCode::NOT_FOUND, "no such room").into_response(),
        Err(NoteError::Storage) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save note — try again",
        )
            .into_response(),
    }
}
pub(crate) async fn update_note(
    State(hub): State<Hub>,
    access: RoomAccess,
    Path((_id, key)): Path<(String, String)>,
    headers: HeaderMap,
    Json(mut body): Json<UpdateNote>,
) -> impl IntoResponse {
    let id = access.room;
    if !hub.is_writable(&id) {
        return (StatusCode::CONFLICT, "this loca is closed — read-only").into_response();
    }
    // Operator authority (may reassign `can_write`) comes from the admin
    // token OR a live admin session, never from the request body. Open when no
    // ADMIN_TOKEN is set.
    let is_operator = is_admin_req(&hub, &headers);
    body.by = match actor_of(&hub, &headers, &body.by) {
        Ok(actor) => actor,
        Err(status) => return status.into_response(),
    };
    match hub.update_note(&id, &key, body, is_operator) {
        Ok(note) => (StatusCode::OK, Json(note)).into_response(),
        Err(NoteError::NotFound) => {
            (StatusCode::NOT_FOUND, "no such note — use POST to create").into_response()
        }
        Err(NoteError::Exists) => (StatusCode::CONFLICT, "conflict").into_response(),
        Err(NoteError::Storage) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save note — try again",
        )
            .into_response(),
    }
}
pub(crate) async fn delete_note(
    State(hub): State<Hub>,
    access: RoomAccess,
    Path((_id, key)): Path<(String, String)>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(status) = mutation_actor(&hub, &headers, String::new(), SenderType::User) {
        return status;
    }
    let id = access.room;
    if !hub.is_writable(&id) {
        return StatusCode::CONFLICT;
    }
    match hub.delete_note(&id, &key) {
        Ok(true) => StatusCode::NO_CONTENT,
        Ok(false) => StatusCode::NOT_FOUND,
        Err(_) => StatusCode::SERVICE_UNAVAILABLE,
    }
}
/// Past versions of a note (newest first) — the room's memory of a fact.
pub(crate) async fn note_history(
    State(hub): State<Hub>,
    access: RoomAccess,
    Path((_id, key)): Path<(String, String)>,
) -> impl IntoResponse {
    match hub.note_history(&access.room, &key) {
        Ok(history) => Json(history).into_response(),
        Err(error) => {
            tracing::error!(%error, "note history read failed");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
    }
}
/// Search the room's memory: full message archive + current notes.
/// GET /rooms/{id}/search?q=needle[&limit=50]
pub(crate) async fn search_room(
    State(hub): State<Hub>,
    access: RoomAccess,
    Query(q): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let id = access.room;
    let needle = q.get("q").cloned().unwrap_or_default();
    if needle.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "missing q").into_response();
    }
    let limit = q
        .get("limit")
        .and_then(|l| l.parse().ok())
        .unwrap_or(50)
        .min(200);
    match hub.search(&id, &needle, limit) {
        Ok((messages, notes)) => {
            // Search is a door too: a switched-off Notes extension returns no
            // notes here, exactly as its own routes refuse them.
            let notes = if hub.extension_enabled(&id, "notes") {
                notes
            } else {
                Vec::new()
            };
            Json(serde_json::json!({ "messages": messages, "notes": notes })).into_response()
        }
        Err(error) => {
            tracing::error!(%error, "room search read failed");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
    }
}
