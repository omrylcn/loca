//! Mentat rooms over HTTP (`hub/mentat.rs`). A mentat room is a kind of loca,
//! not an extension: it is founded as one (`POST /rooms` with `mentat`), and
//! every `/rooms/:id/mentat` route answers 404 in a loca that is not one.
//!
//! - `GET /mentats` — the Building's directory of mentat rooms: room, topic,
//!   mentat, whether it is connected, how long the line is. Every Building
//!   member; nothing of what is said inside.
//! - `GET /rooms/:id/mentat` — the line: the mentat, the limits, whether the
//!   line is paused, the consultation inside and the requests waiting, and
//!   what the caller may do. Anyone who may enter.
//! - `GET /rooms/:id/mentat/history` — requests that are over, newest first,
//!   with how each ended and whether its asker said it was answered.
//! - `PUT /rooms/:id/mentat {"mentat", "topic", "limits"}` — appoint the
//!   mentat, set the topic or the limits; `DELETE` takes the appointment
//!   back. The Master, or the Smaster who founded the room.
//! - `POST /rooms/:id/mentat/requests {"topic"}` — ask to consult the
//!   mentat. Anyone known by name (a session or a rank).
//! - `POST .../requests/:rid/join` — go in with a waiting request's askers;
//!   `.../withdraw` — take one's place back; `.../merge {"into"}` — the
//!   mentat joins two waiting requests.
//! - `POST /rooms/:id/mentat/end {"answered"}` — end the consultation
//!   inside: its askers, the mentat, or a rank who manages the room.
//! - `POST /rooms/:id/mentat/pause`, `.../resume` — the mentat, or a rank
//!   who manages the room, freezes or resumes the line.
use super::extensions::known_actor;
use crate::{
    admin_token_of, hub, member_token_of, session_of, HeaderMap, Hub, IntoResponse, Json, Path,
    RoomAccess, State, StatusCode,
};
use protocol::{
    ChatMode, EndMentatConsultation, MergeMentatRequest, OpenMentatRequest, SetMentatRoom,
};

pub(crate) fn mentat_refusal(error: hub::MentatError) -> axum::response::Response {
    use hub::MentatError as E;
    let status = match error {
        E::NotMentatRoom | E::NoSuchRequest | E::NoSuchAgent => StatusCode::NOT_FOUND,
        E::NotYours => StatusCode::FORBIDDEN,
        E::BadLimits
        | E::EmptyTopic
        | E::TopicTooShort
        | E::InvalidTopic
        | E::IsMentat
        | E::NotAnAgent
        | E::SameRequest => StatusCode::BAD_REQUEST,
        E::TopicTooLong => StatusCode::PAYLOAD_TOO_LARGE,
        E::NoMentat
        | E::AmbiguousName
        | E::MentatElsewhere
        | E::AlreadyInLine
        | E::NotWaiting
        | E::NoConsultation => StatusCode::CONFLICT,
        E::LineFull => StatusCode::TOO_MANY_REQUESTS,
        E::Storage => StatusCode::SERVICE_UNAVAILABLE,
    };
    (status, error.message()).into_response()
}

fn nameless() -> axum::response::Response {
    (
        StatusCode::UNAUTHORIZED,
        "the mentat line knows people by name: take a session first",
    )
        .into_response()
}

fn not_manager() -> axum::response::Response {
    (
        StatusCode::FORBIDDEN,
        "only the Master, or the Smaster who founded this mentat room, manages it",
    )
        .into_response()
}

/// Does the caller manage this mentat room? The Master, or the Smaster who
/// founded it. A mentat room has no Operator.
fn manages(hub: &Hub, room: &str, headers: &HeaderMap) -> bool {
    let actor = hub.resolve_authority(admin_token_of(headers), session_of(headers));
    hub.may_manage_mentat(room, &actor)
}

fn actor_name(hub: &Hub, headers: &HeaderMap) -> String {
    known_actor(hub, headers)
        .map(|(name, _)| name)
        .unwrap_or_else(|| "master".into())
}

/// `GET /mentats`.
pub(crate) async fn list_mentats(
    State(hub): State<Hub>,
    headers: HeaderMap,
) -> axum::response::Response {
    // The directory is for the Building's members: a Building identity (a
    // membership key, a profile, a rank). A session scoped to one loca does
    // not show the Building's experts; an agent reads the directory with its
    // membership, as it reads the Lobby.
    let authority = hub.resolve_authority(admin_token_of(&headers), session_of(&headers));
    let known = authority.principal_id.is_some()
        || authority.is_building_admin()
        || hub
            .member_for_credential(member_token_of(&headers))
            .is_some();
    if !known {
        return (
            StatusCode::FORBIDDEN,
            "the mentat directory is for the Building's members",
        )
            .into_response();
    }
    Json(serde_json::json!({ "mentats": hub.mentat_directory() })).into_response()
}

/// `GET /rooms/:id/mentat`.
pub(crate) async fn get_mentat(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
) -> axum::response::Response {
    match hub.mentat_state(&access.room) {
        Some(state) => {
            let me = known_actor(&hub, &headers).map(|(name, _)| name);
            let is_mentat = me
                .as_deref()
                .is_some_and(|name| state.mentat.mentat.as_deref() == Some(name));
            Json(serde_json::json!({
                "room": access.room,
                "state": state,
                "can_manage": manages(&hub, &access.room, &headers),
                "is_mentat": is_mentat,
            }))
            .into_response()
        }
        None => mentat_refusal(hub::MentatError::NotMentatRoom),
    }
}

/// `GET /rooms/:id/mentat/history`.
pub(crate) async fn get_mentat_history(
    State(hub): State<Hub>,
    access: RoomAccess,
) -> axum::response::Response {
    match hub.mentat_history(&access.room) {
        Ok(requests) => {
            Json(serde_json::json!({ "room": access.room, "requests": requests })).into_response()
        }
        Err(error) => mentat_refusal(error),
    }
}

/// `PUT /rooms/:id/mentat`.
pub(crate) async fn put_mentat(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    Json(asked): Json<SetMentatRoom>,
) -> axum::response::Response {
    if !hub.is_mentat_room(&access.room) {
        return mentat_refusal(hub::MentatError::NotMentatRoom);
    }
    if !manages(&hub, &access.room, &headers) {
        return not_manager();
    }
    match hub.set_mentat_room(&access.room, &asked, &actor_name(&hub, &headers)) {
        Ok(state) => Json(state).into_response(),
        Err(error) => mentat_refusal(error),
    }
}

/// `DELETE /rooms/:id/mentat`.
pub(crate) async fn delete_mentat(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
) -> axum::response::Response {
    if !hub.is_mentat_room(&access.room) {
        return mentat_refusal(hub::MentatError::NotMentatRoom);
    }
    if !manages(&hub, &access.room, &headers) {
        return not_manager();
    }
    match hub.clear_mentat(&access.room, &actor_name(&hub, &headers)) {
        Ok(state) => Json(state).into_response(),
        Err(error) => mentat_refusal(error),
    }
}

/// `POST /rooms/:id/mentat/requests`.
pub(crate) async fn open_mentat_request(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    Json(asked): Json<OpenMentatRequest>,
) -> axum::response::Response {
    let Some((asker, _)) = known_actor(&hub, &headers) else {
        return nameless();
    };
    match hub.open_mentat_request(&access.room, &asker, &asked.topic) {
        Ok(request) => (StatusCode::CREATED, Json(request)).into_response(),
        Err(error) => mentat_refusal(error),
    }
}

/// `POST /rooms/:id/mentat/requests/:rid/join`.
pub(crate) async fn join_mentat_request(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    Path((_room, rid)): Path<(String, u64)>,
) -> axum::response::Response {
    let Some((asker, _)) = known_actor(&hub, &headers) else {
        return nameless();
    };
    match hub.join_mentat_request(&access.room, &asker, rid) {
        Ok(request) => Json(request).into_response(),
        Err(error) => mentat_refusal(error),
    }
}

/// `POST /rooms/:id/mentat/requests/:rid/withdraw`.
pub(crate) async fn withdraw_mentat_request(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    Path((_room, rid)): Path<(String, u64)>,
) -> axum::response::Response {
    let Some((asker, _)) = known_actor(&hub, &headers) else {
        return nameless();
    };
    match hub.withdraw_mentat_request(&access.room, &asker, rid) {
        Ok(request) => Json(request).into_response(),
        Err(error) => mentat_refusal(error),
    }
}

/// `POST /rooms/:id/mentat/requests/:rid/merge`.
pub(crate) async fn merge_mentat_request(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    Path((_room, rid)): Path<(String, u64)>,
    Json(asked): Json<MergeMentatRequest>,
) -> axum::response::Response {
    let rank = manages(&hub, &access.room, &headers);
    let by = match known_actor(&hub, &headers) {
        Some((name, _)) => name,
        None if rank => "master".into(),
        None => return nameless(),
    };
    match hub.merge_mentat_requests(&access.room, rid, asked.into, &by, rank) {
        Ok(request) => Json(request).into_response(),
        Err(error) => mentat_refusal(error),
    }
}

/// `POST /rooms/:id/mentat/end`.
pub(crate) async fn end_mentat_consultation(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    body: Option<Json<EndMentatConsultation>>,
) -> axum::response::Response {
    let rank = manages(&hub, &access.room, &headers);
    let by = match known_actor(&hub, &headers) {
        Some((name, _)) => name,
        None if rank => "master".into(),
        None => return nameless(),
    };
    let answered = body.and_then(|Json(asked)| asked.answered);
    match hub.end_mentat_consultation(&access.room, &by, rank, answered) {
        Ok(request) => Json(request).into_response(),
        Err(error) => mentat_refusal(error),
    }
}

/// Freeze or resume the line: the mentat, or a rank who manages the room.
fn set_line(hub: &Hub, room: &str, headers: &HeaderMap, paused: bool) -> axum::response::Response {
    if !hub.is_mentat_room(room) {
        return mentat_refusal(hub::MentatError::NotMentatRoom);
    }
    let me = known_actor(hub, headers).map(|(name, _)| name);
    let is_mentat = me.as_deref().is_some_and(|name| hub.is_mentat(room, name));
    if !is_mentat && !manages(hub, room, headers) {
        return mentat_refusal(hub::MentatError::NotYours);
    }
    if !hub.is_writable(room) {
        return (StatusCode::CONFLICT, "this loca is closed — read-only").into_response();
    }
    // Any mode but a pause resumes the line (`Hub::mentat_mode_request`).
    let mode = if paused {
        ChatMode::Paused
    } else {
        ChatMode::Free
    };
    if hub.set_mode(room, mode).is_err() {
        return mentat_refusal(hub::MentatError::Storage);
    }
    match hub.mentat_state(room) {
        Some(state) => Json(state).into_response(),
        None => mentat_refusal(hub::MentatError::NotMentatRoom),
    }
}

/// `POST /rooms/:id/mentat/pause`.
pub(crate) async fn pause_mentat_line(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
) -> axum::response::Response {
    set_line(&hub, &access.room, &headers, true)
}

/// `POST /rooms/:id/mentat/resume`.
pub(crate) async fn resume_mentat_line(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
) -> axum::response::Response {
    set_line(&hub, &access.room, &headers, false)
}
