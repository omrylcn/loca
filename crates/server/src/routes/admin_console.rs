//! The master desk on its own port (`ADMIN_CONSOLE_PORT`): its page and the
//! actions it takes on the in-process Hub.

use crate::{
    hub, pairing_ttl_ms, routes, HashMap, HeaderMap, Html, Hub, IntoResponse, Json, Query, State,
    StatusCode, NAME_WORN_RULE,
};

pub(crate) const ADMIN_INDEX: &str = include_str!("../../../../web/admin.html");
pub(crate) const ADMIN_CONSOLE_HEADER: &str = "x-loca-console";
/// The SSH-forward-only master desk. Every asset is inline so opening the
/// tunnel is sufficient; it never fetches code from a third party.
pub(crate) async fn admin_console_index() -> impl IntoResponse {
    (
        [
            ("cache-control", "no-store"),
            ("x-frame-options", "DENY"),
            ("referrer-policy", "no-referrer"),
            (
                "content-security-policy",
                "default-src 'self'; connect-src 'self'; img-src 'self' data:; \
                 style-src 'unsafe-inline'; script-src 'unsafe-inline'; \
                 frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
            ),
        ],
        Html(ADMIN_INDEX),
    )
}

/// Mutations use a non-simple request header. A hostile public web page cannot
/// blindly POST to localhost through an operator's open SSH tunnel: its CORS
/// preflight receives no permission. The tunnel remains the actual authority.
pub(crate) fn admin_console_write_allowed(headers: &HeaderMap) -> bool {
    headers
        .get(ADMIN_CONSOLE_HEADER)
        .and_then(|v| v.to_str().ok())
        == Some("1")
}

pub(crate) async fn admin_console_state(State(hub): State<Hub>) -> impl IntoResponse {
    Json(serde_json::json!({
        "locas": hub.room_summaries_for(|room| !hub.is_dm(room)),
        // Resident deliberately omits the mb_ membership token. The browser
        // needs names and loca occupancy, never identity credentials.
        "residents": hub.residents(),
        "server": std::env::var("PUBLIC_SERVER_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8787".into()),
    }))
}

pub(crate) async fn admin_console_admit(
    State(hub): State<Hub>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if !admin_console_write_allowed(&headers) {
        return (StatusCode::FORBIDDEN, "master desk request required").into_response();
    }
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if name.is_empty() {
        return (StatusCode::BAD_REQUEST, "agent name required").into_response();
    }
    // The rule is for a name being admitted now, decided by
    // `Hub::admit_member` under the lock it writes the name with. A member
    // admitted before the rule keeps answering here: this door finds it again.
    let kind = body.get("kind").and_then(|v| v.as_str()).unwrap_or("agent");
    if kind != "agent" && kind != "user" {
        return (StatusCode::BAD_REQUEST, "kind must be agent or user").into_response();
    }
    match hub.admit_member(name, kind, "master-console") {
        Ok(member) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "name": member.name, "kind": member.kind })),
        )
            .into_response(),
        Err(hub::AdmitError::Rule(rule)) => (StatusCode::BAD_REQUEST, rule).into_response(),
        Err(hub::AdmitError::Worn) => (StatusCode::CONFLICT, NAME_WORN_RULE).into_response(),
        Err(hub::AdmitError::Storage) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save building member",
        )
            .into_response(),
    }
}

pub(crate) async fn admin_console_invite(
    State(hub): State<Hub>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if !admin_console_write_allowed(&headers) {
        return (StatusCode::FORBIDDEN, "master desk request required").into_response();
    }
    let loca = body
        .get("loca")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if loca.is_empty() || name.is_empty() {
        return (StatusCode::BAD_REQUEST, "loca and agent are required").into_response();
    }
    // In the loopback sandbox an invite opens its loca; a name the rule
    // refuses is answered as such, not as a loca to be created first (which
    // would be refused too).
    if let Some(refusal) = routes::implicit_loca_refusal(&hub, loca) {
        return refusal;
    }
    let Some(member) = hub.member_by_name(name) else {
        return (
            StatusCode::CONFLICT,
            "agent is not a building member — add them first",
        )
            .into_response();
    };
    match hub.invite_member_to_room(&member.id, loca, "master") {
        Ok((invite, invite_key)) => (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "token": invite_key,
                "loca": invite.room,
                "name": invite.name,
                "kind": invite.kind,
            })),
        )
            .into_response(),
        Err(hub::InviteError::AlreadyInvited) => (
            StatusCode::CONFLICT,
            "agent already has a davet for this loca",
        )
            .into_response(),
        Err(hub::InviteError::MemberNotFound) => (
            StatusCode::CONFLICT,
            "agent is not a building member — add them first",
        )
            .into_response(),
        Err(hub::InviteError::Full) => {
            (StatusCode::CONFLICT, "loca is full — release a seat first").into_response()
        }
        Err(hub::InviteError::Reserved) => (
            StatusCode::FORBIDDEN,
            "iye is reserved for master, smaster and loca-care",
        )
            .into_response(),
        Err(hub::InviteError::Storage) => {
            (StatusCode::SERVICE_UNAVAILABLE, "could not save davet").into_response()
        }
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

pub(crate) async fn admin_console_pairing(
    State(hub): State<Hub>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !admin_console_write_allowed(&headers) {
        return (StatusCode::FORBIDDEN, "master desk request required").into_response();
    }
    let ttl_ms = match pairing_ttl_ms(&query) {
        Ok(ttl) => ttl,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };
    match hub.rotate_admin_pairing_for(ttl_ms) {
        Some((pairing_code, pairing_expires_at)) => (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "pairing_code": pairing_code,
                "session_ttl_hours": ttl_ms / (60 * 60 * 1000),
                "pairing_expires_at": pairing_expires_at,
            })),
        )
            .into_response(),
        None => (StatusCode::CONFLICT, "ADMIN_TOKEN is not configured").into_response(),
    }
}

/// Master desk: pending join requests + the current admission-stock summary in a
/// single read, so the console renders the approval queue and the stock gauge
/// from one poll. Read-only (the console port is loopback/SSH-forwarded).
pub(crate) async fn admin_console_join_requests(State(hub): State<Hub>) -> impl IntoResponse {
    let pending: Vec<_> = hub
        .list_pending_join_requests()
        .into_iter()
        .map(|(id, name, kind, created_at)| {
            serde_json::json!({ "id": id, "name": name, "kind": kind, "created_at": created_at })
        })
        .collect();
    let (total, available) = hub.admission_stock_summary();
    Json(serde_json::json!({
        "pending": pending,
        "stock": { "total": total, "available": available },
    }))
    .into_response()
}

/// Master desk: approve a pending join request. Consumes exactly one admission
/// right and issues the Lobby membership atomically (see `approve_join_request`).
pub(crate) async fn admin_console_approve_join_request(
    State(hub): State<Hub>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    if !admin_console_write_allowed(&headers) {
        return (StatusCode::FORBIDDEN, "master desk request required").into_response();
    }
    match hub.approve_join_request(&id, "master-console") {
        crate::hub::Approve::Approved => (
            StatusCode::OK,
            Json(serde_json::json!({ "status": "approved" })),
        )
            .into_response(),
        crate::hub::Approve::AlreadyDecided => {
            (StatusCode::CONFLICT, "already decided").into_response()
        }
        crate::hub::Approve::NameTaken => (
            StatusCode::CONFLICT,
            "that name already exists — choose another",
        )
            .into_response(),
        crate::hub::Approve::NameRule(rule) => (StatusCode::CONFLICT, rule).into_response(),
        crate::hub::Approve::NameWorn => (StatusCode::CONFLICT, NAME_WORN_RULE).into_response(),
        crate::hub::Approve::NoStock => {
            (StatusCode::CONFLICT, "no admission stock — mint some first").into_response()
        }
        crate::hub::Approve::Failed => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not approve — try again",
        )
            .into_response(),
    }
}

/// Master desk: deny a pending join request.
pub(crate) async fn admin_console_deny_join_request(
    State(hub): State<Hub>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    if !admin_console_write_allowed(&headers) {
        return (StatusCode::FORBIDDEN, "master desk request required").into_response();
    }
    if hub.deny_join_request(&id, "master-console") {
        (
            StatusCode::OK,
            Json(serde_json::json!({ "status": "denied" })),
        )
            .into_response()
    } else {
        (StatusCode::CONFLICT, "already decided or not found").into_response()
    }
}

/// Master desk: pre-mint single-use, time-limited admission rights (each one is
/// consumed by exactly one approval). ttl_hours defaults to 24, bounded 1..2160.
pub(crate) async fn admin_console_mint_stock(
    State(hub): State<Hub>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if !admin_console_write_allowed(&headers) {
        return (StatusCode::FORBIDDEN, "master desk request required").into_response();
    }
    let count = body.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
    if count == 0 || count > 100 {
        return (StatusCode::BAD_REQUEST, "count must be between 1 and 100").into_response();
    }
    let hours = body.get("ttl_hours").and_then(|v| v.as_u64()).unwrap_or(24);
    let Some(ttl_ms) = hours.checked_mul(60 * 60 * 1000) else {
        return (StatusCode::BAD_REQUEST, "ttl_hours is too large").into_response();
    };
    if !(60 * 60 * 1000..=90 * 24 * 60 * 60 * 1000).contains(&ttl_ms) {
        return (
            StatusCode::BAD_REQUEST,
            "ttl_hours must be between 1 and 2160 (90 days)",
        )
            .into_response();
    }
    // The Master desk acts as the Master.
    let (minted, total, available) = hub.mint_admission_stock(count as u32, ttl_ms, None);
    (
        StatusCode::CREATED,
        Json(serde_json::json!({ "minted": minted, "total": total, "available": available })),
    )
        .into_response()
}
