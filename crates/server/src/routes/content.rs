use crate::*;

fn memory_actor(hub: &Hub, headers: &HeaderMap) -> Result<String, Box<axum::response::Response>> {
    let Some(token) = session_of(headers) else {
        return Err(Box::new(
            (StatusCode::UNAUTHORIZED, "session token required").into_response(),
        ));
    };
    hub.session_identity(Some(token))
        .map(|identity| identity.name)
        .ok_or_else(|| {
            Box::new((StatusCode::UNAUTHORIZED, "invalid session token").into_response())
        })
}

fn memory_write_error(error: crate::store::MemoryWriteError) -> axum::response::Response {
    match error {
        crate::store::MemoryWriteError::PersistenceUnavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "loca memory requires persistent storage in this deployment",
        )
            .into_response(),
        crate::store::MemoryWriteError::OwnerUnassigned => (
            StatusCode::CONFLICT,
            "memory owner is not assigned for this loca",
        )
            .into_response(),
        crate::store::MemoryWriteError::NotOwner => {
            (StatusCode::FORBIDDEN, "only the memory owner may write").into_response()
        }
        crate::store::MemoryWriteError::ShortTooLarge => (
            StatusCode::PAYLOAD_TOO_LARGE,
            "short memory exceeds the 4 KB limit",
        )
            .into_response(),
        crate::store::MemoryWriteError::LongTooLarge => (
            StatusCode::PAYLOAD_TOO_LARGE,
            "long memory exceeds the 64 KB hard limit; consolidate it before adding more",
        )
            .into_response(),
        crate::store::MemoryWriteError::Storage => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save loca memory — try again",
        )
            .into_response(),
    }
}

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
    match hub.loca_memory_metadata() {
        Ok(rows) => Json(rows).into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

pub(crate) async fn set_loca_memory_owner(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    Json(body): Json<protocol::SetLocaMemoryOwner>,
) -> impl IntoResponse {
    if !is_master_req(&hub, &headers) {
        return (
            StatusCode::FORBIDDEN,
            "only the master may assign memory ownership",
        )
            .into_response();
    }
    let owner = body
        .owner
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty());
    match hub.set_memory_owner(&access.room, owner) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
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

pub(crate) async fn write_short_memory(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    Json(body): Json<protocol::WriteLocaMemory>,
) -> impl IntoResponse {
    if !hub.is_writable(&access.room) {
        return (StatusCode::CONFLICT, "this loca is closed — read-only").into_response();
    }
    if body.text.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "memory text is empty").into_response();
    }
    let actor = match memory_actor(&hub, &headers) {
        Ok(actor) => actor,
        Err(response) => return *response,
    };
    match hub.write_short_memory(&access.room, &actor, body.text.trim()) {
        Ok(memory) => Json(memory).into_response(),
        Err(error) => memory_write_error(error),
    }
}

pub(crate) async fn append_long_memory(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    Json(body): Json<protocol::WriteLocaMemory>,
) -> impl IntoResponse {
    if !hub.is_writable(&access.room) {
        return (StatusCode::CONFLICT, "this loca is closed — read-only").into_response();
    }
    if body.text.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "memory text is empty").into_response();
    }
    let actor = match memory_actor(&hub, &headers) {
        Ok(actor) => actor,
        Err(response) => return *response,
    };
    match hub.append_long_memory(&access.room, &actor, body.text.trim()) {
        Ok(entry) => (StatusCode::CREATED, Json(entry)).into_response(),
        Err(error) => memory_write_error(error),
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
    // Identity comes from the session when there is one, so a line cannot be
    // filed under someone else's name.
    let session = headers
        .get(SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|t| hub.session_identity(Some(t)));
    let (by, by_type) = match session {
        Some(idy) => (idy.name, idy.kind),
        None => (
            body.by.clone().unwrap_or_else(|| "anon".into()),
            body.by_type.unwrap_or(SenderType::Agent),
        ),
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
    let principal = session_identity
        .as_ref()
        .and_then(|idy| idy.member.as_ref().map(|member| format!("mb:{member}")))
        .unwrap_or_else(|| {
            let kind = match body.sender_type {
                SenderType::Agent => "agent",
                SenderType::User => "user",
            };
            format!("{kind}:{}", body.sender)
        });
    // A post carrying a valid admin token bypasses mode gating. In dev (no
    // ADMIN_TOKEN configured) everyone is the operator, so requiring a header
    // there would mean nobody could name a lead or speak past a mode gate on a
    // local server.
    let is_admin = if hub.admin_open() {
        true
    } else {
        is_admin_req(&hub, &headers)
    };
    match hub.post(&id, body, is_admin, &principal) {
        Ok(msg) => (StatusCode::CREATED, Json(msg)).into_response(),
        Err(reject) => {
            let code = if reject.is_rate_limit() {
                StatusCode::TOO_MANY_REQUESTS
            } else if matches!(reject, hub::PostReject::Storage) {
                // Not the caller's fault — the write failed. 503 so a client
                // may retry rather than treat it as a permanent rejection.
                StatusCode::SERVICE_UNAVAILABLE
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
) -> impl IntoResponse {
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
    Json(hub.note_history(&access.room, &key)).into_response()
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
    let (messages, notes) = hub.search(&id, &needle, limit);
    Json(serde_json::json!({ "messages": messages, "notes": notes })).into_response()
}
