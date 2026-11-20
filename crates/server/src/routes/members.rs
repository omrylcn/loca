//! The Building's people: members (admit, suspend, revoke), residents,
//! profiles, ranks and Smasters.

use crate::*;

use super::profile::{with_cards_by_id, with_people};

/// Open a session: bind a name/kind to a server-issued token. Requires
/// membership (room token) like any other join path.
/// Who belongs to the building.
pub(crate) async fn list_members(State(hub): State<Hub>, headers: HeaderMap) -> impl IntoResponse {
    if !is_admin_req(&hub, &headers) {
        return (StatusCode::UNAUTHORIZED, "master only").into_response();
    }
    let rows: Vec<_> = hub
        .memberships()
        .into_iter()
        .map(|member| {
            serde_json::json!({
                "id": member.id,
                "name": member.name,
                "kind": member.kind,
                "joined_at": member.joined_at,
                "admitted_by": member.admitted_by,
            })
        })
        .collect();
    Json(with_people(&rows, "name", &hub.people_by_name())).into_response()
}

pub(crate) async fn list_profiles(State(hub): State<Hub>, headers: HeaderMap) -> impl IntoResponse {
    if !is_admin_req(&hub, &headers) {
        return (StatusCode::FORBIDDEN, "Building authority required").into_response();
    }
    Json(with_cards_by_id(&hub.profiles(), &hub.profile_cards())).into_response()
}

/// Admit somebody to the building — the founding act, and the heavy one.
/// The authorized surface is a deployment choice; seating that member in a
/// loca (`/rooms/:id/call`) remains the lightweight everyday action.
pub(crate) async fn admit_member(
    State(hub): State<Hub>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if !is_admin_req(&hub, &headers) {
        return (StatusCode::UNAUTHORIZED, "master only").into_response();
    }
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if name.is_empty() {
        return (StatusCode::BAD_REQUEST, "a member needs a name").into_response();
    }
    // The join-request door's name rule applies here too, so a name admitted
    // directly can still be `@mention`ed. `Hub::admit_member` decides it, and
    // whether the name is taken, under the lock it writes the name with; a
    // member admitted before the rule is found again, not re-checked.
    let kind = body.get("kind").and_then(|v| v.as_str()).unwrap_or("agent");
    // kind is part of the identity, so it is validated at the door, not
    // silently coerced — a bad kind is a bad request, not a quiet "agent".
    if kind != "agent" && kind != "user" {
        return (StatusCode::BAD_REQUEST, "kind must be 'agent' or 'user'").into_response();
    }
    let by = super::actor::rank_actor(&hub, &headers);
    match hub.admit_member_once(name, kind, &by) {
        // A fresh member's key is handed out once, here, as before; the
        // server keeps only its hash.
        Ok((member, Some(key))) => (
            StatusCode::CREATED,
            Json(protocol::Membership {
                token: key,
                name: member.name,
                kind: member.kind,
                joined_at: member.joined_at,
                admitted_by: member.admitted_by,
            }),
        )
            .into_response(),
        // An existing member's key is not stored, so it cannot be read back
        // out: the answer names the member by its management id. The
        // member's own set-up keeps its key; a new set-up claims one of its
        // own with an invite.
        Ok((member, None)) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "id": member.id,
                "name": member.name,
                "kind": member.kind,
                "already": true,
            })),
        )
            .into_response(),
        Err(hub::AdmitError::Rule(rule)) => (StatusCode::BAD_REQUEST, rule).into_response(),
        Err(hub::AdmitError::Worn) => (StatusCode::CONFLICT, NAME_WORN_RULE).into_response(),
        Err(hub::AdmitError::Storage) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save member — try again",
        )
            .into_response(),
    }
}

/// End a membership.
pub(crate) async fn set_member_suspension_route(
    State(hub): State<Hub>,
    Path(reference): Path<String>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if !is_master_req(&hub, &headers) {
        return (StatusCode::FORBIDDEN, "Master authority required").into_response();
    }
    if !hub.member_suspension_supported() {
        return (
            StatusCode::CONFLICT,
            "Suspension requires a persistent Building",
        )
            .into_response();
    }
    let Some(suspended) = body.get("suspended").and_then(|value| value.as_bool()) else {
        return (StatusCode::BAD_REQUEST, "suspended must be a boolean").into_response();
    };
    let reason = body
        .get("reason")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim();
    if reason.len() > 500 {
        return (StatusCode::BAD_REQUEST, "reason is too long").into_response();
    }
    match hub.set_member_suspended(&reference, suspended, reason) {
        Ok(true) => (
            StatusCode::OK,
            Json(serde_json::json!({"suspended": suspended})),
        )
            .into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "membership or suspension not found").into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save membership state",
        )
            .into_response(),
    }
}

pub(crate) async fn revoke_member_route(
    State(hub): State<Hub>,
    Path(token): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !is_master_req(&hub, &headers) {
        return (StatusCode::FORBIDDEN, "Master authority required").into_response();
    }
    match hub.revoke_member_ref(&token) {
        Ok(true) => (StatusCode::OK, "revoked").into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such member").into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save revocation — try again",
        )
            .into_response(),
    }
}

/// Everyone the building knows, and where they sit. What the master picks from
/// when calling somebody into a loca.
pub(crate) async fn list_residents(
    State(hub): State<Hub>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !is_admin_req(&hub, &headers) {
        return (StatusCode::UNAUTHORIZED, "master only").into_response();
    }
    // A Smaster sees every person, but never a group loca it cannot enter
    // named as where someone sits.
    let viewer = hub.resolve_authority(admin_token_of(&headers), session_of(&headers));
    Json(with_people(
        &hub.residents_for(&viewer),
        "name",
        &hub.people_by_name(),
    ))
    .into_response()
}

/// The caretaker's least-privilege Building presence view.
///
/// A configured caretaker must be able to answer "who is connected?" even in
/// deployments that have no loca-dev runtime. Giving that runtime the master
/// key would also let it admit/revoke members and change every loca, which is
/// far broader than the job. Its own membership, invite, or live session is
/// therefore enough for this read-only endpoint, and the configured caretaker
/// name remains the authority boundary.
pub(crate) async fn caretaker_residents(
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
    let Some(actor) = actor else {
        return (StatusCode::UNAUTHORIZED, "caretaker identity required").into_response();
    };
    if !hub.is_caretaker(&actor) {
        return (StatusCode::FORBIDDEN, "configured caretaker only").into_response();
    }
    Json(hub.residents_for(&caretaker_viewer(&hub, &headers))).into_response()
}

/// Whose view a caretaker request reads with: its session's principal, or
/// its membership credential's. An invite proves no principal, so it sees no
/// group loca at all.
pub(crate) fn caretaker_viewer(hub: &Hub, headers: &HeaderMap) -> hub::RequestAuthority {
    let by_session = hub.resolve_authority(None, session_of(headers));
    if by_session.principal_id.is_some() {
        return by_session;
    }
    hub.resolve_authority(member_token_of(headers), None)
}

/// Who can act as a second master. Master-only: seeing the list means seeing
/// who holds authority.
pub(crate) async fn list_smasters(State(hub): State<Hub>, headers: HeaderMap) -> impl IntoResponse {
    if !is_master_req(&hub, &headers) {
        return (StatusCode::UNAUTHORIZED, "master only").into_response();
    }
    let rows: Vec<_> = hub
        .smasters()
        .into_iter()
        .map(|record| serde_json::json!({ "id": record.management_id, "name": record.name }))
        .collect();
    Json(rows).into_response()
}

fn name_error_response(error: hub::NameError) -> axum::response::Response {
    let message = match error {
        hub::NameError::Invalid => {
            "a name is 1-64 ASCII letters, digits, '-', '_' or '.', starting with a letter or digit, so it can be @mentioned"
        }
        hub::NameError::Reserved => {
            "that word is a title or keyword, not a name — use the person's own name"
        }
    };
    (StatusCode::BAD_REQUEST, message).into_response()
}

/// Building ranks — the Master and every Smaster — with where each sits now.
/// Ranks hold no Lobby membership, so this completes the Building directory.
pub(crate) async fn list_building_ranks(
    State(hub): State<Hub>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !is_admin_req(&hub, &headers) {
        return (StatusCode::FORBIDDEN, "Building authority required").into_response();
    }
    let viewer = hub.resolve_authority(admin_token_of(&headers), session_of(&headers));
    Json(with_people(
        &hub.building_ranks_for(&viewer),
        "name",
        &hub.people_by_name(),
    ))
    .into_response()
}

/// `PUT /profile {"display_name": …}` — a Master or Smaster renames itself.
/// The verified principal comes from the credential/session, never the body.
pub(crate) async fn rename_profile_route(
    State(hub): State<Hub>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let actor = hub.resolve_authority(admin_token_of(&headers), session_of(&headers));
    let requested = body
        .get("display_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    match hub.rename_profile(&actor, requested) {
        Ok(profile) => (StatusCode::OK, Json(profile)).into_response(),
        Err(hub::ProfileRenameError::AuthorityRequired) => (
            StatusCode::FORBIDDEN,
            "only a signed-in Master or Smaster renames its own profile here",
        )
            .into_response(),
        Err(hub::ProfileRenameError::Name(error)) => name_error_response(error),
        Err(hub::ProfileRenameError::RestrictionActive) => (
            StatusCode::FORBIDDEN,
            "a moderation restriction must be resolved before renaming this profile",
        )
            .into_response(),
        Err(hub::ProfileRenameError::NameTaken) => (
            StatusCode::CONFLICT,
            "that name is already used by someone in this Building",
        )
            .into_response(),
        Err(hub::ProfileRenameError::Failed) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save the new name — try again",
        )
            .into_response(),
    }
}

/// Make someone a second master. Only the master may — smaster authority
/// flows from the master and nowhere else, so a smaster cannot appoint one.
pub(crate) async fn create_smaster(
    State(hub): State<Hub>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if !is_master_req(&hub, &headers) {
        return (
            StatusCode::UNAUTHORIZED,
            "only the master appoints a smaster",
        )
            .into_response();
    }
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if name.is_empty() {
        return (StatusCode::BAD_REQUEST, "a smaster needs a name").into_response();
    }
    let name = match hub::validate_display_name(name) {
        Ok(name) => name,
        Err(error) => return name_error_response(error),
    };
    match hub.add_smaster(name) {
        Ok(token) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "token": token, "name": name })),
        )
            .into_response(),
        // The same answer as a Smaster key minted at `POST /keys`: two
        // people must never be told apart only by letter case.
        Err(hub::NameClaimError::Worn) => {
            (StatusCode::CONFLICT, "someone already wears that name").into_response()
        }
        Err(hub::NameClaimError::Storage) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save smaster — try again",
        )
            .into_response(),
    }
}

/// Take the authority back. Master-only, for the same reason.
pub(crate) async fn revoke_smaster_route(
    State(hub): State<Hub>,
    Path(token): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !is_master_req(&hub, &headers) {
        return (
            StatusCode::UNAUTHORIZED,
            "only the master revokes a smaster",
        )
            .into_response();
    }
    match hub.revoke_smaster_ref(&token) {
        Ok(true) => (StatusCode::OK, "revoked").into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such smaster").into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save revocation — try again",
        )
            .into_response(),
    }
}
