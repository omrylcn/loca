use crate::{
    hub, member_token_of, session_of, HeaderMap, Hub, IntoResponse, Json, State, StatusCode,
    SESSION_HEADER,
};

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

/// Why a new member's name that someone already wears in some letter case
/// (`Hub::new_member_name_clashes`) is refused, at every admission door, with
/// 409 as the join-request door answers a name that is taken.
pub(crate) const NAME_WORN_RULE: &str =
    "someone in this Building already has that name in some letter case — choose another";

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
