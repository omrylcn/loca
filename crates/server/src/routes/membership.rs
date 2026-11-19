use crate::*;

#[derive(serde::Deserialize)]
pub(crate) struct CreateProfileCredential {
    label: String,
}

fn request_principal(hub: &Hub, headers: &HeaderMap) -> Option<String> {
    hub.resolve_authority(admin_token_of(headers), session_of(headers))
        .principal_id
}

#[derive(serde::Deserialize)]
pub(crate) struct ProfileQuery {
    room: Option<String>,
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
fn with_people<T: serde::Serialize>(
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
fn with_cards_by_id<T: serde::Serialize>(
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
            "a name is 1-64 ASCII letters, digits, '-', '_' or '.', so it can be @mentioned"
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
/// What does the invite I am holding open? Lets a client file its invite under
/// the right loca instead of guessing — and tells a holder plainly when it has
/// been revoked.
pub(crate) async fn whoami(State(hub): State<Hub>, headers: HeaderMap) -> impl IntoResponse {
    if let Some(identity) = headers
        .get(SESSION_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|token| hub.session_identity(Some(token)))
    {
        let mut answer = serde_json::json!({
            "kind": "session",
            "name": identity.name,
            "member_kind": identity.kind,
            "loca": identity.loca,
            "admin": identity.admin,
        });
        if let Some(principal) = identity.principal_id.as_deref() {
            answer["principal_id"] = serde_json::Value::String(principal.to_string());
            hub.profile_card(principal).decorate(&mut answer);
        }
        return Json(answer).into_response();
    }
    let token = member_token_of(&headers);
    // A membership (mb_) token answers "who am I" with the identity itself —
    // the building knows this person even before they take a seat. Checked
    // before the invite/building branches because it is the strongest answer.
    if let Some(m) = hub.member_for_credential(token) {
        let locas: Vec<String> = hub
            .invites_for_member(&m.id)
            .into_iter()
            .map(|i| i.room)
            .collect();
        let mut answer = serde_json::json!({
            "kind": "member",
            "name": m.name,
            "member_kind": m.kind,
            "locas": locas,
        });
        if let Some(principal) = m.principal_id.as_deref() {
            answer["principal_id"] = serde_json::Value::String(principal.to_string());
            hub.profile_card(principal).decorate(&mut answer);
        }
        return Json(answer).into_response();
    }
    match token.and_then(|t| hub.invite_room(t)) {
        Some(loca) => Json(serde_json::json!({ "kind": "davet", "loca": loca })).into_response(),
        None if hub.is_member(token) => {
            // The building key (or the master's own) — reaches every loca.
            Json(serde_json::json!({ "kind": "building" })).into_response()
        }
        None => (StatusCode::UNAUTHORIZED, "no davet").into_response(),
    }
}
/// The header a set-up names itself with when it claims a membership key.
/// A header, like the join request's `x-join-secret`, so it never lands in a
/// URL, and so an older kit that sends no body (or `{}`) is read the same.
const CLAIM_ID_HEADER: &str = "x-claim-id";

/// The set-up's claim id, if one was sent: `Ok(None)` when absent,
/// `Err(())` when present but not one the kit generates (`cl_` and 32 to
/// 128 URL-safe characters). A key, a key's hash or an id (`mbid_`, `ivid_`,
/// `cr_`, `pr_`) is therefore never taken for one.
fn claim_id_of(headers: &HeaderMap) -> Result<Option<&str>, ()> {
    let Some(value) = headers.get(CLAIM_ID_HEADER) else {
        return Ok(None);
    };
    let value = value.to_str().map_err(|_| ())?;
    let body = value.strip_prefix("cl_").ok_or(())?;
    let valid = (32..=128).contains(&body.len())
        && body
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    valid.then_some(Some(value)).ok_or(())
}

fn bad_claim_id() -> axum::response::Response {
    (
        StatusCode::BAD_REQUEST,
        "x-claim-id must be cl_ followed by 32-128 letters, digits, '-' or '_'",
    )
        .into_response()
}

fn claim_storage_unavailable() -> axum::response::Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "the server could not read or record this claim just now — try again",
    )
        .into_response()
}

/// Claim a membership key with a live loca invite.
///
/// The invite proves which member holds it. The answer is a key of this
/// set-up's own for that member: a new credential for the member's
/// principal, never the member's original key, and nothing existing is
/// revoked or replaced. It opens the Lobby only, never a loca, so release can
/// end a seat without making the agent run setup again.
///
/// A set-up that sends a claim id (`x-claim-id`) gets the same key for every
/// repeat until it acknowledges it (`POST /membership/claim/ack`) or 24 hours
/// pass; after that it is told which of the two happened (409 already
/// delivered, 410 delivery ended) and nothing new is minted. A call without a
/// claim id (an older kit) mints a key every time. Either way one invite sets
/// up at most five keys; the sixth set-up is 429 and the Master re-issues the
/// invite. `membership_token` keeps the name older kits read.
pub(crate) async fn claim_membership(
    State(hub): State<Hub>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(token) = member_token_of(&headers) else {
        return (StatusCode::UNAUTHORIZED, "a live davet is required").into_response();
    };
    let Ok(claim_id) = claim_id_of(&headers) else {
        return bad_claim_id();
    };
    let (outcome, member) = match hub.claim_membership_key(token, claim_id) {
        Ok(claimed) => claimed,
        Err(hub::ClaimRefusal::NoInvite) => {
            return (StatusCode::UNAUTHORIZED, "a live davet is required").into_response()
        }
        Err(hub::ClaimRefusal::MembershipKey) => {
            return (
                StatusCode::BAD_REQUEST,
                "this is a membership key, not a davet: it already opens the Lobby and \
                 cannot claim another key",
            )
                .into_response()
        }
        Err(hub::ClaimRefusal::Storage) => return claim_storage_unavailable(),
    };
    match outcome {
        crate::store::MemberClaim::Delivered {
            key,
            credential_id,
            fresh,
        } => (
            if fresh {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            },
            Json(serde_json::json!({
                "membership_token": key,
                "credential_id": credential_id,
                "member_id": member.id,
                "name": member.name,
                "kind": member.kind,
                "ack_required": claim_id.is_some(),
            })),
        )
            .into_response(),
        crate::store::MemberClaim::AlreadyDelivered => (
            StatusCode::CONFLICT,
            "already delivered: this set-up received its key and confirmed it was stored, so \
             the server no longer holds it and mints nothing new. Use the key that set-up \
             stored. If it is lost, ask the Master for a new davet.",
        )
            .into_response(),
        crate::store::MemberClaim::DeliveryEnded => (
            StatusCode::GONE,
            "delivery ended: this set-up's key was not confirmed within 24 hours of the claim \
             (or was revoked first), so the server no longer holds it and mints nothing new. \
             Ask the Master for a new davet.",
        )
            .into_response(),
        crate::store::MemberClaim::CapReached => (
            StatusCode::TOO_MANY_REQUESTS,
            "this davet has already set up five keys. Ask the Master to re-issue the davet; \
             a new davet can set up keys of its own.",
        )
            .into_response(),
    }
}

/// POST /membership/claim/ack — the set-up confirms it verified the key it
/// claimed (via `/whoami`) and stored it, so the server drops its delivery
/// copy. The invite (`x-room-token`) and the claim id (`x-claim-id`) together
/// name the set-up; only its holder knows both. Idempotent: a repeat answers
/// 200 too. Like the join-request ACK it also answers after the delivery
/// deadline. A storage error is 503, never 404, so the kit keeps its claim id
/// and tries again.
pub(crate) async fn ack_membership_claim_route(
    State(hub): State<Hub>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(token) = member_token_of(&headers) else {
        return (
            StatusCode::UNAUTHORIZED,
            "the davet the key was claimed with is required",
        )
            .into_response();
    };
    let claim_id = match claim_id_of(&headers) {
        Ok(Some(claim_id)) => claim_id,
        Ok(None) | Err(()) => return bad_claim_id(),
    };
    match hub.ack_membership_claim(token, claim_id) {
        Ok(Some(_)) => (
            StatusCode::OK,
            Json(serde_json::json!({ "acknowledged": true })),
        )
            .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            "no claim for this davet and claim id",
        )
            .into_response(),
        Err(_) => claim_storage_unavailable(),
    }
}
pub(crate) async fn create_session_route(
    State(hub): State<Hub>,
    headers: HeaderMap,
    Json(body): Json<CreateSession>,
) -> impl IntoResponse {
    let paired_admin_ttl =
        pairing_code_of(&headers).and_then(|code| hub.consume_admin_pairing(code));
    let paired_admin = paired_admin_ttl.is_some();
    // An invite holder must be able to take an identity — the invite already says
    // who they are, so refusing here would leave the invited unable to speak.
    // A pairing code has explicit precedence if a stale invite field is also
    // present: one request must mint exactly one identity.
    let token = if paired_admin {
        None
    } else {
        member_token_of(&headers)
    };
    let supplied_admin_credential = admin_token_of(&headers);
    let credential_authority = hub.resolve_authority(supplied_admin_credential, None);
    let credential_principal = credential_authority.principal_id.is_some();
    let personal_authority = hub.resolve_authority(token, None);
    // An explicitly supplied but revoked/unknown profile credential is not an
    // anonymous open-house login. Otherwise revocation could be bypassed by
    // presenting the dead key to `/sessions` and falling through to the
    // legacy no-key branch.
    if supplied_admin_credential.is_some()
        && !credential_principal
        && !credential_authority.is_building_admin()
    {
        return (StatusCode::UNAUTHORIZED, "credential revoked or unknown").into_response();
    }
    if !paired_admin
        && !hub.is_member(token)
        && hub.member_for_credential(token).is_none()
        && !hub.is_invite_token(token)
        && !credential_principal
        && personal_authority.principal_id.is_none()
    {
        return (StatusCode::UNAUTHORIZED, "davet required").into_response();
    }
    // A session inherits both the REACH and the IDENTITY of the invite that
    // opened the door: it sees that one loca, and it speaks as the invite's
    // member — not as whatever name the body claims (alice's invite cannot
    // mint a "bob"). A building key carries no member, so its session keeps
    // the body's name.
    let invite = token.and_then(|t| hub.invite_by_token(t));
    // Minted with a one-use master pairing code (or the legacy raw-key API
    // path)? Then it is an ADMIN session: it carries authority without making
    // the browser retain the root key, and it expires.
    let admin_grant = if let Some(ttl) = paired_admin_ttl {
        Some(hub.master_pairing_grant(ttl))
    } else if invite.is_none() && admin_token_of(&headers).is_some() {
        hub.admin_session_grant(admin_token_of(&headers), Hub::ADMIN_SESSION_TTL_MS)
    } else if invite.is_none()
        && (hub.member_for_credential(token).is_some()
            || matches!(
                personal_authority.building_role,
                Some(crate::store::BuildingRole::Smaster)
            ))
    {
        // Bind Smaster and Member credentials through canonical authority.
        // Master pairing and anonymous/shared-key sandbox entry stay separate.
        hub.admin_session_grant(token, Hub::ADMIN_SESSION_TTL_MS)
    } else {
        None
    };
    // Verified credentials carry a canonical identity. Anonymous sandbox entry
    // still requires a name; Master pairing never asks the user to name itself.
    if body.name.trim().is_empty() && invite.is_none() && admin_grant.is_none() {
        return (StatusCode::BAD_REQUEST, "empty name").into_response();
    }
    // Only a session taken with a credential that itself reaches the whole
    // Building through the building key inherits that reach. A member's own
    // key or profile credential does not: exchanging it for a session must
    // not widen what it opens.
    let building_key = invite.is_none() && hub.reaches_building(token);
    // A member's own key that no principal grant could bind (memory-only
    // store): bind the session to the membership, so it speaks as that member
    // and dies when the membership is revoked.
    let key_member = if invite.is_none() && admin_grant.is_none() {
        hub.member_for_credential(token)
    } else {
        None
    };
    match hub.create_session_scoped(body, invite.as_ref(), admin_grant, building_key, key_member) {
        Ok(Some(session)) => (StatusCode::CREATED, Json(session)).into_response(),
        Ok(None) => (StatusCode::UNAUTHORIZED, "credential revoked or unknown").into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not persist the admin session — try again",
        )
            .into_response(),
    }
}
pub(crate) fn pairing_ttl_ms(query: &HashMap<String, String>) -> Result<u64, &'static str> {
    let hours = match query.get("ttl_hours") {
        Some(value) => value
            .parse::<u64>()
            .map_err(|_| "ttl_hours must be a whole number")?,
        None => Hub::ADMIN_SESSION_TTL_MS / (60 * 60 * 1000),
    };
    let ttl_ms = hours
        .checked_mul(60 * 60 * 1000)
        .ok_or("ttl_hours is too large")?;
    if !(Hub::MIN_ADMIN_SESSION_TTL_MS..=Hub::MAX_ADMIN_SESSION_TTL_MS).contains(&ttl_ms) {
        return Err("session duration must be between 1 hour and 365 days");
    }
    Ok(ttl_ms)
}
pub(crate) async fn create_pairing_route(
    State(hub): State<Hub>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !is_master_req(&hub, &headers) {
        return (StatusCode::UNAUTHORIZED, "master required").into_response();
    }
    let ttl_ms = match pairing_ttl_ms(&query) {
        Ok(ttl) => ttl,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };
    match hub.rotate_admin_pairing_for(ttl_ms) {
        Some((pairing_code, pairing_expires_at)) => (
            StatusCode::CREATED,
            Json(protocol::PairingInfo {
                pairing_code,
                session_ttl_hours: ttl_ms / (60 * 60 * 1000),
                pairing_expires_at,
            }),
        )
            .into_response(),
        None => (StatusCode::CONFLICT, "ADMIN_TOKEN is not configured").into_response(),
    }
}
#[derive(serde::Deserialize)]
pub(crate) struct MintAdmissionStock {
    count: u32,
    ttl_hours: Option<u64>,
}

fn admission_ttl_ms(ttl_hours: Option<u64>) -> Result<u64, &'static str> {
    let hours = ttl_hours.unwrap_or(24);
    let ms = hours
        .checked_mul(60 * 60 * 1000)
        .ok_or("ttl_hours is too large")?;
    // Bound the lifetime to 1 hour .. 90 days.
    if !(60 * 60 * 1000..=90 * 24 * 60 * 60 * 1000).contains(&ms) {
        return Err("ttl_hours must be between 1 and 2160 (90 days)");
    }
    Ok(ms)
}

/// POST /admission-stock — a Master pre-mints a batch of single-use,
/// time-limited Lobby-admission rights that loca-care later hands out. Only the
/// resulting counts are returned; the right tokens never leave the server (they
/// are delivered one-at-a-time when the join-request approve step consumes one).
pub(crate) async fn create_admission_stock_route(
    State(hub): State<Hub>,
    headers: HeaderMap,
    Json(body): Json<MintAdmissionStock>,
) -> impl IntoResponse {
    if !is_admin_req(&hub, &headers) {
        return (StatusCode::UNAUTHORIZED, "building admin required").into_response();
    }
    if body.count == 0 || body.count > 100 {
        return (StatusCode::BAD_REQUEST, "count must be between 1 and 100").into_response();
    }
    let ttl_ms = match admission_ttl_ms(body.ttl_hours) {
        Ok(ms) => ms,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };
    let (minted, total, available) = hub.mint_admission_stock(body.count, ttl_ms);
    (
        StatusCode::CREATED,
        Json(serde_json::json!({ "minted": minted, "total": total, "available": available })),
    )
        .into_response()
}

/// GET /admission-stock — the Master's remaining admission capacity.
pub(crate) async fn get_admission_stock_route(
    State(hub): State<Hub>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !is_admin_req(&hub, &headers) {
        return (StatusCode::UNAUTHORIZED, "building admin required").into_response();
    }
    let (total, available) = hub.admission_stock_summary();
    Json(serde_json::json!({ "total": total, "available": available })).into_response()
}

#[derive(serde::Deserialize)]
pub(crate) struct CreateJoinRequest {
    name: String,
    kind: Option<String>,
}

// The doors' name rule (`valid_agent_name`, `admission_name_refusal`) lives
// in the hub, which decides it where the name is written.
pub(crate) use crate::hub::admission_name_refusal;

/// Why a new member's name that someone already wears in some letter case
/// (`Hub::new_member_name_clashes`) is refused, at every admission door, with
/// 409 as the join-request door answers a name that is taken.
pub(crate) const NAME_WORN_RULE: &str =
    "someone in this Building already has that name in some letter case — choose another";

/// Client IP for the per-source join-request rate-limit — SECURE BY DEFAULT.
///
/// `X-Forwarded-For` is honored ONLY when the direct TCP peer is itself a trusted
/// proxy, which by default means loopback: prod binds room-server to loopback
/// behind nginx, and nginx appends the real client as the RIGHTMOST XFF entry, so
/// that entry is the true client. When the peer is NOT loopback — a direct or
/// internet-facing self-host with no proxy, or an attacker reaching the server
/// directly — the header is entirely client-controlled, so honoring it would let
/// one source forge a fresh IP per request and slip its own bucket (evading the
/// limit) or reuse a victim's IP (framing them). So for a non-loopback peer we
/// bucket by the real TCP peer, which the client cannot spoof. A remote/non-loopback
/// trusted proxy would need an explicit trusted-CIDR config (documented follow-up);
/// the loopback default already covers the prod deployment and every no-proxy
/// self-host safely.
fn client_ip(headers: &HeaderMap, peer: std::net::SocketAddr) -> std::net::IpAddr {
    if peer.ip().is_loopback() {
        if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
            if let Some(ip) = xff
                .split(',')
                .rev()
                .find_map(|s| s.trim().parse::<std::net::IpAddr>().ok())
            {
                return ip;
            }
        }
    }
    peer.ip()
}

/// POST /join-requests — an outside agent requests to join and names itself.
/// AUTHLESS and grants nothing; returns {request_id, request_secret}. The secret
/// is shown exactly once and is required to poll/bootstrap; a full pending
/// backlog yields 429.
pub(crate) async fn create_join_request_route(
    State(hub): State<Hub>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<CreateJoinRequest>,
) -> impl IntoResponse {
    let name = body.name.trim();
    if let Some(rule) = admission_name_refusal(name) {
        return (StatusCode::BAD_REQUEST, rule).into_response();
    }
    let kind = match body.kind.as_deref() {
        None | Some("agent") => "agent",
        Some("user") => "user",
        Some(_) => {
            return (StatusCode::BAD_REQUEST, "kind must be 'agent' or 'user'").into_response()
        }
    };
    match hub.create_join_request(name, kind, client_ip(&headers, peer)) {
        crate::hub::JoinRequestCreate::Created {
            request_id,
            request_secret,
        } => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "request_id": request_id, "request_secret": request_secret })),
        )
            .into_response(),
        crate::hub::JoinRequestCreate::NameTaken => (
            StatusCode::CONFLICT,
            "that name already exists — choose another",
        )
            .into_response(),
        crate::hub::JoinRequestCreate::NameWorn => {
            (StatusCode::CONFLICT, NAME_WORN_RULE).into_response()
        }
        crate::hub::JoinRequestCreate::BacklogFull => (
            StatusCode::TOO_MANY_REQUESTS,
            "too many pending join requests — try again later",
        )
            .into_response(),
    }
}

/// The per-request secret rides in the `x-join-secret` header, NEVER the query
/// string (which leaks into URLs, access logs, and browser history) — review
/// blocker #2.
fn join_secret_of(headers: &HeaderMap) -> Option<&str> {
    headers.get("x-join-secret").and_then(|v| v.to_str().ok())
}

/// GET /join-requests/:id — the requester polls its status (secret in header).
/// Never carries the mb_ credential; `bootstrap_ready` signals when to bootstrap.
pub(crate) async fn get_join_request_route(
    State(hub): State<Hub>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(secret) = join_secret_of(&headers) else {
        return (StatusCode::UNAUTHORIZED, "x-join-secret header required").into_response();
    };
    match hub.join_request_view(&id, secret) {
        // `bootstrap_expired` is additive: an approved request whose window
        // closed keeps `status: "approved"` and `bootstrap_ready: false`, the
        // answer that already makes an installed helper stop and send the
        // requester to an operator instead of polling for ever.
        Ok(Some(view)) => Json(serde_json::json!({
            "status": view.status,
            "bootstrap_ready": view.bootstrap_ready,
            "bootstrap_expired": view.bootstrap_expired,
        }))
        .into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "unknown request or secret").into_response(),
        Err(_) => join_storage_unavailable(),
    }
}

/// A storage error on a requester's own request. 503, never 404: the
/// requester's helper forgets its request on a 404 and would give up, or
/// file a new one, over a fault that a retry would get past.
fn join_storage_unavailable() -> axum::response::Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "the server could not read this request just now — try again",
    )
        .into_response()
}

/// POST /join-requests/:id/bootstrap — deliver the approved mb_, RE-FETCHABLE
/// until the client ACKs (secret in the `x-join-secret` header). Idempotent by
/// design: a client that crashes or drops the connection between receiving mb_
/// and persisting it re-fetches the same credential instead of losing it. The
/// window closes when the client, having verified mb_ via /whoami, calls
/// POST /join-requests/:id/ack, or 24 hours after the approval
/// (`store::JOIN_BOOTSTRAP_WINDOW_MS`), whichever comes first. After the
/// deadline the answer is 410 with what to do; the membership is kept.
pub(crate) async fn bootstrap_join_request_route(
    State(hub): State<Hub>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(secret) = join_secret_of(&headers) else {
        return (StatusCode::UNAUTHORIZED, "x-join-secret header required").into_response();
    };
    match hub.claim_join_request_bootstrap(&id, secret) {
        Ok(crate::store::BootstrapClaim::Delivered(mb)) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "davet": mb })),
        )
            .into_response(),
        Ok(crate::store::BootstrapClaim::Expired) => (
            StatusCode::GONE,
            "approved, but the key was not collected within 24 hours of the approval, so the \
             server no longer holds it. The membership was kept, so a new join request for \
             this name is refused: ask a Master to give you access again from the Building \
             master desk (a membership or a davet for this same name).",
        )
            .into_response(),
        Ok(crate::store::BootstrapClaim::Unavailable) => (
            StatusCode::NOT_FOUND,
            "not approved, already finalized, or bad secret",
        )
            .into_response(),
        Err(_) => join_storage_unavailable(),
    }
}

/// POST /join-requests/:id/ack — the requester confirms it persisted AND
/// verified the mb_ (via /whoami), so the server closes the one-time delivery
/// window (secret in the `x-join-secret` header). Until this call the bootstrap
/// stays re-fetchable, so a crash cannot lose the credential; after it, the
/// bootstrap endpoint returns 404. Idempotent: acking an already-acked request
/// owned by the same secret still returns 200.
pub(crate) async fn ack_join_request_route(
    State(hub): State<Hub>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(secret) = join_secret_of(&headers) else {
        return (StatusCode::UNAUTHORIZED, "x-join-secret header required").into_response();
    };
    match hub.ack_join_request_bootstrap(&id, secret) {
        Ok(Some(_)) => (
            StatusCode::OK,
            Json(serde_json::json!({ "acknowledged": true })),
        )
            .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            "unknown request or secret, or not approved",
        )
            .into_response(),
        Err(_) => join_storage_unavailable(),
    }
}

/// GET /join-requests — the Master's pending-request review list.
pub(crate) async fn list_join_requests_route(
    State(hub): State<Hub>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !is_admin_req(&hub, &headers) {
        return (StatusCode::UNAUTHORIZED, "building admin required").into_response();
    }
    let pending: Vec<_> = hub
        .list_pending_join_requests()
        .into_iter()
        .map(|(id, name, kind, created_at)| {
            serde_json::json!({ "id": id, "name": name, "kind": kind, "created_at": created_at })
        })
        .collect();
    Json(serde_json::json!({ "pending": pending })).into_response()
}

/// POST /join-requests/:id/approve — Master consumes one stock right and issues
/// the Lobby membership. Exactly-once: a repeat approve is a no-op.
pub(crate) async fn approve_join_request_route(
    State(hub): State<Hub>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    if !is_admin_req(&hub, &headers) {
        return (StatusCode::UNAUTHORIZED, "building admin required").into_response();
    }
    let by = super::actor::rank_actor(&hub, &headers);
    match hub.approve_join_request(&id, &by) {
        crate::hub::Approve::Approved => (
            StatusCode::OK,
            Json(serde_json::json!({ "status": "approved" })),
        )
            .into_response(),
        crate::hub::Approve::AlreadyDecided => (
            StatusCode::OK,
            Json(serde_json::json!({ "status": "already_decided" })),
        )
            .into_response(),
        crate::hub::Approve::NameTaken => (
            StatusCode::CONFLICT,
            "that name already belongs to a member — request refused",
        )
            .into_response(),
        crate::hub::Approve::NameRule(rule) => (StatusCode::CONFLICT, rule).into_response(),
        crate::hub::Approve::NameWorn => (StatusCode::CONFLICT, NAME_WORN_RULE).into_response(),
        crate::hub::Approve::NoStock => (
            StatusCode::CONFLICT,
            "no admission stock — mint more with POST /admission-stock",
        )
            .into_response(),
        crate::hub::Approve::Failed => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not issue membership — try again",
        )
            .into_response(),
    }
}

/// POST /join-requests/:id/deny — Master denies a pending request.
pub(crate) async fn deny_join_request_route(
    State(hub): State<Hub>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    if !is_admin_req(&hub, &headers) {
        return (StatusCode::UNAUTHORIZED, "building admin required").into_response();
    }
    // Record WHICH authority denied, matching the approve route's audit trail.
    let by = super::actor::rank_actor(&hub, &headers);
    if hub.deny_join_request(&id, &by) {
        (
            StatusCode::OK,
            Json(serde_json::json!({ "status": "denied" })),
        )
            .into_response()
    } else {
        (StatusCode::CONFLICT, "request is not pending").into_response()
    }
}

pub(crate) async fn delete_session_route(
    State(hub): State<Hub>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(token) = session_of(&headers) else {
        return (StatusCode::UNAUTHORIZED, "session token required").into_response();
    };
    match hub.revoke_session(token) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::UNAUTHORIZED, "invalid or expired session").into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not revoke the session — try again",
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers_with_xff(xff: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Some(v) = xff {
            h.insert("x-forwarded-for", v.parse().expect("valid header value"));
        }
        h
    }

    #[test]
    fn client_ip_trusts_x_forwarded_for_only_from_a_loopback_peer() {
        let ip = |s: &str| s.parse::<std::net::IpAddr>().expect("ip");
        let sock = |s: &str| s.parse::<std::net::SocketAddr>().expect("sock");

        // A DIRECT (non-loopback) peer sending a spoofed XFF must NOT escape its
        // bucket: the header is ignored and we bucket by the real TCP peer.
        assert_eq!(
            client_ip(&headers_with_xff(Some("1.2.3.4")), sock("203.0.113.9:5000")),
            ip("203.0.113.9"),
            "a non-loopback peer's X-Forwarded-For must be ignored (no bypass)"
        );
        // Even a whole forged chain from a direct peer is ignored.
        assert_eq!(
            client_ip(
                &headers_with_xff(Some("10.0.0.1, 8.8.8.8")),
                sock("198.51.100.7:443")
            ),
            ip("198.51.100.7")
        );

        // A LOOPBACK peer is our trusted reverse proxy (nginx): honor the RIGHTMOST
        // XFF entry, which nginx set to the real client.
        assert_eq!(
            client_ip(
                &headers_with_xff(Some("9.9.9.9, 5.6.7.8")),
                sock("127.0.0.1:5000")
            ),
            ip("5.6.7.8"),
            "behind the loopback proxy the rightmost XFF entry is the real client"
        );
        // Loopback peer with no XFF falls back to the peer itself.
        assert_eq!(
            client_ip(&headers_with_xff(None), sock("127.0.0.1:5000")),
            ip("127.0.0.1")
        );
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct LeaveBuilding {
    #[serde(default)]
    confirm: String,
}

/// `POST /membership/leave` — a member leaves the Building of their own
/// accord (PRINCIPLES "Authority and departure matrix": Leave). It is the
/// Master's revocation done by the member: the membership ends with every
/// seat, key and session it authorized; the words already said stay with
/// their locas. Only the member's own Building sign-in can do it (a loca
/// invite cannot), the Master cannot leave their own Building, and the body
/// must say `{"confirm": "leave"}`.
pub(crate) async fn leave_building_route(
    State(hub): State<Hub>,
    headers: HeaderMap,
    Json(body): Json<LeaveBuilding>,
) -> impl IntoResponse {
    let Some(identity) = session_of(&headers).and_then(|token| hub.session_identity(Some(token)))
    else {
        return (StatusCode::UNAUTHORIZED, "sign in to leave the Building").into_response();
    };
    if identity.loca.is_some() {
        return (
            StatusCode::FORBIDDEN,
            "a loca invite cannot leave the Building; sign in with your own key",
        )
            .into_response();
    }
    if matches!(identity.building_role, crate::store::BuildingRole::Master) {
        return (
            StatusCode::FORBIDDEN,
            "the Master cannot leave their own Building",
        )
            .into_response();
    }
    if body.confirm != "leave" {
        return (
            StatusCode::BAD_REQUEST,
            "to leave, send {\"confirm\": \"leave\"}",
        )
            .into_response();
    }
    let Some(member_id) = identity
        .principal_id
        .as_deref()
        .and_then(|principal| hub.member_id_of_principal(principal))
    else {
        return (StatusCode::NOT_FOUND, "no Building membership to leave").into_response();
    };
    match hub.revoke_member(&member_id) {
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({ "left": true }))).into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no Building membership to leave").into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save leaving — try again",
        )
            .into_response(),
    }
}
