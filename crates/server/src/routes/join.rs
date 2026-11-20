//! Ways into the Building: admission rights and join requests, from filing to
//! approval.

use crate::{is_admin_req, HeaderMap, Hub, IntoResponse, Json, State, StatusCode, NAME_WORN_RULE};

use super::profile::request_principal;

#[derive(serde::Deserialize)]
pub(crate) struct MintAdmissionStock {
    count: u32,
    ttl_hours: Option<u64>,
}

#[derive(serde::Deserialize)]
pub(crate) struct CreateJoinRequest {
    name: String,
    kind: Option<String>,
}

// The doors' name rule (`valid_agent_name`, `admission_name_refusal`) lives
// in the hub, which decides it where the name is written.
pub(crate) use crate::hub::admission_name_refusal;

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
    let minter = request_principal(&hub, &headers);
    let (minted, total, available) =
        hub.mint_admission_stock(body.count, ttl_ms, minter.as_deref());
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
