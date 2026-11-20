use crate::{hub, HashMap, HeaderMap, Hub, IntoResponse, Path, SenderType, State, StatusCode};

/// Middleware and handlers must authorize the same decoded room id. Return
/// malformed path parameters as an error rather than silently skipping a fence.
pub(super) async fn decoded_room_request(
    hub: &Hub,
    req: axum::extract::Request,
) -> Result<(axum::extract::Request, Option<String>), Box<axum::response::Response>> {
    let (mut parts, body) = req.into_parts();
    let Path(params) = <Path<HashMap<String, String>> as axum::extract::FromRequestParts<Hub>>::from_request_parts(
        &mut parts, hub,
    ).await.map_err(|error| Box::new(error.into_response()))?;
    Ok((
        axum::extract::Request::from_parts(parts, body),
        params.get("id").cloned(),
    ))
}

/// The one answer for a loca id that names nothing. A direct message or a group loca
/// the caller may not enter answers exactly this too, at every door (REST and
/// WebSocket), so neither one's existence leaks (PRINCIPLES "Birebir",
/// "Groups"). A person who mistyped a loca name learns that it is not there.
pub(crate) fn no_such_loca() -> axum::response::Response {
    (
        StatusCode::NOT_FOUND,
        "loca does not exist; create it first",
    )
        .into_response()
}

/// Does a closed Building's REST door tell this caller that `room` does not
/// exist? Either it names nothing (and was never sealed), or it names a
/// direct message or a group loca the caller may not enter. Both are decided here,
/// together, after the same work whichever is true: every lookup runs
/// whatever the id names, so neither the answer nor the time it takes tells
/// a hidden loca from nothing, and the direct message and group fences behind this
/// gate have no hidden loca left to refuse.
pub(crate) fn rest_sees_nothing(
    hub: &Hub,
    room: &str,
    headers: &HeaderMap,
    session: Option<&hub::SessionIdentity>,
) -> bool {
    let private = hub.private_loca_decision(
        room,
        admin_token_of(headers),
        member_token_of(headers),
        session,
    );
    let exists = hub.room_exists(room);
    let sealed = hub.is_deleted(room);
    (!exists && !sealed) || private.is_some_and(|decision| !decision.is_allowed())
}

/// The same question at the WebSocket door, asked before anything else
/// there reads the loca id: the id names nothing, or a direct message or group
/// loca the caller may not enter. (A group member the loca banned is still
/// answered 403 by the door decision after it, as before.)
pub(crate) fn socket_sees_nothing(
    hub: &Hub,
    room: &str,
    admin_tok: Option<&str>,
    invite_tok: Option<&str>,
    session: Option<&hub::SessionIdentity>,
) -> bool {
    let private = hub.private_loca_decision(room, admin_tok, invite_tok, session);
    let exists = hub.room_exists(room);
    !exists || private == Some(hub::EnterDecision::Denied)
}

/// Header carrying the admin token for admin-only actions.
pub(crate) const ADMIN_HEADER: &str = "x-admin-token";
/// Header carrying the room (join) token.
pub(crate) const ROOM_HEADER: &str = "x-room-token";
/// Header carrying a session token (server-derived identity).
pub(crate) const SESSION_HEADER: &str = "x-session-token";
/// Header carrying a one-use master-browser pairing code.
pub(crate) const PAIRING_HEADER: &str = "x-pairing-code";
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

/// A mutation's author is the authenticated session, never a body claim.
/// Only the explicitly session-optional sandbox accepts an absent session.
pub(crate) fn mutation_actor(
    hub: &Hub,
    headers: &HeaderMap,
    claimed: String,
    kind: SenderType,
) -> Result<(String, SenderType), StatusCode> {
    let token = session_of(headers);
    match hub.session_identity(token) {
        Some(identity) => Ok((identity.name, identity.kind)),
        None if headers.contains_key(SESSION_HEADER) || hub.require_sessions() => {
            Err(StatusCode::UNAUTHORIZED)
        }
        None => Ok((claimed, kind)),
    }
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
/// loca's mode/settings/bans/notes/tasks to any other loca's invite holder).
///
/// It runs the same single decision (`enter_decision`) and adds the tombstone
/// check, so a sealed loca is never reached through the front door either.
pub(crate) struct RoomAccess {
    pub(crate) room: String,
}

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
/// Paths readable WITHOUT a Building credential. Kept as a pure function so the
/// exact boundary is unit-testable — a sibling prefix (`/downloads/skillsevil`)
/// must NOT inherit a public prefix (`/downloads/skills`).
pub(crate) fn is_public_path(path: &str) -> bool {
    path == "/"
        || path.starts_with("/assets/")
        || path == "/PRINCIPLES.md"
        || path == "/PRINCIPLES.tr.md"
        || path == "/PRINCIPLES.en.md"
        // The Getting Started guide links this one doc for the full setup
        // walkthrough, so it must be publicly readable like PRINCIPLES (it is
        // not a Building credential). Exact match keeps every other /docs path
        // gated. It once 401'd on a closed prod building for lacking this entry.
        || path == "/docs/getting-started.md"
        || path == "/health"
        // The skill distribution bundles are public release artifacts — no
        // credential, no membership. An outside agent installs `loca` from here
        // BEFORE it has any identity, so gating this would break onboarding.
        // Match the EXACT path or a child, never a sibling prefix like
        // `/downloads/skillsevil`, which stays gated.
        || path == "/downloads/skills"
        || path.starts_with("/downloads/skills/")
        || path == "/sessions"
        || path == "/membership/claim"
        // The claim's ACK carries the same invite and claim id as the claim.
        || path == "/membership/claim/ack"
        // Join requests are authless BY DESIGN: an outsider creates one, polls it,
        // and bootstraps the issued mb_ with only the per-request secret — none of
        // which is a Building credential. The admin-only sub-routes (GET list,
        // approve, deny) still enforce `is_admin_req` (Master or Smaster) inside
        // their handlers, so waving the whole prefix past this membership gate is
        // safe.
        || path.starts_with("/join-requests")
        || path.starts_with("/ws")
        || path.starts_with("/lobby/ws")
}

pub(crate) async fn require_membership(
    State(hub): State<Hub>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let path = req.uri().path();
    if is_public_path(path) {
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
    // Kept: the loca door below decides with this same identity.
    let session = headers
        .get(SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|t| hub.session_identity(Some(t)));
    let has_session = session.is_some();
    // An invite is loca-scoped, so this blanket gate cannot judge it — it only
    // waves the holder through to the handler, whose `RoomAccess` extractor
    // knows which loca is being entered. Holding *some* invite gets you off the
    // street; it does not get you into any particular loca.
    let holds_invite = hub.is_invite_token(member_token_of(headers));
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
    // `/whoami` reject a perfectly healthy lobby-only agent in invite mode,
    // even though the handler itself knows how to describe that member.
    let holds_membership = hub
        .member_for_credential(member_token_of(headers))
        .is_some();
    if holds_membership
        || hub.is_member(member_token_of(headers))
        || has_session
        || holds_invite
        || has_principal_authority
        || is_legacy_smaster
    {
        // Closed Buildings create locas only through POST /rooms. Apply this to
        // every room route, including legacy admin handlers without RoomAccess.
        // The loopback sandbox deliberately retains its anonymous room model.
        if !hub.admin_open() && path.starts_with("/rooms/") {
            let (mut parts, body) = req.into_parts();
            let params = <Path<HashMap<String, String>> as axum::extract::FromRequestParts<Hub>>::from_request_parts(&mut parts, &hub).await;
            let Ok(Path(params)) = params else {
                return (StatusCode::BAD_REQUEST, "invalid loca path").into_response();
            };
            if params
                .get("id")
                .is_some_and(|room| rest_sees_nothing(&hub, room, &parts.headers, session.as_ref()))
            {
                return no_such_loca();
            }
            return next
                .run(axum::extract::Request::from_parts(parts, body))
                .await;
        }
        // The sandbox opens a loca the first time a route names it.
        if hub.admin_open() && path.starts_with("/rooms/") {
            let (mut parts, body) = req.into_parts();
            let params = <Path<HashMap<String, String>> as axum::extract::FromRequestParts<Hub>>::from_request_parts(&mut parts, &hub).await;
            if let Some(refusal) = params
                .ok()
                .and_then(|Path(params)| params.get("id").cloned())
                .and_then(|room| super::implicit_loca_refusal(&hub, &room))
            {
                return refusal;
            }
            return next
                .run(axum::extract::Request::from_parts(parts, body))
                .await;
        }
        return next.run(req).await;
    }
    // The member's own live key, refused only because the Master suspended
    // the membership: say so (`/whoami` included), as the Lobby door does.
    if hub.credential_of_suspended_member(member_token_of(headers)) {
        return (StatusCode::FORBIDDEN, "membership suspended").into_response();
    }
    (StatusCode::UNAUTHORIZED, "davet required").into_response()
}

#[cfg(test)]
mod same_work;

#[cfg(test)]
mod tests {
    use super::is_public_path;

    #[test]
    fn skill_download_prefix_is_exact_not_sibling() {
        // The exact index and its children are public.
        assert!(is_public_path("/downloads/skills"));
        assert!(is_public_path("/downloads/skills/loca"));
        assert!(is_public_path("/downloads/skills/loca/manifest"));
        // A SIBLING prefix must NOT inherit the public prefix.
        assert!(!is_public_path("/downloads/skillsevil"));
        assert!(!is_public_path("/downloads/skills-private"));
        assert!(!is_public_path("/downloads"));
        assert!(!is_public_path("/downloads/other"));
        // Unrelated gated routes stay gated.
        assert!(!is_public_path("/rooms"));
        // Existing public paths remain public.
        assert!(is_public_path("/health"));
        assert!(is_public_path("/docs/getting-started.md"));
        assert!(is_public_path("/assets/app.js"));
    }
}
