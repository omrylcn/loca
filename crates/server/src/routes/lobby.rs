use crate::*;

/// Call somebody who is already in the building into this loca.
///
/// This is the lightweight everyday action: the master, looking at a loca,
/// says "bring them in". The server issues a persisted davet and delivers it
/// privately over the member's building-lobby socket.
pub(crate) async fn call_into_loca(
    State(hub): State<Hub>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if !is_admin_req(&hub, &headers) {
        return (StatusCode::UNAUTHORIZED, "only the master calls someone in").into_response();
    }
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if name.is_empty() {
        return (StatusCode::BAD_REQUEST, "who?").into_response();
    }
    // Calling in seats an EXISTING member — knowing someone and inviting them
    // to the table are different acts. An unknown name is not called in; it is
    // admitted first (the UI offers "Admit & invite" as two explicit steps).
    let Some(member) = hub.member_by_name(&name) else {
        return (
            StatusCode::NOT_FOUND,
            "not a building member — admit them first",
        )
            .into_response();
    };
    let issued_by = hub
        .smaster_name(admin_token_of(&headers))
        .map(|n| format!("smaster:{n}"))
        .unwrap_or_else(|| "master".into());
    match hub.invite_member_to_room(&member.token, &id, &issued_by) {
        Ok(inv) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "name": name, "loca": id, "token": inv.token })),
        )
            .into_response(),
        // Already seated here — say so plainly rather than minting a second
        // davet for the same person and loca.
        Err(hub::InviteError::AlreadyInvited) => (
            StatusCode::OK,
            Json(serde_json::json!({ "already": true, "name": name })),
        )
            .into_response(),
        Err(hub::InviteError::MemberNotFound) => (
            StatusCode::NOT_FOUND,
            "not a building member — admit them first",
        )
            .into_response(),
        Err(hub::InviteError::Full) => {
            (StatusCode::CONFLICT, "this loca is full — no free seat").into_response()
        }
        Err(hub::InviteError::Reserved) => (
            StatusCode::FORBIDDEN,
            "iye is reserved for master, smaster, loca-dev and loca-care",
        )
            .into_response(),
        Err(hub::InviteError::Storage) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save davet — try again",
        )
            .into_response(),
    }
}
/// Leave one's own loca without being kicked or banned. Identity is derived
/// only from the session/davet at the door; there is deliberately no `name`
/// body field to spoof. The invitation and live seat end, while building
/// membership survives — returning the member to the lobby roster.
pub(crate) async fn release_self_from_loca(
    State(hub): State<Hub>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    // Prefer the exact davet when both credentials are present. One identity
    // file can hold davets for several locas but only one current session; a
    // session for loca A must not make the valid davet for loca B unusable.
    let invite = member_token_of(&headers)
        .and_then(|token| hub.invite_for(&id, Some(token)))
        .or_else(|| {
            let identity = hub.session_identity(session_of(&headers))?;
            match (identity.member.as_deref(), identity.loca.as_deref()) {
                (Some(member), Some(loca)) if loca == id => hub
                    .invites_for_member(member)
                    .into_iter()
                    .find(|invite| invite.room == id),
                _ => None,
            }
        });
    let Some(invite) = invite else {
        return (
            StatusCode::UNAUTHORIZED,
            "your own davet/session for this loca is required",
        )
            .into_response();
    };
    match hub.revoke_invite(&invite.token) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "davet already ended").into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not release the seat — try again",
        )
            .into_response(),
    }
}
/// The building lobby is presence and routing, never a chat room. A permanent
/// membership credential opens only this socket. Calls are private to that
/// membership and carry the freshly persisted loca davet.
pub(crate) async fn lobby_ws_handler(
    State(hub): State<Hub>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    if q.contains_key("membership") && !legacy_ws_query_auth() {
        return (
            StatusCode::BAD_REQUEST,
            "WebSocket credentials belong in Sec-WebSocket-Protocol, not the URL",
        )
            .into_response();
    }
    let token = websocket_credential(&headers, "loca.membership.").or_else(|| {
        legacy_ws_query_auth()
            .then(|| q.get("membership").cloned())
            .flatten()
    });
    let Some(token) = token else {
        return (StatusCode::UNAUTHORIZED, "membership required").into_response();
    };
    let Some(member) = hub.member_for_credential(Some(&token)) else {
        return (StatusCode::UNAUTHORIZED, "invalid membership").into_response();
    };
    ws.protocols([WS_PROTOCOL])
        .on_upgrade(move |socket| lobby_ws_session(socket, hub, member))
        .into_response()
}
pub(crate) async fn lobby_ws_session(socket: WebSocket, hub: Hub, member: protocol::Membership) {
    use futures_util::{SinkExt, StreamExt};

    let mut calls = hub.subscribe_lobby();
    let (mut sink, mut stream) = socket.split();
    hub.lobby_join(&member.token);
    tracing::info!(name = %member.name, "lobby join");

    let invite_snapshot = hub.invites_for_member(&member.token);
    let ready = serde_json::json!({
        "t": "lobby_ready",
        "name": member.name,
        "invites": invite_snapshot.iter().map(|invite| serde_json::json!({
            "room": invite.room,
            "token": invite.token,
        })).collect::<Vec<_>>(),
    })
    .to_string();
    if sink.send(WsMessage::Text(ready)).await.is_err() {
        hub.lobby_leave(&member.token);
        return;
    }

    // Davets are persisted. Replaying the current set here closes the race
    // between a call and a reconnect/server restart.
    for invite in invite_snapshot {
        let frame = serde_json::json!({
            "t": "called",
            "room": invite.room,
            "token": invite.token,
        })
        .to_string();
        if sink.send(WsMessage::Text(frame)).await.is_err() {
            hub.lobby_leave(&member.token);
            return;
        }
    }

    let mut ping = tokio::time::interval(std::time::Duration::from_secs(WS_PING_SECS));
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ping.tick().await;

    loop {
        tokio::select! {
            event = calls.recv() => match event {
                Ok(event) if event.member() != member.token => {}
                Ok(LobbyEvent::Called { room, token, .. }) => {
                    let frame = serde_json::json!({
                        "t": "called",
                        "room": room,
                        "token": token,
                    }).to_string();
                    if sink.send(WsMessage::Text(frame)).await.is_err() {
                        break;
                    }
                }
                Ok(LobbyEvent::MembershipRevoked { .. }) => {
                    let frame = serde_json::json!({ "t": "membership_revoked" }).to_string();
                    let _ = sink.send(WsMessage::Text(frame)).await;
                    break;
                }
                Err(RecvError::Lagged(_)) => {
                    // The database-backed snapshot is authoritative; replay it
                    // after lag rather than trusting a lossy signal bus.
                    let mut failed = false;
                    for invite in hub.invites_for_member(&member.token) {
                        let frame = serde_json::json!({
                            "t": "called",
                            "room": invite.room,
                            "token": invite.token,
                        }).to_string();
                        if sink.send(WsMessage::Text(frame)).await.is_err() {
                            failed = true;
                            break;
                        }
                    }
                    if failed {
                        break;
                    }
                }
                Err(RecvError::Closed) => break,
            },
            _ = ping.tick() => {
                if sink.send(WsMessage::Ping(Vec::new())).await.is_err() {
                    break;
                }
            },
            incoming = stream.next() => match incoming {
                Some(Ok(WsMessage::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
        }
    }

    hub.lobby_leave(&member.token);
    tracing::info!(name = %member.name, "lobby leave");
}
