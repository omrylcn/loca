//! A person's own profile: the profile view and card, reading preferences,
//! and the profile's own credentials.

use crate::*;

#[derive(serde::Deserialize)]
pub(crate) struct CreateProfileCredential {
    label: String,
}

#[derive(serde::Deserialize)]
pub(crate) struct ProfileQuery {
    room: Option<String>,
}

pub(super) fn request_principal(hub: &Hub, headers: &HeaderMap) -> Option<String> {
    hub.resolve_authority(admin_token_of(headers), session_of(headers))
        .principal_id
}

pub(crate) async fn profile_view(
    State(hub): State<Hub>,
    Query(query): Query<ProfileQuery>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let mut authority = hub.resolve_authority(admin_token_of(&headers), session_of(&headers));
    let session = hub.session_identity(session_of(&headers));
    let scoped = session.as_ref().and_then(|s| s.loca.as_deref());
    // A room-only session may identify itself *in that room* for pins,
    // sidebar preferences and editor attribution, never as a Building login.
    if let Some(scope) = scoped {
        if query.room.as_deref() != Some(scope)
            || !room_decision_of(&hub, scope, &headers).is_allowed()
        {
            return (StatusCode::UNAUTHORIZED, "profile is scoped to this loca").into_response();
        }
        authority.principal_id = session.as_ref().and_then(|s| s.principal_id.clone());
    }
    let Some(principal_id) = authority.principal_id.as_deref() else {
        return (StatusCode::UNAUTHORIZED, "verified profile required").into_response();
    };
    let Some(display_name) = authority.display_name.as_deref() else {
        return (StatusCode::UNAUTHORIZED, "active profile required").into_response();
    };
    let room = query.room.as_deref().filter(|room| !room.is_empty());
    let mut loca_roles = Vec::new();
    let mut operator_source: Option<&str> = None;
    if let Some(room) = room {
        if hub.is_operator_principal(room, principal_id) {
            loca_roles.push("operator");
            operator_source = Some("appointed");
        } else if authority.is_master() && !hub.is_dm(room) {
            // The Master stands at the top of the hierarchy: in every loca it
            // holds the operator's authority by rank, without an appointment.
            // (A direct message has no operator; there the Master is one of the two.)
            loca_roles.push("operator");
            operator_source = Some("master");
        }
        if hub.is_lead(room, display_name) {
            loca_roles.push("lead");
        }
        loca_roles.push("participant");
    }
    let current_credential_id =
        hub.credential_id_for_request(admin_token_of(&headers), session_of(&headers));
    let mut principal = serde_json::json!({
        "id": principal_id,
        "display_name": display_name,
        "kind": authority.kind,
    });
    hub.profile_card(principal_id).decorate(&mut principal);
    Json(serde_json::json!({
        "principal": principal,
        "building_role": authority.building_role,
        "groups": if scoped.is_some() { Vec::new() } else { hub.groups_of_principal(principal_id) },
        "loca": room.map(|room| serde_json::json!({
            "room": room,
            "roles": loca_roles,
            "operator_source": operator_source,
            "building_management": authority.is_building_admin(),
        })),
        "session": session.map(|identity| serde_json::json!({
            "expires_at": identity.expires_at,
            "credential_id": current_credential_id,
            "bounded": true,
        })),
    }))
    .into_response()
}

/// The caller's own appearance preferences. There is deliberately no way to
/// name another principal: the row is always the authenticated caller's.
pub(crate) async fn get_profile_preferences(
    State(hub): State<Hub>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(principal_id) = request_principal(&hub, &headers) else {
        return (StatusCode::UNAUTHORIZED, "verified profile required").into_response();
    };
    Json(hub.preferences_for(&principal_id)).into_response()
}

/// `PUT /profile/card {"full_name", "icon", "avatar_color", "avatar_shape"}` —
/// the caller's own full name, profile icon and avatar colour and shape. Display only: the `@name` and everything it decides stay as
/// they are. Fields left out keep their value; `null` or `""` clears one; an
/// unknown field or a refused value rejects the whole body with 400. There is
/// no way to name another principal.
pub(crate) async fn put_profile_card(
    State(hub): State<Hub>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let Some(principal_id) = request_principal(&hub, &headers) else {
        return (StatusCode::UNAUTHORIZED, "verified profile required").into_response();
    };
    let Ok(patch) = serde_json::from_slice::<serde_json::Value>(&body) else {
        return (StatusCode::BAD_REQUEST, "the body must be a JSON object").into_response();
    };
    match hub.update_profile_card(&principal_id, &patch) {
        Ok(card) => {
            // `full_name` and `icon` are always there (null when unset), as
            // before; the avatar's colour and shape only when set.
            let mut saved = serde_json::json!({
                "full_name": card.full_name,
                "icon": card.icon,
            });
            if let Some(fields) = saved.as_object_mut() {
                if let Some(color) = card.avatar_color {
                    fields.insert("avatar_color".into(), color.into());
                }
                if let Some(shape) = card.avatar_shape {
                    fields.insert("avatar_shape".into(), shape.into());
                }
            }
            Json(saved).into_response()
        }
        Err(crate::store::ProfileCardError::Invalid(reason)) => {
            (StatusCode::BAD_REQUEST, reason).into_response()
        }
        Err(crate::store::ProfileCardError::NoProfile) => {
            (StatusCode::UNAUTHORIZED, "verified profile required").into_response()
        }
        Err(crate::store::ProfileCardError::Storage) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save your full name and icon — try again",
        )
            .into_response(),
    }
}

/// Add each person's principal id, full name and icon (when set) to rows
/// that name them by `@name` under `name_key`, so a client keys a card by the
/// principal, never by the label. A row whose name no single active
/// principal wears is left exactly as it was.
pub(super) fn with_people<T: serde::Serialize>(
    rows: &[T],
    name_key: &str,
    people: &HashMap<String, (String, crate::store::ProfileCard)>,
) -> serde_json::Value {
    let rows: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            let mut value = serde_json::to_value(row).unwrap_or(serde_json::Value::Null);
            let person = value
                .get(name_key)
                .and_then(|name| name.as_str())
                .and_then(|name| people.get(name))
                .cloned();
            if let (Some((id, card)), Some(fields)) = (person, value.as_object_mut()) {
                fields
                    .entry("principal_id")
                    .or_insert_with(|| serde_json::Value::String(id));
                card.decorate(&mut value);
            }
            value
        })
        .collect();
    serde_json::Value::Array(rows)
}

/// `/profiles` rows already carry the principal's `id`: add its card.
pub(super) fn with_cards_by_id<T: serde::Serialize>(
    rows: &[T],
    cards: &HashMap<String, crate::store::ProfileCard>,
) -> serde_json::Value {
    let rows: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            let mut value = serde_json::to_value(row).unwrap_or(serde_json::Value::Null);
            let card = value
                .get("id")
                .and_then(|id| id.as_str())
                .and_then(|id| cards.get(id))
                .cloned();
            if let Some(card) = card {
                card.decorate(&mut value);
            }
            value
        })
        .collect();
    serde_json::Value::Array(rows)
}

/// Partial update: fields left out keep their value; any unknown field or
/// value outside the allowed set rejects the whole body with 400.
pub(crate) async fn put_profile_preferences(
    State(hub): State<Hub>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let Some(principal_id) = request_principal(&hub, &headers) else {
        return (StatusCode::UNAUTHORIZED, "verified profile required").into_response();
    };
    let Ok(patch) = serde_json::from_slice::<serde_json::Value>(&body) else {
        return (StatusCode::BAD_REQUEST, "preferences must be a JSON object").into_response();
    };
    match hub.update_preferences(&principal_id, &patch) {
        Ok(preferences) => Json(preferences).into_response(),
        Err(crate::store::PreferenceError::Invalid(reason)) => {
            (StatusCode::BAD_REQUEST, reason).into_response()
        }
        Err(crate::store::PreferenceError::Storage) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save preferences — try again",
        )
            .into_response(),
    }
}

pub(crate) async fn list_profile_credentials(
    State(hub): State<Hub>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(principal_id) = request_principal(&hub, &headers) else {
        return (StatusCode::UNAUTHORIZED, "verified profile required").into_response();
    };
    let current = hub.credential_id_for_request(admin_token_of(&headers), session_of(&headers));
    let credentials: Vec<_> = hub
        .credentials_for(&principal_id)
        .into_iter()
        .map(|credential| {
            let is_current = current.as_deref() == Some(credential.id.as_str());
            serde_json::json!({
                "id": credential.id,
                "label": credential.label,
                "created_at": credential.created_at,
                "last_used_at": credential.last_used_at,
                "revoked_at": credential.revoked_at,
                "root_recovery": credential.root_recovery,
                "current": is_current,
            })
        })
        .collect();
    Json(credentials).into_response()
}

pub(crate) async fn create_profile_credential_route(
    State(hub): State<Hub>,
    headers: HeaderMap,
    Json(body): Json<CreateProfileCredential>,
) -> impl IntoResponse {
    let Some(principal_id) = request_principal(&hub, &headers) else {
        return (StatusCode::UNAUTHORIZED, "verified profile required").into_response();
    };
    let label = body.label.trim();
    if label.is_empty() || label.len() > 64 || label.chars().any(char::is_control) {
        return (
            StatusCode::BAD_REQUEST,
            "credential label must be 1-64 visible characters",
        )
            .into_response();
    }
    match hub.create_profile_credential(&principal_id, label) {
        Ok((credential, secret)) => (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "credential": credential,
                "secret": secret,
            })),
        )
            .into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not create credential — try again",
        )
            .into_response(),
    }
}

pub(crate) async fn revoke_profile_credential_route(
    State(hub): State<Hub>,
    Path(credential_id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(principal_id) = request_principal(&hub, &headers) else {
        return (StatusCode::UNAUTHORIZED, "verified profile required").into_response();
    };
    match hub.revoke_profile_credential(&principal_id, &credential_id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(crate::store::CredentialError::NotFound) => {
            (StatusCode::NOT_FOUND, "no such credential").into_response()
        }
        Err(crate::store::CredentialError::RootRecovery) => (
            StatusCode::CONFLICT,
            "root recovery credential cannot be revoked here",
        )
            .into_response(),
        Err(crate::store::CredentialError::Storage) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not revoke credential — try again",
        )
            .into_response(),
    }
}
