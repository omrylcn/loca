//! Master Agent HTTP: the Master's switch and floor list, the Master Agents
//! it appointed, and the fence that keeps a Master Agent to its few routes.
use crate::{
    admin_token_of, hub, member_token_of, no_such_loca, session_of, HeaderMap, Hub, IntoResponse,
    Json, Path, State, StatusCode,
};

use crate::store::MasterAgentSetting;

fn viewer(hub: &Hub, headers: &HeaderMap) -> hub::RequestAuthority {
    hub.resolve_authority(admin_token_of(headers), session_of(headers))
}

fn refusal(error: hub::MasterAgentError) -> axum::response::Response {
    use hub::MasterAgentError::*;
    let status = match error {
        MasterOnly | TurnedOff | FloorNotDesignated => StatusCode::FORBIDDEN,
        LocasPerFloor | LocasPerHour => StatusCode::TOO_MANY_REQUESTS,
        NotPersistent | NameWorn => StatusCode::CONFLICT,
        Name(_) | NoSuchFloor | LimitOutOfRange => StatusCode::BAD_REQUEST,
        NotFound => StatusCode::NOT_FOUND,
        Storage => StatusCode::SERVICE_UNAVAILABLE,
    };
    (status, error.message()).into_response()
}

/// The setting as the Master's settings screen shows it, with its journal.
fn setting_answer(hub: &Hub, who: &hub::RequestAuthority) -> axum::response::Response {
    let setting = hub.master_agent_setting();
    match hub.master_agent_setting_log(who) {
        Ok(log) => Json(serde_json::json!({
            "on": setting.on,
            "floors": setting.floors,
            "limits": setting.limits,
            "limit_ceiling": crate::store::MasterAgentLimits::CEILING,
            "log": log,
        }))
        .into_response(),
        Err(error) => refusal(error),
    }
}

/// `GET /building/settings/master-agent` — Master only.
pub(crate) async fn get_master_agent_setting(
    State(hub): State<Hub>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let who = viewer(&hub, &headers);
    if !who.is_master() {
        return refusal(hub::MasterAgentError::MasterOnly);
    }
    setting_answer(&hub, &who)
}

/// `PUT /building/settings/master-agent {"on": bool, "floors"?: [id]}` —
/// Master only. Leaving `floors` out keeps the designated floors.
pub(crate) async fn put_master_agent_setting(
    State(hub): State<Hub>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let who = viewer(&hub, &headers);
    if !who.is_master() {
        return refusal(hub::MasterAgentError::MasterOnly);
    }
    let Some(on) = body.get("on").and_then(|v| v.as_bool()) else {
        return (StatusCode::BAD_REQUEST, "on must be true or false").into_response();
    };
    let floors = match body.get("floors") {
        None | Some(serde_json::Value::Null) => hub.master_agent_setting().floors,
        Some(serde_json::Value::Array(list)) => {
            let mut floors = Vec::new();
            for item in list {
                match item.as_str() {
                    Some(id) => floors.push(id.to_string()),
                    None => {
                        return (StatusCode::BAD_REQUEST, "floors: a list of floor ids")
                            .into_response()
                    }
                }
            }
            floors
        }
        Some(_) => return (StatusCode::BAD_REQUEST, "floors: a list of floor ids").into_response(),
    };
    // Each limit the body leaves out keeps the value in force.
    let mut limits = hub.master_agent_setting().limits;
    match body.get("limits") {
        None | Some(serde_json::Value::Null) => {}
        Some(serde_json::Value::Object(given)) => {
            for (key, value) in given {
                let slot = match key.as_str() {
                    "locas_per_floor" => &mut limits.locas_per_floor,
                    "locas_per_hour" => &mut limits.locas_per_hour,
                    "admissions_per_hour" => &mut limits.admissions_per_hour,
                    "live_agents" => &mut limits.live_agents,
                    _ => return (
                        StatusCode::BAD_REQUEST,
                        "limits: locas_per_floor, locas_per_hour, admissions_per_hour, live_agents",
                    )
                        .into_response(),
                };
                match value.as_u64().and_then(|n| u32::try_from(n).ok()) {
                    Some(n) => *slot = n,
                    None => return refusal(hub::MasterAgentError::LimitOutOfRange),
                }
            }
        }
        Some(_) => {
            return (
                StatusCode::BAD_REQUEST,
                "limits: an object of whole numbers",
            )
                .into_response()
        }
    }
    match hub.set_master_agent_setting(MasterAgentSetting { on, floors, limits }, &who) {
        Ok(_) => setting_answer(&hub, &who),
        Err(error) => refusal(error),
    }
}

/// `GET /master-agents` — Master only: every Master Agent ever appointed,
/// never a key.
pub(crate) async fn list_master_agents(
    State(hub): State<Hub>,
    headers: HeaderMap,
) -> impl IntoResponse {
    match hub.master_agents(&viewer(&hub, &headers)) {
        Ok(rows) => Json(rows).into_response(),
        Err(error) => refusal(error),
    }
}

/// `DELETE /master-agents/:id` — Master only. Its key, sessions and
/// principal end; the switch does not matter.
pub(crate) async fn revoke_master_agent_route(
    State(hub): State<Hub>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    match hub.revoke_master_agent(&id, &viewer(&hub, &headers)) {
        Ok(()) => (StatusCode::OK, "revoked").into_response(),
        Err(error) => refusal(error),
    }
}

/// `POST /keys {"kind": "master_agent", "name": …}` — the Master appoints a
/// Master Agent; refused while the Building switch is off. The key is in
/// this answer once and never again.
pub(crate) fn issue_master_agent_key(
    hub: &Hub,
    headers: &HeaderMap,
    body: &serde_json::Value,
) -> axum::response::Response {
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("");
    match hub.appoint_master_agent(name, &viewer(hub, headers)) {
        Ok((code, record)) => (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "kind": "master_agent",
                "id": record.id,
                "name": record.name,
                "code": code,
                "note": "shown once: give it to the automation's own secret store, never to a model or a chat",
            })),
        )
            .into_response(),
        Err(error) => refusal(error),
    }
}

/// The answer for a refused admission or call. A loca the Master Agent did
/// not open is answered as one that is not there.
pub(crate) fn admission_refusal(error: hub::AdmissionError) -> axum::response::Response {
    use hub::AdmissionError::*;
    let status = match &error {
        NotItsLoca => return super::access::no_such_loca(),
        Refused(_) | NotItsAgent => StatusCode::FORBIDDEN,
        NotAnAgent | BadClaim | Name(_) => StatusCode::BAD_REQUEST,
        Taken | Full => StatusCode::CONFLICT,
        LiveAgents | AdmissionsPerHour => StatusCode::TOO_MANY_REQUESTS,
        Storage => StatusCode::SERVICE_UNAVAILABLE,
    };
    (status, error.message()).into_response()
}

/// `POST /rooms/:id/admissions {"name", "claim_id", "kind"?}` — a Master
/// Agent admits an agent into a loca it opened: the member, its floor and
/// its seat, in one call. The key is in the answer (201; 200 when the same
/// claim collects it again) until the claim is acknowledged.
pub(crate) async fn admit_into_loca(
    State(hub): State<Hub>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let actor = viewer(&hub, &headers);
    if !Hub::is_master_agent(&actor) {
        return (StatusCode::FORBIDDEN, "only a Master Agent admits here").into_response();
    }
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let kind = body.get("kind").and_then(|v| v.as_str());
    let claim = body.get("claim_id").and_then(|v| v.as_str());
    match hub.master_agent_admit(&id, name, kind, claim, &actor) {
        Ok(admission) => {
            let status = if admission.created {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            };
            (status, Json(admission)).into_response()
        }
        Err(error) => admission_refusal(error),
    }
}

/// `POST /rooms/:id/admissions/:member_id/ack {"claim_id"}` — the Master
/// Agent stored the key; it is never handed out again.
pub(crate) async fn ack_admission(
    State(hub): State<Hub>,
    Path((id, member)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let actor = viewer(&hub, &headers);
    if !Hub::is_master_agent(&actor) {
        return (StatusCode::FORBIDDEN, "only a Master Agent admits here").into_response();
    }
    let claim = body.get("claim_id").and_then(|v| v.as_str()).unwrap_or("");
    match hub.master_agent_ack_admission(&id, &member, claim, &actor) {
        Ok(()) => Json(serde_json::json!({ "acknowledged": true })).into_response(),
        Err(error) => admission_refusal(error),
    }
}

/// What a Master Agent may reach. Anything else is refused before its
/// handler runs, so no existing route has to know the rank exists.
fn master_agent_may_call(method: &axum::http::Method, path: &str) -> bool {
    use axum::http::Method;
    let static_page = path == "/"
        || path.starts_with("/assets/")
        || path == "/PRINCIPLES.md"
        || path == "/PRINCIPLES.tr.md"
        || path == "/PRINCIPLES.en.md"
        || path == "/docs/getting-started.md"
        || path == "/health"
        || path == "/downloads/skills"
        || path.starts_with("/downloads/skills/");
    if *method == Method::GET && static_page {
        return true;
    }
    let loca_path = path
        .strip_prefix("/rooms/")
        .is_some_and(|rest| !rest.is_empty());
    match *method {
        // Its own sessions and who it is.
        Method::POST | Method::DELETE if path == "/sessions" => true,
        Method::GET if path == "/whoami" => true,
        // The loca listing and the socket: the door decides which locas.
        Method::GET if path == "/rooms" || path == "/ws" => true,
        // Reading a loca and speaking in it: the door decides which locas.
        Method::GET if loca_path => true,
        Method::POST if loca_path && path.ends_with("/messages") => path.matches('/').count() == 3,
        // Its acts: open a loca on a floor it was given, and set or clear
        // the lead of one it opened (the route asks which).
        Method::POST if path == "/rooms" => true,
        Method::POST if loca_path && path.ends_with("/lead") => path.matches('/').count() == 3,
        // Admitting agents into a loca it opened, and calling one it admitted.
        Method::POST if loca_path && path.ends_with("/admissions") => {
            path.matches('/').count() == 3
        }
        Method::POST if loca_path && path.ends_with("/ack") => {
            path.matches('/').count() == 5 && path.split('/').nth(3) == Some("admissions")
        }
        Method::POST if loca_path && path.ends_with("/call") => path.matches('/').count() == 3,
        _ => false,
    }
}

/// The Master Agent fence, outermost of the route layers. A request that is
/// not a Master Agent's passes untouched. A Master Agent's is refused with
/// one sentence while the Building switch is off, and otherwise reaches only
/// `master_agent_may_call`'s routes.
pub(crate) async fn master_agent_fence(
    State(hub): State<Hub>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    // Its key may arrive as the admin credential, as a session, or as the
    // room credential (`x-room-token`): each is the Master Agent, and each
    // is held to the same switch and short list.
    // On the socket the same credentials travel as subprotocols
    // (`loca.admin.`, `loca.room.`, `loca.session.`; ws.rs).
    let headers = req.headers();
    let socket = |prefix: &str| crate::ws::websocket_credential(headers, prefix);
    let (ws_admin, ws_room, ws_session) = (
        socket("loca.admin."),
        socket("loca.room."),
        socket("loca.session."),
    );
    // Only a Master Agent's own key (`ma_…`) is looked up from the room
    // credential and the socket's key subprotocols: resolving every other
    // request's key here would read it, and stamp its last use, once more.
    fn master_agent_key(key: Option<&str>) -> Option<&str> {
        key.filter(|k| k.starts_with("ma_"))
    }
    let candidates = [
        viewer(&hub, headers),
        hub.resolve_authority(master_agent_key(member_token_of(headers)), None),
        hub.resolve_authority(master_agent_key(ws_admin.as_deref()), ws_session.as_deref()),
        hub.resolve_authority(master_agent_key(ws_room.as_deref()), None),
    ];
    let authority = candidates
        .iter()
        .find(|a| Hub::is_master_agent(a))
        .cloned()
        .unwrap_or_else(|| candidates[0].clone());
    if !Hub::is_master_agent(&authority) {
        return next.run(req).await;
    }
    if !hub.master_agent_on() {
        return (StatusCode::FORBIDDEN, Hub::MASTER_AGENT_OFF).into_response();
    }
    if !master_agent_may_call(req.method(), req.uri().path()) {
        return (StatusCode::FORBIDDEN, "a Master Agent may not do this").into_response();
    }
    // A loca it did not open answers as one that is not there, whatever kind
    // of loca it is, so the answer tells it nothing about the Building.
    if let Some(room) = named_loca(req.uri()) {
        if !hub.master_agent_opened(&room, &authority) {
            return no_such_loca();
        }
    }
    next.run(req).await
}

/// The loca a request names: `/rooms/<id>/…` or the socket's `?room=`.
fn named_loca(uri: &axum::http::Uri) -> Option<String> {
    let path = uri.path();
    if let Some(rest) = path.strip_prefix("/rooms/") {
        let id = rest.split('/').next().unwrap_or_default();
        return (!id.is_empty()).then(|| percent_decoded(id, false));
    }
    if path == "/ws" {
        // Read as the socket handler reads its query (a map of decoded keys,
        // the last of a repeated key winning), so a repeated or encoded
        // `room` cannot make this check judge another loca than the one the
        // handler opens.
        return uri.query().and_then(|query| {
            query
                .split('&')
                .filter_map(|pair| {
                    let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
                    (percent_decoded(key, true) == "room").then(|| percent_decoded(value, true))
                })
                .next_back()
                .filter(|room| !room.is_empty())
        });
    }
    None
}

/// `%XX` decoded (and `+` as a space in a query value), as the router decodes
/// a path parameter and a query string before the handler sees them.
fn percent_decoded(text: &str, plus_is_space: bool) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                (Some(h), Some(l)) => {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                    continue;
                }
                _ => out.push(b'%'),
            },
            b'+' if plus_is_space => out.push(b' '),
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::master_agent_may_call;
    use axum::http::Method;

    #[test]
    fn a_master_agent_reaches_only_its_short_list() {
        let yes = [
            (Method::GET, "/health"),
            (Method::POST, "/sessions"),
            (Method::DELETE, "/sessions"),
            (Method::GET, "/whoami"),
            (Method::GET, "/rooms"),
            (Method::GET, "/ws"),
            (Method::GET, "/rooms/alpha/messages"),
            (Method::POST, "/rooms/alpha/messages"),
            (Method::POST, "/rooms"),
            (Method::POST, "/rooms/alpha/lead"),
            (Method::POST, "/rooms/alpha/admissions"),
            (Method::POST, "/rooms/alpha/admissions/mbid_x/ack"),
            (Method::POST, "/rooms/alpha/call"),
        ];
        for (method, path) in yes {
            assert!(master_agent_may_call(&method, path), "{method} {path}");
        }
        let no = [
            (Method::POST, "/rooms/alpha/messages/7/reactions"),
            (Method::POST, "/rooms/alpha/notes"),
            (Method::PUT, "/rooms/alpha/mode"),
            (Method::POST, "/rooms/alpha/notes/x/ack"),
            (Method::POST, "/rooms/alpha/care/x/ack"),
            (Method::POST, "/rooms/alpha/invites"),
            (Method::POST, "/rooms/alpha/operators"),
            (Method::POST, "/rooms/alpha/moderate"),
            (Method::PUT, "/rooms/alpha/settings"),
            (Method::POST, "/rooms/alpha/extra/lead"),
            (Method::POST, "/groups"),
            (Method::GET, "/groups"),
            (Method::POST, "/members"),
            (Method::GET, "/profiles"),
            (Method::POST, "/join-requests"),
            (Method::POST, "/keys"),
            (Method::GET, "/building/settings/master-agent"),
            (Method::GET, "/rooms/"),
            (Method::GET, "/birebirs"),
            (Method::PUT, "/profile"),
        ];
        for (method, path) in no {
            assert!(!master_agent_may_call(&method, path), "{method} {path}");
        }
    }
}
