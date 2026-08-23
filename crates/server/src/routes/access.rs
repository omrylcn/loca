use crate::*;

pub(crate) fn valid_identity_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}
pub(crate) fn admin_token_of(headers: &HeaderMap) -> Option<&str> {
    headers.get(ADMIN_HEADER).and_then(|v| v.to_str().ok())
}
pub(crate) fn session_of(headers: &HeaderMap) -> Option<&str> {
    headers.get(SESSION_HEADER).and_then(|v| v.to_str().ok())
}
pub(crate) fn pairing_code_of(headers: &HeaderMap) -> Option<&str> {
    headers.get(PAIRING_HEADER).and_then(|v| v.to_str().ok())
}
/// Does this request carry Building-admin authority? Compatibility requests
/// may present the root/bootstrap credential directly; normal clients use a
/// bounded principal session instead. This keeps the recovery credential in
/// the server environment during everyday browser use.
pub(crate) fn is_admin_req(hub: &Hub, headers: &HeaderMap) -> bool {
    hub.resolve_authority(admin_token_of(headers), session_of(headers))
        .is_building_admin()
}
/// Same, but for master-only acts (a smaster cannot undo the master). The admin
/// session inherits Master rank only when it authenticates the Master
/// principal. Smaster credentials and sessions remain below that boundary.
pub(crate) fn is_master_req(hub: &Hub, headers: &HeaderMap) -> bool {
    hub.resolve_authority(admin_token_of(headers), session_of(headers))
        .is_master()
}
/// Membership token from a request: prefer the room token, fall back to the
/// admin token (admins are members).
pub(crate) fn member_token_of(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(ROOM_HEADER)
        .and_then(|v| v.to_str().ok())
        .or_else(|| admin_token_of(headers))
}
/// The one door for room-scoped routes. Extracting it IS the authorization:
/// a handler that takes `RoomAccess` only runs when the caller may enter this
/// loca, so a route can never forget its gate (the copied `may_enter_room`
/// `if`s were exactly that failure — six read routes had none, leaking one
/// loca's mode/settings/bans/notes/tasks to any other loca's davet holder).
///
/// It runs the same single decision (`enter_decision`) and adds the tombstone
/// check, so a sealed loca is never reached through the front door either.
pub(crate) struct RoomAccess {
    pub(crate) room: String,
}

#[axum::async_trait]
impl axum::extract::FromRequestParts<Hub> for RoomAccess {
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        hub: &Hub,
    ) -> Result<Self, Self::Rejection> {
        // The loca id is the first path segment param `id`. Every room-scoped
        // route is `/rooms/:id/...`, so this is always present.
        let Path(params) = Path::<HashMap<String, String>>::from_request_parts(parts, hub)
            .await
            .map_err(|_| (StatusCode::BAD_REQUEST, "missing loca id"))?;
        let room = params
            .get("id")
            .cloned()
            .ok_or((StatusCode::BAD_REQUEST, "missing loca id"))?;

        // A sealed loca is gone — not "unauthorized," but not there.
        if hub.is_deleted(&room) {
            return Err((StatusCode::NOT_FOUND, "this loca no longer exists"));
        }

        let headers = &parts.headers;
        let session = headers
            .get(SESSION_HEADER)
            .and_then(|v| v.to_str().ok())
            .and_then(|t| hub.session_identity(Some(t)));
        match hub.enter_decision(
            &room,
            admin_token_of(headers),
            member_token_of(headers),
            session.as_ref(),
            None,
        ) {
            hub::EnterDecision::Allowed => Ok(RoomAccess { room }),
            hub::EnterDecision::Banned => Err((StatusCode::FORBIDDEN, "banned from this loca")),
            hub::EnterDecision::Denied => {
                Err((StatusCode::UNAUTHORIZED, "davet required for this loca"))
            }
        }
    }
}
/// Membership gate for the whole API — reading is as private as writing.
///
/// A room's history, notes, tasks and roster are the group's; a room token
/// that only guards writes protects nothing (anyone could read every word).
/// Public by design: `/` (the client shell, which then asks for the key),
/// `/health` (needs_token discovery) and `/sessions` (how you exchange a room
/// token for an identity). `/ws` carries its token in the query and checks
/// itself.
pub(crate) async fn require_membership(
    State(hub): State<Hub>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let path = req.uri().path();
    let public = path == "/"
        || path.starts_with("/assets/")
        || path == "/PRINCIPLES.md"
        || path == "/PRINCIPLES.en.md"
        || path == "/health"
        || path == "/sessions"
        || path == "/membership/claim"
        || path.starts_with("/ws")
        || path.starts_with("/lobby/ws");
    if public {
        return next.run(req).await;
    }
    let headers = req.headers();
    let supplied_admin_credential = admin_token_of(headers);
    let supplied_authority = hub.resolve_authority(supplied_admin_credential, session_of(headers));
    if supplied_admin_credential.is_some()
        && supplied_authority.principal_id.is_none()
        && !supplied_authority.is_building_admin()
    {
        return (StatusCode::UNAUTHORIZED, "credential revoked or unknown").into_response();
    }
    if headers
        .get(ROOM_HEADER)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|credential| hub.credential_is_revoked(credential))
    {
        return (StatusCode::UNAUTHORIZED, "credential revoked").into_response();
    }
    // A session token also proves membership (it was issued against one).
    let has_session = headers
        .get(SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|t| hub.session_identity(Some(t)).is_some())
        .unwrap_or(false);
    // A davet is loca-scoped, so this blanket gate cannot judge it — it only
    // waves the holder through to the handler, whose `RoomAccess` extractor
    // knows which loca is being entered. Holding *some* davet gets you off the
    // street; it does not get you into any particular loca.
    let holds_davet = hub.is_invite_token(member_token_of(headers));
    // A smaster acts with the master's reach, so the building key is not their
    // door — their own key is. Without this they are turned away at the street
    // before any handler can recognise them.
    let has_principal_authority = hub
        .resolve_authority(admin_token_of(headers), session_of(headers))
        .principal_id
        .is_some();
    let is_legacy_smaster = hub.smaster_name(admin_token_of(headers)).is_some();
    // A permanent mb_ credential is a building membership too. `is_member`
    // below is the legacy building-key check; using only that check made
    // `/whoami` reject a perfectly healthy lobby-only agent in davet mode,
    // even though the handler itself knows how to describe that member.
    let holds_membership = hub
        .member_for_credential(member_token_of(headers))
        .is_some();
    if holds_membership
        || hub.is_member(member_token_of(headers))
        || has_session
        || holds_davet
        || has_principal_authority
        || is_legacy_smaster
    {
        return next.run(req).await;
    }
    (StatusCode::UNAUTHORIZED, "davet required").into_response()
}
