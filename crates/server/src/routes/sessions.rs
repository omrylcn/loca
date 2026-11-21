//! Sessions: taking one with a credential or the Master's pairing code, and
//! ending it.

use crate::{
    admin_token_of, is_master_req, member_token_of, pairing_code_of, session_of, CreateSession,
    HashMap, HeaderMap, Hub, IntoResponse, Json, Query, State, StatusCode,
};

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
        && (hub.member_for_credential(token).is_some() || personal_authority.principal_id.is_some())
    {
        // Bind every credential that names a principal (a member's key, a
        // Smaster's, a Master Agent's, a profile credential) through
        // canonical authority, so the session is that principal's and the
        // body cannot name it. A principal's key used to fall through to an
        // anonymous session under any name when it was not a member's or a
        // Smaster's. Master pairing and anonymous/shared-key sandbox entry
        // stay separate.
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
