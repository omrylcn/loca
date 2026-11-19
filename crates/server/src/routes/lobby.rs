use crate::*;

/// Call somebody who is already in the building into this loca.
///
/// This is the lightweight everyday action: the master, looking at a loca,
/// says "bring them in". The server issues a persisted invite and delivers it
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
    // In the loopback sandbox a call opens its loca; a name the rule refuses
    // is answered as such, not as a loca to be created first.
    if let Some(refusal) = super::implicit_loca_refusal(&hub, &id) {
        return refusal;
    }
    // `reissue: true` replaces the invite the member already holds here with a
    // fresh one. Absent (or null), this is the ordinary call.
    let reissue = match body.get("reissue") {
        None | Some(serde_json::Value::Null) => false,
        Some(serde_json::Value::Bool(reissue)) => *reissue,
        Some(_) => {
            return (StatusCode::BAD_REQUEST, "reissue must be true or false").into_response()
        }
    };
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
    let authority = hub.resolve_authority(admin_token_of(&headers), session_of(&headers));
    if reissue {
        return match hub.reissue_invite(&member.id, &id, &authority) {
            // The same answer a first call gives: the fresh invite goes back
            // to the caller here and to the member over its Lobby line.
            Ok((_, invite)) => (
                StatusCode::CREATED,
                Json(serde_json::json!({ "name": name, "loca": id, "token": invite })),
            )
                .into_response(),
            Err(hub::ReissueError::NotHeld) => (
                StatusCode::NOT_FOUND,
                "holds no davet for this loca — nothing to re-send; call them instead",
            )
                .into_response(),
            // The refusal the revoke route gives: re-issuing ends an invite.
            Err(hub::ReissueError::Forbidden) => (
                StatusCode::FORBIDDEN,
                "cannot end a davet issued by a higher or unknown authority",
            )
                .into_response(),
            Err(hub::ReissueError::Invite(error)) => call_refused(error),
        };
    }
    match hub.invite_member_to_room(&member.id, &id, &authority.invitation_issuer()) {
        Ok((_, invite)) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "name": name, "loca": id, "token": invite })),
        )
            .into_response(),
        // Already seated here — say so plainly rather than minting a second
        // invite for the same person and loca.
        Err(hub::InviteError::AlreadyInvited) => (
            StatusCode::OK,
            Json(serde_json::json!({ "already": true, "name": name })),
        )
            .into_response(),
        Err(error) => call_refused(error),
    }
}

/// What a call (or a re-issue) answers when the invite cannot be issued.
fn call_refused(error: hub::InviteError) -> axum::response::Response {
    match error {
        hub::InviteError::AlreadyInvited => {
            (StatusCode::CONFLICT, "already holds a davet for this loca").into_response()
        }
        hub::InviteError::MemberNotFound => (
            StatusCode::NOT_FOUND,
            "not a building member — admit them first",
        )
            .into_response(),
        hub::InviteError::Full => {
            (StatusCode::CONFLICT, "this loca is full — no free seat").into_response()
        }
        hub::InviteError::Reserved => (
            StatusCode::FORBIDDEN,
            "iye is reserved for master, smaster, loca-dev and loca-care",
        )
            .into_response(),
        hub::InviteError::Storage => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save davet — try again",
        )
            .into_response(),
        hub::InviteError::MissingRoom => (
            StatusCode::NOT_FOUND,
            "create the loca before issuing a davet",
        )
            .into_response(),
        hub::InviteError::Sealed => {
            (StatusCode::CONFLICT, "this loca is permanently sealed").into_response()
        }
        hub::InviteError::NotInGroup => (
            StatusCode::FORBIDDEN,
            "not a member of this loca's group — add them to the group first",
        )
            .into_response(),
        hub::InviteError::PrivatePair => (
            StatusCode::FORBIDDEN,
            "a birebir admits only its two people",
        )
            .into_response(),
        hub::InviteError::Lobby => (
            StatusCode::CONFLICT,
            "every member is already in the Lobby — it takes no davet",
        )
            .into_response(),
    }
}
/// Leave one's own loca without being kicked or banned. Identity is derived
/// only from the session/invite at the door; there is deliberately no `name`
/// body field to spoof. The invitation and live seat end, while building
/// membership survives — returning the member to the lobby roster.
pub(crate) async fn release_self_from_loca(
    State(hub): State<Hub>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    // Prefer the exact invite when both credentials are present. One identity
    // file can hold invites for several locas but only one current session; a
    // session for loca A must not make the valid invite for loca B unusable.
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
    match hub.revoke_invite_by_hash(&invite.secret_hash) {
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
/// membership and carry the freshly persisted loca invite.
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
        // Suspension is reversible, so the holder of the member's own live
        // key is told so and can wait for Resume. Anything else (unknown,
        // revoked, a revoked membership) gets the one answer it got before.
        if hub.credential_of_suspended_member(Some(&token)) {
            return (StatusCode::FORBIDDEN, "membership suspended").into_response();
        }
        return (StatusCode::UNAUTHORIZED, "invalid membership").into_response();
    };
    // The credential this socket authenticated with: revoking it at the
    // profile door closes this socket and no other (door B). `None` in a
    // memory-only Building, which has no credential records.
    let credential_id = hub.credential_id_for_secret(&token);
    // Display only, never authority (hub/kit_version.rs).
    let kit = hub::kit_version::from_query(&q);
    ws.protocols([WS_PROTOCOL])
        .on_upgrade(move |socket| lobby_ws_session(socket, hub, member, token, credential_id, kit))
        .into_response()
}

/// `{"t":"seats"}`'s list, also `lobby_ready.seats`: the loca and invite id
/// (`ivid_`) of every seat the member holds. The invites themselves are not
/// in it; a kit enters a seat from the invite it keeps when the id matches.
fn seat_list(hub: &Hub, member: &str) -> Vec<serde_json::Value> {
    hub.seats_of(member)
        .into_iter()
        .map(|(room, invite_id)| serde_json::json!({ "room": room, "invite_id": invite_id }))
        .collect()
}

/// A `called` frame: the invite for one loca, sent live at a call and again
/// on a connect while its delivery is owed. `invite_id` is the invite's
/// public id, so a client can match it to a seat without hashing.
fn called_frame(hub: &Hub, room: &str, token: &str) -> String {
    serde_json::json!({
        "t": "called",
        "room": room,
        "token": token,
        "invite_id": hub.invite_management_id(token),
    })
    .to_string()
}

/// Whether the credential a Lobby socket opened with still opens it for the
/// same member. Re-checked where an event could have been missed: right
/// after subscribing (a revoke racing the handshake) and after the bus
/// lagged.
enum LobbyCredential {
    Live,
    /// Revoked at the profile door: say so, then close.
    Revoked,
    /// The membership ended or moved: close; the reconnect is refused.
    Gone,
}

/// `member` is the membership's id the socket opened as.
fn lobby_credential(hub: &Hub, presented: &str, member: &str) -> LobbyCredential {
    if hub.credential_is_revoked(presented) {
        return LobbyCredential::Revoked;
    }
    match hub.member_for_credential(Some(presented)) {
        Some(current) if current.id == member => LobbyCredential::Live,
        _ => LobbyCredential::Gone,
    }
}

/// How a pass over the owed deliveries ended.
enum OwedPass {
    /// Everything owed at each step was sent.
    Done,
    /// The socket is gone.
    SocketClosed,
    /// The credential stopped opening this Lobby while the pass ran.
    Credential(LobbyCredential),
}

/// Send the invites whose delivery is still owed, one `called` each. Nothing
/// is decided ahead of its own frame: before every send the connection's
/// credential is checked again and the hub is asked afresh what is owed, so
/// an invite that was revoked, re-issued or used while an earlier frame was
/// waiting on the socket is not sent. Used on connect and after a lag.
async fn send_owed_deliveries<S>(sink: &mut S, hub: &Hub, presented: &str, member: &str) -> OwedPass
where
    S: futures_util::Sink<WsMessage> + Unpin,
{
    use futures_util::SinkExt;
    let mut sent = std::collections::HashSet::new();
    loop {
        match lobby_credential(hub, presented, member) {
            LobbyCredential::Live => {}
            ended => return OwedPass::Credential(ended),
        }
        let Some((invite, invite_key)) = hub.next_owed_delivery(member, &sent) else {
            return OwedPass::Done;
        };
        let frame = called_frame(hub, &invite.room, &invite_key);
        sent.insert(invite.secret_hash);
        if sink.send(WsMessage::Text(frame.into())).await.is_err() {
            return OwedPass::SocketClosed;
        }
    }
}

fn credential_revoked_frame(credential_id: Option<&str>) -> String {
    match credential_id {
        Some(id) => serde_json::json!({ "t": "credential_revoked", "credential_id": id }),
        None => serde_json::json!({ "t": "credential_revoked" }),
    }
    .to_string()
}

pub(crate) async fn lobby_ws_session(
    socket: WebSocket,
    hub: Hub,
    member: crate::store::MemberRecord,
    presented: String,
    credential_id: Option<String>,
    kit: Option<hub::kit_version::KitVersion>,
) {
    use futures_util::{SinkExt, StreamExt};

    let mut calls = hub.subscribe_lobby();
    // Notifications are addressed by principal; the binding behind a
    // membership never changes, so take it once.
    let principal = member.principal_id.clone();
    // Presence is counted by the membership's id, never by its key.
    let member_id = member.id.clone();
    let (mut sink, mut stream) = socket.split();
    let kit_log = kit.as_ref().map(ToString::to_string);
    let _kit = hub.note_lobby_kit_version(&member_id, kit);
    hub.lobby_join(&member_id);
    tracing::info!(name = %member.name, kit_version = ?kit_log, "lobby join");

    // Subscribed: a revoke from now on arrives as an event. One that landed
    // between the handshake and the subscription is caught here.
    match lobby_credential(&hub, &presented, &member_id) {
        LobbyCredential::Live => {}
        LobbyCredential::Revoked => {
            let frame = credential_revoked_frame(credential_id.as_deref());
            let _ = sink.send(WsMessage::Text(frame.into())).await;
            leave_lobby_and_close(&hub, &member_id, &mut sink, &mut stream).await;
            return;
        }
        LobbyCredential::Gone => {
            leave_lobby_and_close(&hub, &member_id, &mut sink, &mut stream).await;
            return;
        }
    }

    // The seat list, never the invites: a kit enters a seat from the invite it
    // keeps. There is deliberately no `invites` key: an older kit takes that
    // list as the whole truth and deletes every kept invite missing from it;
    // without the key it asks /whoami, which drops only dead ones.
    let ready = serde_json::json!({
        "t": "lobby_ready",
        "name": member.name,
        "member_id": member_id,
        "seats": seat_list(&hub, &member_id),
    })
    .to_string();
    if sink.send(WsMessage::Text(ready.into())).await.is_err() {
        hub.lobby_leave(&member_id);
        return;
    }

    // The invites whose delivery is still owed (minted for delivery, not yet
    // used, inside their window) go out again on every connect, the same
    // invite each time: a call made while this member was away, or a frame
    // lost with a dropped socket, arrives now. Nothing is minted or revoked.
    match send_owed_deliveries(&mut sink, &hub, &presented, &member_id).await {
        OwedPass::Done => {}
        OwedPass::Credential(LobbyCredential::Revoked) => {
            let frame = credential_revoked_frame(credential_id.as_deref());
            let _ = sink.send(WsMessage::Text(frame.into())).await;
            leave_lobby_and_close(&hub, &member_id, &mut sink, &mut stream).await;
            return;
        }
        OwedPass::SocketClosed | OwedPass::Credential(_) => {
            leave_lobby_and_close(&hub, &member_id, &mut sink, &mut stream).await;
            return;
        }
    }

    let mut ping = tokio::time::interval(std::time::Duration::from_secs(WS_PING_SECS));
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ping.tick().await;

    loop {
        tokio::select! {
            event = calls.recv() => match event {
                Ok(LobbyEvent::Notification { recipients, frame }) => {
                    // Older kits ignore an unknown `t`; the persisted list
                    // (`GET /notifications`) stays the authority after lag.
                    if principal.as_deref().is_some_and(|p| recipients.contains(p))
                        && sink.send(WsMessage::Text(frame.as_ref().clone().into())).await.is_err()
                    {
                        break;
                    }
                }
                Ok(LobbyEvent::CredentialRevoked { credential_id: revoked }) => {
                    // Door B: only the sockets opened with this credential.
                    if credential_id.as_deref() == Some(revoked.as_str()) {
                        let frame = credential_revoked_frame(Some(&revoked));
                        let _ = sink.send(WsMessage::Text(frame.into())).await;
                        break;
                    }
                }
                Ok(event) if event.member() != Some(member_id.as_str()) => {}
                // An invite that ended before this socket got to forward its
                // call (revoked, released or re-issued since) is not sent: a
                // client keeps the last invite it is called with, and that
                // must be the one the server holds.
                Ok(LobbyEvent::Called { room, token, .. })
                    if !hub.call_is_current(&member_id, &room, &token) => {}
                Ok(LobbyEvent::Called { room, token, .. }) => {
                    let frame = called_frame(&hub, &room, &token);
                    if sink.send(WsMessage::Text(frame.into())).await.is_err() {
                        break;
                    }
                }
                Ok(LobbyEvent::MembershipRevoked { .. }) => {
                    let frame = serde_json::json!({ "t": "membership_revoked" }).to_string();
                    let _ = sink.send(WsMessage::Text(frame.into())).await;
                    break;
                }
                Ok(LobbyEvent::MembershipSuspended { .. }) => {
                    // Do not send membership_revoked: older listeners erase their
                    // locally held membership on permanent revocation only.
                    let frame = serde_json::json!({ "t": "membership_suspended" }).to_string();
                    let _ = sink.send(WsMessage::Text(frame.into())).await;
                    break;
                }
                Err(RecvError::Lagged(_)) => {
                    // Events were lost. A revoke among them is re-checked
                    // here; then the seat list and the deliveries still owed
                    // are re-sent from the store's mirror, never every invite.
                    match lobby_credential(&hub, &presented, &member_id) {
                        LobbyCredential::Live => {}
                        LobbyCredential::Revoked => {
                            let frame = credential_revoked_frame(credential_id.as_deref());
                            let _ = sink.send(WsMessage::Text(frame.into())).await;
                            break;
                        }
                        LobbyCredential::Gone => break,
                    }
                    let seats = serde_json::json!({
                        "t": "seats",
                        "seats": seat_list(&hub, &member_id),
                    }).to_string();
                    if sink.send(WsMessage::Text(seats.into())).await.is_err() {
                        break;
                    }
                    match send_owed_deliveries(&mut sink, &hub, &presented, &member_id).await {
                        OwedPass::Done => {}
                        OwedPass::Credential(LobbyCredential::Revoked) => {
                            let frame = credential_revoked_frame(credential_id.as_deref());
                            let _ = sink.send(WsMessage::Text(frame.into())).await;
                            break;
                        }
                        OwedPass::SocketClosed | OwedPass::Credential(_) => break,
                    }
                }
                Err(RecvError::Closed) => break,
            },
            _ = ping.tick() => {
                if sink.send(WsMessage::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            },
            incoming = stream.next() => match incoming {
                Some(Ok(WsMessage::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
        }
    }

    leave_lobby_and_close(&hub, &member_id, &mut sink, &mut stream).await;
    tracing::info!(name = %member.name, "lobby leave");
}

/// The end of a Lobby socket, whatever ended it: answer with a close frame,
/// leave the Lobby, then read what the peer had already sent. Without the
/// close frame the kit sees a broken connection instead of an ended one; and
/// a socket closed over unread bytes (a pong on its way) is answered by the
/// operating system with a reset, which can reach the peer instead of the
/// last frame written (`credential_revoked`, `membership_revoked`).
async fn leave_lobby_and_close<S, R>(hub: &Hub, member_id: &str, sink: &mut S, stream: &mut R)
where
    S: futures_util::Sink<WsMessage> + Unpin,
    R: futures_util::Stream<Item = Result<WsMessage, axum::Error>> + Unpin,
{
    use futures_util::SinkExt;
    let _ = sink.send(WsMessage::Close(None)).await;
    hub.lobby_leave(member_id);
    drain_before_close(stream).await;
}
