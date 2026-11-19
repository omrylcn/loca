use crate::*;

#[derive(serde::Deserialize)]
pub(crate) struct CreateLoca {
    name: String,
    /// The extensions the new loca starts with. Absent keeps the legacy set
    /// (every extension that existed before they could be chosen), so an
    /// older client or script that sends only a name is unaffected. The web
    /// create screen always sends its ticked list.
    #[serde(default)]
    extensions: Option<Vec<String>>,
    /// The group the new loca belongs to (its id). Absent or null: the
    /// default group, exactly as before groups existed.
    #[serde(default)]
    group: Option<String>,
    /// The loca's identity (all optional; see `hub::IdentityEdit`). Absent
    /// fields open the loca exactly as before.
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    seats: Option<u32>,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    color: Option<String>,
    #[serde(default)]
    labels: Option<Vec<String>>,
}

use super::groups::group_error as groups_error;

pub(crate) async fn create_loca_route(
    State(hub): State<Hub>,
    headers: HeaderMap,
    Json(body): Json<CreateLoca>,
) -> impl IntoResponse {
    let name = body.name.trim();
    // The character rule needs no other loca; the kit-key clash is asked
    // with the locas held, when the loca is opened.
    if let Err(error) = hub::loca_name::check_chars(name) {
        return name_refusal(&error);
    }
    let extensions = match body.extensions.as_deref().map(hub::normalize_extensions) {
        None => None,
        // A new loca has no Lead yet, so it cannot open with the Loca Board.
        Some(Ok(list)) if list.iter().any(|id| id == "board") => {
            let error = hub::ExtensionError::BoardNeedsLead;
            return (StatusCode::CONFLICT, error.message()).into_response();
        }
        Some(Ok(list)) => Some(list),
        Some(Err(error)) => return (StatusCode::BAD_REQUEST, error.message()).into_response(),
    };
    let actor = hub.resolve_authority(admin_token_of(&headers), session_of(&headers));
    let group = body.group.as_deref().filter(|g| !g.is_empty());
    // A title is given later, from the loca's identity panel; a new loca
    // shows its id.
    let identity = hub::IdentityEdit {
        title: None,
        description: body.description,
        seats: body.seats,
        icon: body.icon,
        color: body.color,
        labels: body.labels,
    };
    match hub.create_loca_with(name, &actor, extensions, group, &identity) {
        Ok(assignment) => (
            StatusCode::CREATED,
            Json(serde_json::json!({"room":name,"operator":assignment,"group":group})),
        )
            .into_response(),
        Err(hub::CreateLocaError::Group(error)) => groups_error(error),
        Err(hub::CreateLocaError::Identity(error)) => {
            (StatusCode::BAD_REQUEST, error.message()).into_response()
        }
        Err(hub::CreateLocaError::Name(error)) => name_refusal(&error),
        Err(hub::CreateLocaError::Operator(hub::OperatorAssignmentError::AuthorityRequired)) => {
            (StatusCode::FORBIDDEN, "Building authority required").into_response()
        }
        Err(hub::CreateLocaError::Operator(hub::OperatorAssignmentError::Conflict)) => (
            StatusCode::CONFLICT,
            "loca already exists or is permanently sealed",
        )
            .into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not create loca and operator",
        )
            .into_response(),
    }
}

/// A new loca's name refused (`hub::loca_name`): 409 when its agent kit key
/// is an existing loca's, 400 otherwise.
pub(crate) fn name_refusal(error: &hub::LocaNameError) -> axum::response::Response {
    let status = if error.is_conflict() {
        StatusCode::CONFLICT
    } else {
        StatusCode::BAD_REQUEST
    };
    (status, error.message()).into_response()
}

/// The loopback sandbox opens a loca the first time a route or the socket
/// names it; refuse there an id the rule refuses (`Hub::refuse_implicit_loca`).
pub(crate) fn implicit_loca_refusal(hub: &Hub, room: &str) -> Option<axum::response::Response> {
    hub.refuse_implicit_loca(room)
        .map(|error| name_refusal(&error))
}

#[derive(serde::Deserialize)]
pub(crate) struct AppointLocaOperator {
    principal_id: String,
}

pub(crate) async fn get_loca_operator(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
) -> impl IntoResponse {
    let history = if is_admin_req(&hub, &headers) {
        hub.loca_operator_history(&access.room)
    } else {
        Vec::new()
    };
    Json(serde_json::json!({
        "building_master": hub.master_principal().map(|master| serde_json::json!({
            "principal_id": master.id,
            "display_name": master.display_name,
            "kind": master.kind,
            "building_role": "master",
            "source": "building",
        })),
        "appointed": hub.loca_operator(&access.room),
        "history": history,
    }))
}

pub(crate) async fn appoint_loca_operator_route(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    Json(body): Json<AppointLocaOperator>,
) -> impl IntoResponse {
    let actor = hub.resolve_authority(admin_token_of(&headers), session_of(&headers));
    match hub.appoint_loca_operator(&access.room, body.principal_id.trim(), &actor) {
        Ok(assignment) => (StatusCode::CREATED, Json(assignment)).into_response(),
        Err(hub::OperatorAssignmentError::AuthorityRequired) => {
            (StatusCode::FORBIDDEN, "Building authority required").into_response()
        }
        Err(hub::OperatorAssignmentError::EmptySeatRequired) => (
            StatusCode::CONFLICT,
            "Smaster may appoint only while the explicit operator seat is empty",
        )
            .into_response(),
        Err(hub::OperatorAssignmentError::PrincipalNotFound) => {
            (StatusCode::NOT_FOUND, "no such active profile").into_response()
        }
        Err(hub::OperatorAssignmentError::PrincipalMustBeHuman) => (
            StatusCode::BAD_REQUEST,
            "Loca Operator must be a human profile",
        )
            .into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not appoint operator — try again",
        )
            .into_response(),
    }
}

pub(crate) async fn revoke_loca_operator_route(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
) -> impl IntoResponse {
    let actor = hub.resolve_authority(admin_token_of(&headers), session_of(&headers));
    match hub.revoke_loca_operator(&access.room, &actor) {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(hub::OperatorAssignmentError::AuthorityRequired) => {
            (StatusCode::FORBIDDEN, "Building authority required").into_response()
        }
        Err(hub::OperatorAssignmentError::MasterProtected) => (
            StatusCode::FORBIDDEN,
            "Smaster cannot remove an appointment made by Master",
        )
            .into_response(),
        Err(hub::OperatorAssignmentError::NotFound) => {
            (StatusCode::NOT_FOUND, "no appointed operator").into_response()
        }
        Err(hub::OperatorAssignmentError::Conflict) => {
            (StatusCode::CONFLICT, "operator appointment changed; retry").into_response()
        }
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not remove operator — try again",
        )
            .into_response(),
    }
}

pub(crate) async fn list_rooms(State(hub): State<Hub>, headers: HeaderMap) -> impl IntoResponse {
    // You see the locas you may enter — no more. An invite opens one loca, so it
    // lists one; the master (or building key) reaches the building, so it lists
    // all. This uses the same door decision as every other room-scoped read.
    Json(hub.room_summaries_for(caller_reach(&hub, &headers)))
}
/// The caller's door, decided per loca: whether the credentials on this
/// request may enter `room`. The credentials are resolved once; each loca is
/// then judged by `enter_decision`, the single door decision. `GET /rooms`
/// lists with it and `GET /search` reads with it, so the two cannot differ.
pub(crate) fn caller_reach<'a>(hub: &'a Hub, headers: &HeaderMap) -> impl Fn(&str) -> bool + 'a {
    let session = headers
        .get(SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|t| hub.session_identity(Some(t)));
    let admin = admin_token_of(headers).map(str::to_string);
    let member = member_token_of(headers).map(str::to_string);
    move |room: &str| {
        hub.enter_decision(
            room,
            admin.as_deref(),
            member.as_deref(),
            session.as_ref(),
            None,
        )
        .is_allowed()
    }
}
/// `GET /building/bans` — who is barred from which loca, for every loca the
/// caller may enter, in one request (the Building view used to ask each loca
/// in turn). Direct messages are conversation only and never listed.
pub(crate) async fn list_building_bans(
    State(hub): State<Hub>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !is_admin_req(&hub, &headers) {
        return (StatusCode::FORBIDDEN, "Building authority required").into_response();
    }
    let rows: Vec<_> = hub
        .bans_by_room()
        .into_iter()
        .filter(|(room, _)| !hub.is_dm(room) && room_decision_of(&hub, room, &headers).is_allowed())
        .map(|(room, banned)| serde_json::json!({ "room": room, "banned": banned }))
        .collect();
    Json(rows).into_response()
}
pub(crate) async fn get_members(State(hub): State<Hub>, access: RoomAccess) -> impl IntoResponse {
    Json(hub.members(&access.room)).into_response()
}
pub(crate) async fn get_messages(
    State(hub): State<Hub>,
    access: RoomAccess,
    Query(q): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let id = access.room;
    let Some(since) = q.get("since").and_then(|s| s.parse::<u64>().ok()) else {
        return Json(hub.recent_messages(&id)).into_response();
    };
    let limit = q
        .get("limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(200)
        .clamp(1, 1_000);
    match hub.messages_after(&id, since, limit) {
        Ok(messages) => Json(messages).into_response(),
        Err(error) => {
            tracing::error!(%error, room = id, "durable message backfill failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "could not read message archive — retry",
            )
                .into_response()
        }
    }
}
// ---- chat mode (admin-controlled) ----

pub(crate) async fn get_mode(State(hub): State<Hub>, access: RoomAccess) -> impl IntoResponse {
    Json(hub.mode(&access.room))
}
pub(crate) async fn set_mode(
    State(hub): State<Hub>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<SetMode>,
) -> impl IntoResponse {
    // A loca operator runs their own loca's mode (PRINCIPLES: operator manages
    // mode/turn order), not only the master.
    if !is_room_operator_req(&hub, &id, &headers) {
        return (
            StatusCode::UNAUTHORIZED,
            "operator authority required for this loca",
        )
            .into_response();
    }
    if !hub.is_writable(&id) {
        return (StatusCode::CONFLICT, "this loca is closed — read-only").into_response();
    }
    match hub.set_mode(&id, body.mode) {
        Ok(mode) => (StatusCode::OK, Json(mode)).into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save mode — try again",
        )
            .into_response(),
    }
}
/// Name (or clear) this loca's lead. An explicit operator action — the old way
/// (typing "@lead x" in chat) is gone, because talking must not mutate state.
pub(crate) async fn set_lead(
    State(hub): State<Hub>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<protocol::SetLead>,
) -> impl IntoResponse {
    if !is_room_operator_req(&hub, &id, &headers) {
        return (
            StatusCode::UNAUTHORIZED,
            "operator authority required for this loca",
        )
            .into_response();
    }
    if !hub.is_writable(&id) {
        return (StatusCode::CONFLICT, "this loca is closed — read-only").into_response();
    }
    // Who is naming the lead — the session name, or "operator" as a fallback.
    let by = headers
        .get(SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|t| hub.session_identity(Some(t)))
        .map(|idy| idy.name)
        .unwrap_or_else(|| "operator".into());
    match hub.set_lead(&id, body.lead, &by) {
        Ok(settings) => (StatusCode::OK, Json(settings)).into_response(),
        Err(LeadError::ActiveGoal) => (
            StatusCode::CONFLICT,
            "Active Goal requires a Lead — transfer the Lead or close the Goal first",
        )
            .into_response(),
        Err(LeadError::BoardOn) => (
            StatusCode::CONFLICT,
            "the Loca Board needs a Lead — transfer the Lead or turn the Loca Board off first",
        )
            .into_response(),
        Err(LeadError::Storage) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save lead — try again",
        )
            .into_response(),
    }
}
pub(crate) async fn get_settings(State(hub): State<Hub>, access: RoomAccess) -> impl IntoResponse {
    Json(hub.settings(&access.room))
}
pub(crate) async fn set_settings(
    State(hub): State<Hub>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<SetSettings>,
) -> impl IntoResponse {
    if !is_room_operator_req(&hub, &id, &headers) {
        return (
            StatusCode::UNAUTHORIZED,
            "operator authority required for this loca",
        )
            .into_response();
    }
    if !is_admin_req(&hub, &headers) && (body.operators.is_some() || body.archived.is_some()) {
        return (
            StatusCode::FORBIDDEN,
            "Building authority required for governance changes",
        )
            .into_response();
    }
    if hub.is_deleted(&id) {
        return (StatusCode::CONFLICT, "this loca is permanently sealed").into_response();
    }
    if body.has_identity() {
        return set_identity(&hub, &id, &headers, body);
    }
    // Live mode is an extension: with it off, nobody turns live on. Turning
    // it off stays allowed (removing the extension already does that).
    if body.live == Some(true) && !hub.extension_enabled(&id, "live") {
        return extension_off("live");
    }
    if let Some(protocol::ReminderRecipient::Person { name }) = &body.care_recipient {
        if !valid_identity_name(name) {
            return (
                StatusCode::BAD_REQUEST,
                "reminder recipient must be a valid Loca identity name",
            )
                .into_response();
        }
    }
    // Reminders fail closed when their configured coordinator does not exist.
    // Do not guess a fallback: a legacy room may remain visibly unavailable,
    // but a new active rule cannot be saved until it has a real recipient.
    let current = hub.settings(&id);
    let recipient = body
        .care_recipient
        .as_ref()
        .unwrap_or(&current.care_recipient);
    let enables_reminder = body.care_goal_secs.is_some_and(|value| value > 0)
        || body.care_task_secs.is_some_and(|value| value > 0)
        || body.care_wait_secs.is_some_and(|value| value > 0)
        || body.care_silence_secs.is_some_and(|value| value > 0)
        || (body.care_recipient.is_some()
            && (current.care_goal_secs > 0
                || current.care_task_secs > 0
                || current.care_wait_secs > 0
                || current.care_silence_secs > 0));
    if enables_reminder
        && matches!(recipient, protocol::ReminderRecipient::Lead)
        && current.lead.is_none()
    {
        return (
            StatusCode::BAD_REQUEST,
            "select a room lead or another reminder recipient before enabling reminders",
        )
            .into_response();
    }
    match hub.set_settings(
        &id,
        body.rate_limit,
        body.rate_window_secs,
        body.live,
        body.archived,
        body.live_timeout_secs,
        body.operators,
        body.turn_max_messages,
        body.turn_idle_ms,
        body.turn_max_wait_ms,
        body.care_wait_secs,
        body.care_cooldown_secs,
        body.care_max_attempts,
        body.care_context_messages,
        body.care_recipient,
        body.care_goal_secs,
        body.care_task_secs,
        body.care_silence_secs,
    ) {
        Ok(settings) => (StatusCode::OK, Json(settings)).into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save settings — try again",
        )
            .into_response(),
    }
}
/// The loca's identity through `PUT /rooms/:id/settings`: changed by
/// whoever may change its extensions (the same check), sent on its own, and
/// journaled. Answers the settings after the change, like any settings edit.
fn set_identity(
    hub: &Hub,
    room: &str,
    headers: &HeaderMap,
    body: SetSettings,
) -> axum::response::Response {
    if body.has_other() {
        return (
            StatusCode::BAD_REQUEST,
            "send the loca's identity (title, description, seats, icon, color, labels) on its \
             own, without other settings",
        )
            .into_response();
    }
    if hub.is_dm(room) {
        return (StatusCode::FORBIDDEN, hub::IdentityError::Dm.message()).into_response();
    }
    if !super::extensions::may_manage(hub, room, headers) {
        return (
            StatusCode::FORBIDDEN,
            "only this loca's operator, the Master or a Smaster changes its identity",
        )
            .into_response();
    }
    let edit = hub::IdentityEdit {
        title: body.title,
        description: body.description,
        seats: body.seats,
        icon: body.icon,
        color: body.color,
        labels: body.labels,
    };
    let (by, by_type) = super::extensions::journal_actor(hub, headers);
    let sees = |other: &str| room_decision_of(hub, other, headers).is_allowed();
    match hub.edit_identity(room, &edit, &by, by_type, sees) {
        Ok(settings) => (StatusCode::OK, Json(settings)).into_response(),
        Err(error) => {
            let status = match error {
                hub::IdentityError::Invalid(_) => StatusCode::BAD_REQUEST,
                hub::IdentityError::SeatsHeld { .. } => StatusCode::CONFLICT,
                hub::IdentityError::TitleTaken => StatusCode::CONFLICT,
                hub::IdentityError::Dm => StatusCode::FORBIDDEN,
                hub::IdentityError::NotWritable => StatusCode::CONFLICT,
                hub::IdentityError::Storage => StatusCode::SERVICE_UNAVAILABLE,
            };
            (status, error.message()).into_response()
        }
    }
}

/// Issue an invite for one loca (master only). The token is minted here — the
/// master never types one in, so an invite cannot be guessed or hand-forged.
pub(crate) async fn create_invite(
    State(hub): State<Hub>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CreateInvite>,
) -> impl IntoResponse {
    if !is_admin_req(&hub, &headers) {
        return (StatusCode::UNAUTHORIZED, "master only").into_response();
    }
    let name = body.name.trim();
    if name.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            "name required — a davet is given to someone",
        )
            .into_response();
    }
    // An invite seats an EXISTING member — it never creates identity. Admitting
    // to the building is its own act (POST /members); this door only hands a
    // known member the key to one loca.
    let Some(member) = hub.member_by_name(name) else {
        return (
            StatusCode::CONFLICT,
            "this agent does not belong to the building yet",
        )
            .into_response();
    };
    // Who signed this invite. The distinction matters at revoke time: a
    // smaster may not undo what the master decided.
    let issued_by = hub
        .resolve_authority(admin_token_of(&headers), session_of(&headers))
        .invitation_issuer();
    match hub.invite_member_to_room(&member.id, &id, &issued_by) {
        // The invite is handed over once, here. The member it seats is named
        // by its management id: the server keeps no member key to give.
        Ok((inv, invite)) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "id": inv.id,
                "token": invite,
                "room": inv.room,
                "member_id": inv.member_id,
                "name": inv.name,
                "kind": inv.kind,
                "issued_at": inv.issued_at,
                "issued_by": inv.issued_by,
            })),
        )
            .into_response(),
        Err(hub::InviteError::AlreadyInvited) => {
            (StatusCode::CONFLICT, "already holds a davet for this loca").into_response()
        }
        Err(hub::InviteError::MemberNotFound) => (
            StatusCode::CONFLICT,
            "this agent does not belong to the building yet",
        )
            .into_response(),
        Err(hub::InviteError::Full) => {
            (StatusCode::CONFLICT, "this loca is full — no free seat").into_response()
        }
        Err(hub::InviteError::Reserved) => (
            StatusCode::FORBIDDEN,
            "iye is reserved for Building authority and configured caretakers",
        )
            .into_response(),
        Err(hub::InviteError::Storage) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save davet — try again",
        )
            .into_response(),
        Err(hub::InviteError::MissingRoom) => (
            StatusCode::NOT_FOUND,
            "create the loca before issuing a davet",
        )
            .into_response(),
        Err(hub::InviteError::Sealed) => {
            (StatusCode::CONFLICT, "this loca is permanently sealed").into_response()
        }
        Err(hub::InviteError::NotInGroup) => (
            StatusCode::FORBIDDEN,
            "not a member of this loca's group — add them to the group first",
        )
            .into_response(),
        Err(hub::InviteError::Lobby) => (
            StatusCode::CONFLICT,
            "every member is already in the Lobby — it takes no davet",
        )
            .into_response(),
        Err(hub::InviteError::PrivatePair) => (
            StatusCode::FORBIDDEN,
            "a birebir admits only its two people",
        )
            .into_response(),
    }
}
/// Who currently holds an invite to this loca (master view).
pub(crate) async fn list_invites(
    State(hub): State<Hub>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !is_admin_req(&hub, &headers) {
        return (StatusCode::UNAUTHORIZED, "master only").into_response();
    }
    let rows: Vec<_> = hub
        .invites_of(&id)
        .into_iter()
        .map(|invite| {
            serde_json::json!({
                "id": invite.id,
                "room": invite.room,
                "member_id": invite.member_id,
                "name": invite.name,
                "kind": invite.kind,
                "issued_at": invite.issued_at,
                "issued_by": invite.issued_by,
            })
        })
        .collect();
    (StatusCode::OK, Json(rows)).into_response()
}
/// End an invite (master only). The holder can no longer enter that loca.
pub(crate) async fn revoke_invite(
    State(hub): State<Hub>,
    Path((id, token)): Path<(String, String)>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !is_admin_req(&hub, &headers) {
        return (StatusCode::UNAUTHORIZED, "master only").into_response();
    }
    // An invite belongs to one loca; the URL names a loca. They must agree —
    // otherwise `DELETE /rooms/A/invites/<tokenB>` would revoke loca B's invite
    // through loca A's door, and the API would be doing something other than
    // what it says. A mismatch reads as "no such davet here."
    match hub.invite_by_ref(&token) {
        Some(inv) if inv.room != id => {
            return (StatusCode::NOT_FOUND, "no such davet in this loca").into_response();
        }
        _ => {}
    }
    // The master has the last word: a smaster does everything a master does,
    // except undo the Master's own decisions. A Master principal session may
    // end anything without exposing the root/bootstrap credential.
    {
        let actor = hub.resolve_authority(admin_token_of(&headers), session_of(&headers));
        if hub
            .invite_by_ref(&token)
            .is_some_and(|invite| !hub.may_end_invite(&actor, &invite))
        {
            return (
                StatusCode::FORBIDDEN,
                "cannot end a davet issued by a higher or unknown authority",
            )
                .into_response();
        }
    }
    match hub.revoke_invite_ref(&token) {
        Ok(true) => (StatusCode::OK, "revoked").into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such davet").into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save revocation — try again",
        )
            .into_response(),
    }
}
/// Seal a room permanently (Master only): everyone is dropped; the record is kept.
pub(crate) async fn delete_room(
    State(hub): State<Hub>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !is_master_req(&hub, &headers) {
        return StatusCode::UNAUTHORIZED;
    }
    match hub.delete_room(&id) {
        Ok(true) => StatusCode::NO_CONTENT,
        Ok(false) => StatusCode::NOT_FOUND,
        Err(hub::DeleteReject::NotArchived) => StatusCode::CONFLICT,
        Err(hub::DeleteReject::Storage) => StatusCode::SERVICE_UNAVAILABLE,
    }
}
// ---- per-participant moderation (admin-only) ----

pub(crate) async fn get_mod(State(hub): State<Hub>, access: RoomAccess) -> impl IntoResponse {
    Json(hub.mod_state(&access.room))
}
pub(crate) async fn moderate(
    State(hub): State<Hub>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Moderate>,
) -> impl IntoResponse {
    // Moderation (mute/kick/ban/release) is a loca operator's power over their
    // own loca, not the master's alone (PRINCIPLES: operator manages moderation).
    if !is_room_operator_req(&hub, &id, &headers) {
        return (
            StatusCode::UNAUTHORIZED,
            "operator authority required for this loca",
        )
            .into_response();
    }
    let actor = hub.resolve_authority(admin_token_of(&headers), session_of(&headers));
    if !hub.may_moderate(&id, &actor, body.action, &body.name) {
        return (
            StatusCode::FORBIDDEN,
            "cannot override a higher authority's decision or moderate a protected Building rank",
        )
            .into_response();
    }
    match hub.moderate(&id, body.action, &body.name, &actor) {
        Ok(state) => (StatusCode::OK, Json(state)).into_response(),
        Err(hub::ModerationError::Forbidden) => (
            StatusCode::FORBIDDEN,
            "cannot override a higher authority's restriction or davet",
        )
            .into_response(),
        Err(hub::ModerationError::Storage) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save moderation — try again",
        )
            .into_response(),
    }
}
// ---- tasks: declared work (operator's signature births it) ----

/// May this request run an operator action in `room`? PRINCIPLES: a loca
/// operator (not only the master) runs their loca's mode/mute/moderation/lead.
/// Building management authority and an explicit room appointment independently
/// permit management. Neither is derived from a client-supplied name.
pub(crate) fn is_room_operator_req(hub: &Hub, room: &str, headers: &HeaderMap) -> bool {
    // Building administrators need not occupy this room's Operator appointment.
    if is_admin_req(hub, headers) {
        return true;
    }
    let name = headers
        .get(SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|t| hub.session_identity(Some(t)))
        .map(|idy| idy.name)
        .unwrap_or_default();
    !name.is_empty()
        && hub.is_loca_operator(room, admin_token_of(headers), session_of(headers), &name)
}
/// Resolve who is acting: a session token binds identity; otherwise the body
/// claim stands (dev/localhost trust, same posture as messages).
pub(crate) fn actor_of(
    hub: &Hub,
    headers: &HeaderMap,
    claimed: &str,
) -> Result<String, StatusCode> {
    let sess = headers.get(SESSION_HEADER).and_then(|v| v.to_str().ok());
    let session_identity = hub.session_identity(sess);
    if hub.require_sessions() {
        return session_identity
            .map(|identity| identity.name)
            .ok_or(StatusCode::UNAUTHORIZED);
    }
    let authority = hub.resolve_authority(admin_token_of(headers), sess);
    let legacy_root = admin_token_of(headers).is_some() && hub.is_master(admin_token_of(headers));
    if !legacy_root && authority.principal_id.is_some() && authority.display_name.is_some() {
        return Ok(authority.display_name.unwrap_or_default());
    }
    match session_identity {
        Some(idy) => Ok(idy.name),
        None if sess.is_some() => Err(StatusCode::UNAUTHORIZED),
        None => Ok(claimed.to_string()),
    }
}
pub(crate) fn room_decision_of(hub: &Hub, room: &str, headers: &HeaderMap) -> hub::EnterDecision {
    let session = headers
        .get(SESSION_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|token| hub.session_identity(Some(token)));
    hub.enter_decision(
        room,
        admin_token_of(headers),
        member_token_of(headers),
        session.as_ref(),
        None,
    )
}

#[derive(serde::Deserialize)]
pub(crate) struct TypingSignal {
    on: bool,
}

/// An agent says it is replying. Its model turn runs in the runtime adapter,
/// not in the listener holding its socket, so the adapter says it over REST
/// with its own session; the room hears the same ephemeral `typing` frame a
/// browser's keystrokes send. Only a session seated in this loca may say it,
/// and nothing is stored.
pub(crate) async fn post_typing(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    Json(body): Json<TypingSignal>,
) -> impl IntoResponse {
    let session = headers.get(SESSION_HEADER).and_then(|v| v.to_str().ok());
    let Some(identity) = hub.session_identity(session) else {
        return (StatusCode::UNAUTHORIZED, "a session is required").into_response();
    };
    if identity
        .loca
        .as_deref()
        .is_some_and(|loca| loca != access.room)
    {
        return (StatusCode::UNAUTHORIZED, "this session is for another loca").into_response();
    }
    let seated = hub
        .members(&access.room)
        .iter()
        .any(|member| member.name == identity.name);
    if !seated {
        return (StatusCode::CONFLICT, "not seated in this loca").into_response();
    }
    hub.typing(&access.room, &identity.name, body.on);
    StatusCode::NO_CONTENT.into_response()
}
