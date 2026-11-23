//! The Vault over HTTP (PRINCIPLES "Vault"), routes of the `vault` extension;
//! the extension fence refuses all of them while it is off.
//!
//! - `GET /rooms/:id/vault` — the loca's rules, its revision, who wrote it
//!   and when, and whether the caller may write it. Anyone who may enter.
//! - `PUT /rooms/:id/vault {"body", "base_rev"}` — a new revision, written
//!   against the revision the writer read (409 when it is not the newest).
//!   Only the loca's appointed Operator, the Master and Smasters.
//! - `GET /rooms/:id/vault/history` — older revisions, newest first.
//! - `GET /rooms/:id/vault/lessons` — proposed and decided lessons, waiting
//!   ones first, and whether the caller may decide them.
//! - `POST /rooms/:id/vault/lessons {"text"}` — propose a lesson. Anyone
//!   seated who is known by name (a session or a rank).
//! - `POST /rooms/:id/vault/lessons/:lid/accept {"text"}` — write it into the
//!   Vault's Lessons section, in the words given; `.../dismiss` — set it
//!   aside. Only those who write the Vault.
use super::extensions::{journal_actor, may_manage};
use crate::{
    admin_token_of, hub, session_of, HeaderMap, Hub, IntoResponse, Json, Path, RoomAccess, State,
    StatusCode,
};
use protocol::{LessonSource, SenderType};

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

fn lesson_refusal(error: hub::LessonError) -> axum::response::Response {
    let status = match error {
        hub::LessonError::Empty | hub::LessonError::Invalid => StatusCode::BAD_REQUEST,
        hub::LessonError::TooLong | hub::LessonError::VaultFull => StatusCode::PAYLOAD_TOO_LARGE,
        hub::LessonError::Off => StatusCode::FORBIDDEN,
        hub::LessonError::NotFound => StatusCode::NOT_FOUND,
        hub::LessonError::Decided | hub::LessonError::NotWritable => StatusCode::CONFLICT,
        hub::LessonError::TooMany => StatusCode::TOO_MANY_REQUESTS,
        hub::LessonError::NoStorage | hub::LessonError::Storage => StatusCode::SERVICE_UNAVAILABLE,
    };
    (status, error.message()).into_response()
}

/// `GET /rooms/:id/vault/lessons`.
pub(crate) async fn get_lessons(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
) -> axum::response::Response {
    match hub.lessons(&access.room) {
        Ok(lessons) => Json(serde_json::json!({
            "room": access.room,
            "lessons": lessons,
            "can_decide": may_manage(&hub, &access.room, &headers),
        }))
        .into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not read the lessons — try again",
        )
            .into_response(),
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct LessonText {
    #[serde(default)]
    text: Option<String>,
}

/// Who is proposing, by a name the server verified: a session or a rank.
/// A request that names nobody proposes nothing.
fn known_proposer(hub: &Hub, headers: &HeaderMap) -> Option<(String, SenderType)> {
    let authority = hub.resolve_authority(admin_token_of(headers), session_of(headers));
    if let Some(name) = authority.display_name {
        return Some((name, authority.kind.unwrap_or(SenderType::User)));
    }
    hub.session_identity(session_of(headers))
        .map(|identity| (identity.name, identity.kind))
}

/// `POST /rooms/:id/vault/lessons`.
pub(crate) async fn post_lesson(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    Json(asked): Json<LessonText>,
) -> axum::response::Response {
    let Some((by, by_type)) = known_proposer(&hub, &headers) else {
        return (
            StatusCode::UNAUTHORIZED,
            "a lesson is proposed under a name: take a session first",
        )
            .into_response();
    };
    let source = match by_type {
        SenderType::Agent => LessonSource::Agent,
        SenderType::User => LessonSource::Person,
    };
    let text = asked.text.unwrap_or_default();
    match hub.propose_lesson(&access.room, &text, source, &by) {
        Ok(lesson) => (StatusCode::CREATED, Json(lesson)).into_response(),
        Err(error) => lesson_refusal(error),
    }
}

async fn decide(
    hub: Hub,
    room: &str,
    lid: u64,
    headers: &HeaderMap,
    accept_text: Option<&str>,
) -> axum::response::Response {
    if !may_manage(&hub, room, headers) {
        return (
            StatusCode::FORBIDDEN,
            "only this loca's operator, the Master or a Smaster decides its lessons",
        )
            .into_response();
    }
    let (by, by_type) = journal_actor(&hub, headers);
    let principal = hub
        .resolve_authority(admin_token_of(headers), session_of(headers))
        .principal_id;
    match hub.decide_lesson(room, lid, accept_text, &by, by_type, principal.as_deref()) {
        Ok(lesson) => Json(lesson).into_response(),
        Err(error) => lesson_refusal(error),
    }
}

/// `POST /rooms/:id/vault/lessons/:lid/accept`. Without `text` the lesson
/// is accepted in the words it was proposed in.
pub(crate) async fn accept_lesson(
    State(hub): State<Hub>,
    access: RoomAccess,
    Path((_id, lid)): Path<(String, u64)>,
    headers: HeaderMap,
    Json(asked): Json<LessonText>,
) -> axum::response::Response {
    let room = access.room;
    let text = match asked.text.filter(|t| !t.trim().is_empty()) {
        Some(text) => text,
        None => match hub.lessons(&room) {
            Ok(lessons) => match lessons.into_iter().find(|l| l.id == lid) {
                Some(lesson) => lesson.text,
                None => return lesson_refusal(hub::LessonError::NotFound),
            },
            Err(_) => return lesson_refusal(hub::LessonError::Storage),
        },
    };
    decide(hub, &room, lid, &headers, Some(&text)).await
}

/// `POST /rooms/:id/vault/lessons/:lid/dismiss`.
pub(crate) async fn dismiss_lesson(
    State(hub): State<Hub>,
    access: RoomAccess,
    Path((_id, lid)): Path<(String, u64)>,
    headers: HeaderMap,
) -> axum::response::Response {
    decide(hub, &access.room, lid, &headers, None).await
}
