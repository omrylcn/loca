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
    let authority = hub.resolve_authority(admin_token_of(&headers), session_of(&headers));
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
        if authority.is_master() {
            loca_roles.push("operator");
            operator_source = Some("inherited_master");
        } else if authority.building_role == Some(crate::store::BuildingRole::Smaster) {
            loca_roles.push("operator");
            operator_source = Some("inherited_smaster");
        } else if hub
            .loca_operator(room)
            .is_some_and(|assignment| assignment.principal_id == principal_id)
        {
            loca_roles.push("operator");
            operator_source = Some("appointed");
        }
        if hub.is_lead(room, display_name) {
            loca_roles.push("lead");
        }
        loca_roles.push("participant");
    }
    let session = session_of(&headers).and_then(|token| hub.session_identity(Some(token)));
    let current_credential_id =
        hub.credential_id_for_request(admin_token_of(&headers), session_of(&headers));
    Json(serde_json::json!({
        "principal": {
            "id": principal_id,
            "display_name": display_name,
            "kind": authority.kind,
        },
        "building_role": authority.building_role,
        "loca": room.map(|room| serde_json::json!({
            "room": room,
            "roles": loca_roles,
            "operator_source": operator_source,
        })),
        "session": session.map(|identity| serde_json::json!({
            "expires_at": identity.expires_at,
            "credential_id": current_credential_id,
            "bounded": true,
        })),
    }))
    .into_response()
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
                "id": hub.member_management_id(&member.token),
                "name": member.name,
                "kind": member.kind,
                "joined_at": member.joined_at,
                "admitted_by": member.admitted_by,
            })
        })
        .collect();
    Json(rows).into_response()
}

pub(crate) async fn list_profiles(State(hub): State<Hub>, headers: HeaderMap) -> impl IntoResponse {
    if !is_admin_req(&hub, &headers) {
        return (StatusCode::FORBIDDEN, "Building authority required").into_response();
    }
    Json(hub.profiles()).into_response()
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
    let kind = body.get("kind").and_then(|v| v.as_str()).unwrap_or("agent");
    // kind is part of the identity, so it is validated at the door, not
    // silently coerced — a bad kind is a bad request, not a quiet "agent".
    if kind != "agent" && kind != "user" {
        return (StatusCode::BAD_REQUEST, "kind must be 'agent' or 'user'").into_response();
    }
    let by = hub
        .smaster_name(admin_token_of(&headers))
        .map(|n| format!("smaster:{n}"))
        .unwrap_or_else(|| "master".into());
    match hub.admit_member(name, kind, &by) {
        Ok(member) => (StatusCode::CREATED, Json(member)).into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save member — try again",
        )
            .into_response(),
    }
}
/// End a membership.
pub(crate) async fn revoke_member_route(
    State(hub): State<Hub>,
    Path(token): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !is_admin_req(&hub, &headers) {
        return (StatusCode::UNAUTHORIZED, "master only").into_response();
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
    Json(hub.residents()).into_response()
}
/// The caretaker's least-privilege Building presence view.
///
/// A configured caretaker must be able to answer "who is connected?" even in
/// deployments that have no loca-dev runtime. Giving that runtime the master
/// key would also let it admit/revoke members and change every loca, which is
/// far broader than the job. Its own membership, davet, or live session is
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
    Json(hub.residents()).into_response()
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
        .map(|(token, name)| {
            serde_json::json!({ "id": hub.smaster_management_id(&token), "name": name })
        })
        .collect();
    Json(rows).into_response()
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
    match hub.add_smaster(name) {
        Ok(token) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "token": token, "name": name })),
        )
            .into_response(),
        Err(_) => (
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
/// What does the davet I am holding open? Lets a client file its davet under
/// the right loca instead of guessing — and tells a holder plainly when it has
/// been revoked.
pub(crate) async fn whoami(State(hub): State<Hub>, headers: HeaderMap) -> impl IntoResponse {
    if let Some(identity) = headers
        .get(SESSION_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|token| hub.session_identity(Some(token)))
    {
        return Json(serde_json::json!({
            "kind": "session",
            "name": identity.name,
            "member_kind": identity.kind,
            "loca": identity.loca,
            "admin": identity.admin,
        }))
        .into_response();
    }
    let token = member_token_of(&headers);
    // A membership (mb_) token answers "who am I" with the identity itself —
    // the building knows this person even before they take a seat. Checked
    // before the davet/building branches because it is the strongest answer.
    if let Some(m) = hub.member_for_credential(token) {
        let locas: Vec<String> = hub
            .invites_for_member(&m.token)
            .into_iter()
            .map(|i| i.room)
            .collect();
        return Json(serde_json::json!({
            "kind": "member",
            "name": m.name,
            "member_kind": m.kind,
            "locas": locas,
        }))
        .into_response();
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
/// Exchange a live loca davet for the permanent building membership it names.
///
/// This is the one-time onboarding bridge: the davet already proves exactly
/// which member is holding it, and the returned `mb_` credential can only wait
/// in the lobby. It does not open any loca. Keeping this credential lets
/// release end a seat without making the agent run setup again.
pub(crate) async fn claim_membership(
    State(hub): State<Hub>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(token) = member_token_of(&headers) else {
        return (StatusCode::UNAUTHORIZED, "a live davet is required").into_response();
    };
    let member = hub.member_for_credential(Some(token)).or_else(|| {
        let invite = hub.invite_by_token(token)?;
        hub.member_of(Some(&invite.member))
    });
    let Some(member) = member else {
        return (StatusCode::UNAUTHORIZED, "a live davet is required").into_response();
    };
    Json(serde_json::json!({
        "membership_token": member.token,
        "name": member.name,
        "kind": member.kind,
    }))
    .into_response()
}
pub(crate) async fn create_session_route(
    State(hub): State<Hub>,
    headers: HeaderMap,
    Json(body): Json<CreateSession>,
) -> impl IntoResponse {
    if body.name.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "empty name").into_response();
    }
    let paired_admin_ttl =
        pairing_code_of(&headers).and_then(|code| hub.consume_admin_pairing(code));
    let paired_admin = paired_admin_ttl.is_some();
    // A davet holder must be able to take an identity — the davet already says
    // who they are, so refusing here would leave the invited unable to speak.
    // A pairing code has explicit precedence if a stale davet field is also
    // present: one request must mint exactly one identity.
    let token = if paired_admin {
        None
    } else {
        member_token_of(&headers)
    };
    let supplied_admin_credential = admin_token_of(&headers);
    let credential_authority = hub.resolve_authority(supplied_admin_credential, None);
    let credential_principal = credential_authority.principal_id.is_some();
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
        && !hub.is_invite_token(token)
        && !credential_principal
    {
        return (StatusCode::UNAUTHORIZED, "davet required").into_response();
    }
    // A session inherits both the REACH and the IDENTITY of the davet that
    // opened the door: it sees that one loca, and it speaks as the davet's
    // member — not as whatever name the body claims (alice's davet cannot
    // mint a "bob"). A building key carries no member, so its session keeps
    // the body's name.
    let davet = token.and_then(|t| hub.invite_by_token(t));
    // Minted with a one-use master pairing code (or the legacy raw-key API
    // path)? Then it is an ADMIN session: it carries authority without making
    // the browser retain the root key, and it expires.
    let admin_grant = if let Some(ttl) = paired_admin_ttl {
        Some(hub.master_pairing_grant(ttl))
    } else if davet.is_none() && admin_token_of(&headers).is_some() {
        hub.admin_session_grant(admin_token_of(&headers), Hub::ADMIN_SESSION_TTL_MS)
    } else {
        None
    };
    match hub.create_session_scoped(body, davet.as_ref(), admin_grant) {
        Ok(session) => (StatusCode::CREATED, Json(session)).into_response(),
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
    if !is_master_req(&hub, &headers) {
        return (StatusCode::UNAUTHORIZED, "master required").into_response();
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
    if !is_master_req(&hub, &headers) {
        return (StatusCode::UNAUTHORIZED, "master required").into_response();
    }
    let (total, available) = hub.admission_stock_summary();
    Json(serde_json::json!({ "total": total, "available": available })).into_response()
}

#[derive(serde::Deserialize)]
pub(crate) struct CreateJoinRequest {
    name: String,
    kind: Option<String>,
}

fn valid_agent_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Best-effort client IP for the per-source join-request rate-limit. Behind our
/// single trusted reverse proxy (nginx on prod) the true client is the RIGHTMOST
/// `X-Forwarded-For` entry — nginx appends the real peer, so any earlier entries
/// are client-supplied and untrusted. With no proxy header we fall back to the
/// TCP peer. Without this, prod (where every peer is the nginx loopback) would
/// collapse the per-source limiter back into a global one (review re-blocker #3).
fn client_ip(headers: &HeaderMap, peer: std::net::SocketAddr) -> std::net::IpAddr {
    if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        if let Some(ip) = xff
            .split(',')
            .rev()
            .find_map(|s| s.trim().parse::<std::net::IpAddr>().ok())
        {
            return ip;
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
    if !valid_agent_name(name) {
        return (
            StatusCode::BAD_REQUEST,
            "name must be 1-64 ASCII letters, digits, dot, dash, or underscore",
        )
            .into_response();
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
        Some((status, _name, bootstrap_ready)) => {
            Json(serde_json::json!({ "status": status, "bootstrap_ready": bootstrap_ready }))
                .into_response()
        }
        None => (StatusCode::NOT_FOUND, "unknown request or secret").into_response(),
    }
}

/// POST /join-requests/:id/bootstrap — deliver the approved mb_ ONCE (secret in
/// the `x-join-secret` header).
pub(crate) async fn bootstrap_join_request_route(
    State(hub): State<Hub>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(secret) = join_secret_of(&headers) else {
        return (StatusCode::UNAUTHORIZED, "x-join-secret header required").into_response();
    };
    match hub.claim_join_request_bootstrap(&id, secret) {
        Some(mb) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "davet": mb })),
        )
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            "not approved, already delivered, or bad secret",
        )
            .into_response(),
    }
}

/// GET /join-requests — the Master's pending-request review list.
pub(crate) async fn list_join_requests_route(
    State(hub): State<Hub>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !is_master_req(&hub, &headers) {
        return (StatusCode::UNAUTHORIZED, "master required").into_response();
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
    if !is_master_req(&hub, &headers) {
        return (StatusCode::UNAUTHORIZED, "master required").into_response();
    }
    let by = hub
        .smaster_name(admin_token_of(&headers))
        .map(|n| format!("smaster:{n}"))
        .unwrap_or_else(|| "master".into());
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
    if !is_master_req(&hub, &headers) {
        return (StatusCode::UNAUTHORIZED, "master required").into_response();
    }
    // Record WHICH authority denied, matching the approve route's audit trail.
    let by = hub
        .smaster_name(admin_token_of(&headers))
        .map(|n| format!("smaster:{n}"))
        .unwrap_or_else(|| "master".into());
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
