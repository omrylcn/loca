//! One door for issuing codes, and the Master's ledger of every code issued.
use crate::*;

/// `GET /keys` — Master only: every code the Building has handed out, with
/// when it was issued, until when it can be used, whether it was used, and
/// what became of it. Never a secret, only a fingerprint.
pub(crate) async fn list_keys(State(hub): State<Hub>, headers: HeaderMap) -> impl IntoResponse {
    if !is_master_req(&hub, &headers) {
        return (StatusCode::FORBIDDEN, "only the Master sees the key ledger").into_response();
    }
    Json(hub.key_ledger()).into_response()
}

/// `POST /keys {"kind": "master" | "smaster" | "admission", ...}` — the one
/// generator. A Smaster code needs the real person's `name`; a Master code may
/// carry `ttl_hours` (session length); admission takes `count` and
/// `ttl_hours`. The secret is returned once, here, and recorded only as a
/// fingerprint.
pub(crate) async fn issue_key(
    State(hub): State<Hub>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if !is_master_req(&hub, &headers) {
        return (StatusCode::FORBIDDEN, "only the Master issues codes").into_response();
    }
    let ttl_hours = body.get("ttl_hours").and_then(|v| v.as_u64());
    match body.get("kind").and_then(|v| v.as_str()) {
        Some("master") => {
            let mut query = HashMap::new();
            if let Some(hours) = ttl_hours {
                query.insert("ttl_hours".to_string(), hours.to_string());
            }
            let ttl_ms = match pairing_ttl_ms(&query) {
                Ok(ttl) => ttl,
                Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
            };
            match hub.rotate_admin_pairing_for(ttl_ms) {
                Some((code, use_by)) => (
                    StatusCode::CREATED,
                    Json(serde_json::json!({
                        "kind": "master",
                        "code": code,
                        "use_by": use_by,
                        "session_hours": ttl_ms / 3_600_000,
                        "note": "works once; an earlier unused Master code stops working",
                    })),
                )
                    .into_response(),
                None => (StatusCode::CONFLICT, "this Building has no Master key").into_response(),
            }
        }
        Some("smaster") => {
            let name = body
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            // The name is optional: the person types their own at Smaster
            // sign-in, which renames them. Until then the key wears a neutral
            // placeholder and the Building shows it as an unused key.
            let placeholder;
            let name = if name.is_empty() {
                placeholder = format!("yeni-{:04x}", hub.now() & 0xffff);
                placeholder.as_str()
            } else {
                name
            };
            let name = match hub::validate_display_name(name) {
                Ok(name) => name,
                Err(_) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        "use the person's own name (one word, not a title like smaster)",
                    )
                        .into_response()
                }
            };
            if hub
                .smasters()
                .iter()
                .any(|(_, n)| n.eq_ignore_ascii_case(name))
                || hub
                    .profiles()
                    .iter()
                    .any(|p| p.display_name.eq_ignore_ascii_case(name))
            {
                return (StatusCode::CONFLICT, "someone already wears that name").into_response();
            }
            match hub.add_smaster(name) {
                Ok(token) => (
                    StatusCode::CREATED,
                    Json(serde_json::json!({
                        "kind": "smaster",
                        "name": name,
                        "code": token,
                        "use_by": hub.now().saturating_add(Hub::SMASTER_USE_MS),
                        "note": "unused for 7 days, it stops working; once used, it works until revoked",
                    })),
                )
                    .into_response(),
                Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "could not save — try again").into_response(),
            }
        }
        Some("admission") => {
            let count = body.get("count").and_then(|v| v.as_u64()).unwrap_or(5);
            if count == 0 || count > 100 {
                return (StatusCode::BAD_REQUEST, "count must be between 1 and 100")
                    .into_response();
            }
            let ttl_ms = ttl_hours.unwrap_or(24).clamp(1, 24 * 30) * 3_600_000;
            let (minted, total, available) = hub.mint_admission_stock(count as u32, ttl_ms);
            (
                StatusCode::CREATED,
                Json(serde_json::json!({
                    "kind": "admission",
                    "minted": minted,
                    "total": total,
                    "available": available,
                })),
            )
                .into_response()
        }
        _ => (
            StatusCode::BAD_REQUEST,
            "kind: master, smaster or admission",
        )
            .into_response(),
    }
}

/// `DELETE /keys/:id` — Master only.
pub(crate) async fn revoke_key_route(
    State(hub): State<Hub>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !is_master_req(&hub, &headers) {
        return (StatusCode::FORBIDDEN, "only the Master revokes codes").into_response();
    }
    match hub.revoke_key(&id) {
        Ok(()) => (StatusCode::OK, "revoked").into_response(),
        Err(hub::KeyRevokeError::NotFound) => {
            (StatusCode::NOT_FOUND, "no live code with that id").into_response()
        }
        Err(hub::KeyRevokeError::NotHere) => (
            StatusCode::CONFLICT,
            "members leave through the Building directory; admission rights expire on their own",
        )
            .into_response(),
        Err(hub::KeyRevokeError::Storage) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save — try again",
        )
            .into_response(),
    }
}

/// `PUT /smasters/:id {"name": "..."}` — the Master names a Smaster.
pub(crate) async fn rename_smaster_route(
    State(hub): State<Hub>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if !is_master_req(&hub, &headers) {
        return (StatusCode::FORBIDDEN, "only the Master renames a Smaster").into_response();
    }
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("");
    match hub.rename_smaster_by_master(&id, name) {
        Ok(principal) => {
            Json(serde_json::json!({ "name": principal.display_name })).into_response()
        }
        Err(hub::ProfileRenameError::AuthorityRequired) => {
            (StatusCode::NOT_FOUND, "no such Smaster").into_response()
        }
        Err(hub::ProfileRenameError::Name(_)) => (
            StatusCode::BAD_REQUEST,
            "use the person's own name (one word, not a title like smaster)",
        )
            .into_response(),
        Err(hub::ProfileRenameError::NameTaken) => {
            (StatusCode::CONFLICT, "someone already wears that name").into_response()
        }
        Err(hub::ProfileRenameError::RestrictionActive) => (
            StatusCode::FORBIDDEN,
            "a moderation restriction must be resolved before renaming this profile",
        )
            .into_response(),
        Err(hub::ProfileRenameError::Failed) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save — try again",
        )
            .into_response(),
    }
}
