use crate::*;

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
        "inherited_master": hub.master_principal().map(|master| serde_json::json!({
            "principal_id": master.id,
            "display_name": master.display_name,
            "kind": master.kind,
            "building_role": "master",
            "source": "inherited",
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
    // You see the locas you may enter — no more. A davet opens one loca, so it
    // lists one; the master (or building key) reaches the building, so it lists
    // all. This uses the same door decision as every other room-scoped read.
    let session = headers
        .get(SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|t| hub.session_identity(Some(t)));
    let admin = admin_token_of(&headers).map(str::to_string);
    let member = member_token_of(&headers).map(str::to_string);
    Json(hub.room_summaries_for(|room| {
        hub.enter_decision(
            room,
            admin.as_deref(),
            member.as_deref(),
            session.as_ref(),
            None,
        )
        .is_allowed()
    }))
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
    // mod/sıra), not only the master.
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
    if !is_admin_req(&hub, &headers) {
        return (StatusCode::UNAUTHORIZED, "admin token required").into_response();
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
/// Issue a davet for one loca (master only). The token is minted here — the
/// master never types one in, so a davet cannot be guessed or hand-forged.
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
    // A davet seats an EXISTING member — it never creates identity. Admitting
    // to the building is its own act (POST /members); this door only hands a
    // known member the key to one loca.
    let Some(member) = hub.member_by_name(name) else {
        return (
            StatusCode::CONFLICT,
            "this agent does not belong to the building yet",
        )
            .into_response();
    };
    // Who signed this davet. The distinction matters at revoke time: a
    // smaster may not undo what the master decided.
    let issued_by = hub
        .smaster_name(admin_token_of(&headers))
        .map(|n| format!("smaster:{n}"))
        .unwrap_or_else(|| "master".into());
    match hub.invite_member_to_room(&member.token, &id, &issued_by) {
        Ok(inv) => (StatusCode::OK, Json(inv)).into_response(),
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
    }
}
/// Who currently holds a davet to this loca (master view).
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
                "id": hub.invite_management_id(&invite.token),
                "room": invite.room,
                "member_id": hub.member_management_id(&invite.member),
                "name": invite.name,
                "kind": invite.kind,
                "issued_at": invite.issued_at,
                "issued_by": invite.issued_by,
            })
        })
        .collect();
    (StatusCode::OK, Json(rows)).into_response()
}
/// End a davet (master only). The holder can no longer enter that loca.
pub(crate) async fn revoke_invite(
    State(hub): State<Hub>,
    Path((id, token)): Path<(String, String)>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !is_admin_req(&hub, &headers) {
        return (StatusCode::UNAUTHORIZED, "master only").into_response();
    }
    // A davet belongs to one loca; the URL names a loca. They must agree —
    // otherwise `DELETE /rooms/A/invites/<tokenB>` would revoke loca B's davet
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
    if !is_master_req(&hub, &headers) {
        let issued_by_master = hub
            .invite_by_token(&token)
            .map(|i| i.issued_by == "master")
            .unwrap_or(false);
        if issued_by_master {
            return (
                StatusCode::FORBIDDEN,
                "the master issued this davet — only the master ends it",
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
    match hub.moderate(&id, body.action, &body.name) {
        Ok(state) => (StatusCode::OK, Json(state)).into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save moderation — try again",
        )
            .into_response(),
    }
}
// ---- tasks: declared work (operator's signature births it) ----

/// May this request run an operator action in `room`? PRINCIPLES: a loca
/// operator (not only the master) runs their loca's mode/mute/moderation/lead.
/// The admin token is an operator everywhere; otherwise the session's name must
/// be on this loca's `operators` list. Identity comes from the session, never a
/// body claim — an operator act cannot be spoofed by typing a name.
pub(crate) fn is_room_operator_req(hub: &Hub, room: &str, headers: &HeaderMap) -> bool {
    // The master — by raw key OR admin session — operates every loca.
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
