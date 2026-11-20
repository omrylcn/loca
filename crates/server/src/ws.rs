//! The room WebSocket: `GET /ws`, its credentials and filters, the
//! per-connection session loop and the frames it sends.

use crate::*;

pub(crate) const WS_PROTOCOL: &str = "loca.v1";

pub(crate) fn websocket_credential(headers: &HeaderMap, prefix: &str) -> Option<String> {
    headers
        .get_all(SEC_WEBSOCKET_PROTOCOL)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .find_map(|protocol| protocol.strip_prefix(prefix).map(str::to_string))
        .filter(|value| !value.is_empty())
}

pub(crate) fn legacy_ws_query_auth() -> bool {
    matches!(
        std::env::var("LEGACY_WS_QUERY_AUTH").as_deref(),
        Ok("1") | Ok("true")
    )
}

pub(crate) const MAX_WS_MESSAGE_BYTES: usize = 64 * 1024;

pub(crate) async fn ws_handler(
    State(hub): State<Hub>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let room = q
        .get("room")
        .cloned()
        .unwrap_or_else(|| hub.home_room().to_string());
    let mut name = q.get("name").cloned().unwrap_or_else(|| "anon".into());
    let query_has_credentials =
        q.contains_key("admin") || q.contains_key("token") || q.contains_key("session");
    if query_has_credentials && !legacy_ws_query_auth() {
        return (
            StatusCode::BAD_REQUEST,
            "WebSocket credentials belong in Sec-WebSocket-Protocol, not the URL",
        )
            .into_response();
    }
    let legacy = legacy_ws_query_auth();
    let credentials = WsCredentials {
        admin: websocket_credential(&headers, "loca.admin.")
            .or_else(|| legacy.then(|| q.get("admin").cloned()).flatten()),
        invite: websocket_credential(&headers, "loca.room.")
            .or_else(|| legacy.then(|| q.get("token").cloned()).flatten()),
        session: websocket_credential(&headers, "loca.session.")
            .or_else(|| legacy.then(|| q.get("session").cloned()).flatten()),
    };

    // Resolve the session FIRST: it is a way in on its own (the browser holds
    // one and no invite), so judging the door before reading it would turn
    // every seated operator away.
    let session_identity = hub.session_identity(credentials.session.as_deref());
    if credentials.session.is_some() && session_identity.is_none() {
        return (StatusCode::UNAUTHORIZED, "invalid session token").into_response();
    }
    // A closed Building answers an id that names nothing and a direct message or
    // group loca this caller may not enter alike, decided here, together,
    // before anything else reads the id, after the same work whichever is
    // true (PRINCIPLES "Birebir", "Groups").
    if !hub.admin_open()
        && socket_sees_nothing(
            &hub,
            &room,
            credentials.admin.as_deref(),
            credentials.invite.as_deref(),
            session_identity.as_ref(),
        )
    {
        return no_such_loca();
    }

    // The WS door: the SAME single decision the REST gate uses. Bearers arrive
    // as WebSocket subprotocol credentials; ?name= is only the ban hint when
    // there is no authoritative session or invite identity yet.
    match hub.enter_decision(
        &room,
        credentials.admin.as_deref(),
        credentials.invite.as_deref(),
        session_identity.as_ref(),
        q.get("name").map(String::as_str),
    ) {
        hub::EnterDecision::Allowed => {}
        hub::EnterDecision::Banned => {
            return (StatusCode::FORBIDDEN, "banned from this room").into_response();
        }
        hub::EnterDecision::Denied => {
            if hub.is_dm(&room) || hub.is_group_loca(&room) || !hub.room_exists(&room) {
                return no_such_loca();
            }
            return (StatusCode::UNAUTHORIZED, "davet required for this loca").into_response();
        }
    }
    if !hub.admin_open() && !hub.room_exists(&room) {
        return no_such_loca();
    }
    if let Some(refusal) = routes::implicit_loca_refusal(&hub, &room) {
        return refusal;
    }
    // A session OR invite names its holder authoritatively. Query-string names
    // are only a hint for open/legacy doors; allowing an invite to enter under an
    // alias meant revocation kicked the recorded holder while the alias socket
    // stayed alive.
    let invite_identity = credentials
        .invite
        .as_deref()
        .and_then(|token| hub.invite_for(&room, Some(token)));
    let member_identity = credentials
        .invite
        .as_deref()
        .and_then(|token| hub.member_for_credential(Some(token)));
    if let Some(idy) = &session_identity {
        name = idy.name.clone();
    } else if let Some(invite) = &invite_identity {
        name = invite.name.clone();
    } else if let Some(member) = &member_identity {
        name = member.name.clone();
    }
    let kind = match &session_identity {
        Some(idy) => idy.kind,
        None => match invite_identity.as_ref().map(|invite| invite.kind.as_str()) {
            Some("agent") => SenderType::Agent,
            Some(_) => SenderType::User,
            None => match member_identity.as_ref().map(|member| member.kind.as_str()) {
                Some("agent") => SenderType::Agent,
                Some(_) => SenderType::User,
                None => match q.get("type").map(String::as_str) {
                    Some("agent") => SenderType::Agent,
                    _ => SenderType::User,
                },
            },
        },
    };
    // ?filter= controls what the server pushes down this connection:
    //   (none) -> everything (msg + typing + members + history + control)
    //   msg      -> only real chat messages (drops typing/members/history noise)
    //   mentions -> only messages addressing this client (target==name/all, or
    //               @name / @all in the text) — the client wakes only when
    //               spoken to. Server-side so ANY client (web/mobile/bot) gets
    //               it, not just Claude Code.
    let filter = match q.get("filter").map(String::as_str) {
        Some("msg") => WsFilter::Messages,
        Some("mentions") => WsFilter::Mentions,
        _ => WsFilter::All,
    };
    // Addressed agent messages can be coalesced into one runtime turn. The
    // loca owns the defaults; explicit query values are a per-runtime
    // compatibility/tuning override. Clamp hostile values to a small range.
    let room_settings = hub.settings(&room);
    let turn_max = q
        .get("turn_max")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(room_settings.turn_max_messages as usize)
        .clamp(1, 16);
    let turn_idle_ms = q
        .get("turn_idle_ms")
        .or_else(|| q.get("turn_wait_ms"))
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(room_settings.turn_idle_ms as u64)
        .clamp(100, 30_000);
    let turn_max_wait_ms = q
        .get("turn_max_wait_ms")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(room_settings.turn_max_wait_ms as u64)
        .clamp(turn_idle_ms, 60_000);
    // ?watch=1 — listen without taking a seat: the connection receives frames
    // but never joins the roster, so the room's presence stays the group's.
    // This is how the loca agent can hear its name anywhere while remaining a
    // member of its one private maintenance loca alone.
    let mut watch_only = matches!(q.get("watch").map(String::as_str), Some("1") | Some("true"));
    // The caretaker is bound to one configured PRIVATE loca. Anywhere else it
    // may only listen — never take a seat — so neither the lobby nor another
    // group's loca becomes its room. Enforced server-side.
    let is_caretaker = hub.is_caretaker(&name);
    if is_caretaker && room != hub.home_room() {
        watch_only = true;
    }
    // Admin authority for this connection (gates `control` frames) comes from
    // the admin/session WebSocket credential. The browser holds the short-lived
    // admin session, not the raw key. Open when no ADMIN_TOKEN is configured.
    // The seat key — one key = one seat. Derived from the same credentials the
    // door decision read, so the same key under two display names lands on ONE
    // seat instead of two ghosts.
    let identity = hub.seat_identity(
        credentials.admin.as_deref(),
        credentials.invite.as_deref(),
        session_identity.as_ref(),
        credentials.session.as_deref(),
        &name,
    );
    // A loca socket taken with an invite is that invite's use: its delivery
    // copy, if still held, ends once the socket is open.
    let invite_used = invite_identity.map(|invite| invite.secret_hash);
    // Display only, never authority (hub/kit_version.rs).
    let kit = hub::kit_version::from_query(&q);
    ws.protocols([WS_PROTOCOL])
        .max_message_size(MAX_WS_MESSAGE_BYTES)
        .max_frame_size(MAX_WS_MESSAGE_BYTES)
        .on_upgrade(move |socket| {
            if let Some(hash) = &invite_used {
                hub.note_invite_used_by_hash(hash);
            }
            ws_session(
                socket,
                hub,
                room,
                identity,
                name,
                kind,
                filter,
                turn_max,
                turn_idle_ms,
                turn_max_wait_ms,
                credentials,
                watch_only,
                is_caretaker,
                kit,
            )
        })
        .into_response()
}

/// Bearers used at the WebSocket door. They stay attached to the connection so
/// logout, expiry, smaster revoke, and invite revoke take effect after the
/// handshake too; authorization is not a forever-valid snapshot.
#[derive(Clone)]
pub(crate) struct WsCredentials {
    admin: Option<String>,
    invite: Option<String>,
    session: Option<String>,
}

impl WsCredentials {
    fn live_session(&self, hub: &Hub) -> Option<hub::SessionIdentity> {
        hub.session_identity(self.session.as_deref())
    }

    fn allowed(&self, hub: &Hub, room: &str, name: &str) -> bool {
        let session = self.live_session(hub);
        if self.session.is_some() && session.is_none() {
            return false;
        }
        matches!(
            hub.enter_decision(
                room,
                self.admin.as_deref(),
                self.invite.as_deref(),
                session.as_ref(),
                Some(name),
            ),
            hub::EnterDecision::Allowed
        )
    }

    fn is_admin(&self, hub: &Hub) -> bool {
        hub.is_admin(self.admin.as_deref()) || hub.is_admin_session(self.session.as_deref())
    }
}

/// What a WS connection wants pushed to it.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum WsFilter {
    All,
    Messages,
    Mentions,
}

/// Does message `m` address participant `name`? (target or @-mention of name/all)
///
/// An explicit `@all` or `target=all` addresses every identity seated in this
/// loca, including its caretakers. System announcements remain passive context:
/// they must not manufacture a model turn merely because their target is all.
/// `all_in_text` is false in the Lobby room, where only `target=all` calls
/// everyone: the server keeps it on a rank's call alone
/// (`Hub::settle_lobby_call`).
pub(crate) fn msg_addresses(
    m: &protocol::Message,
    name: &str,
    accepts_all: bool,
    all_in_text: bool,
) -> bool {
    let all_is_a_call = accepts_all && m.kind != protocol::MessageKind::Announce;
    if m.target.as_deref() == Some(name) {
        return true;
    }
    // A reply addresses the author of the message it answers (resolved by the
    // server), separately from any explicit target, so replying wakes them.
    if m.reply_to_sender.as_deref() == Some(name) {
        return true;
    }
    if all_is_a_call && m.target.as_deref() == Some("all") {
        return true;
    }
    // word-boundary @name or @all, case-insensitive
    let text = m.text.to_lowercase();
    let needle_name = format!("@{}", name.to_lowercase());
    for tok in text.split(|c: char| !(c.is_alphanumeric() || c == '@' || c == '-' || c == '_')) {
        if tok == needle_name {
            return true;
        }
        if all_is_a_call && all_in_text && tok == "@all" {
            return true;
        }
    }
    false
}

/// Server-side keepalive so an idle connection isn't closed (was seen as 1006).
// Runtime readiness expires after 20 seconds.  Ping often enough that a native
// listener can prove the socket is still carrying traffic before renewing its
// wake lease; a 30-second ping left an impossible gap between those contracts.
pub(crate) const WS_PING_SECS: u64 = 10;
/// How long a socket the server is closing keeps reading what its peer had
/// already sent, so the final frame is not lost to a TCP reset.
pub(crate) const WS_CLOSE_DRAIN: std::time::Duration = std::time::Duration::from_secs(1);

/// One WS connection: replay history, then fan broadcast frames down to the
/// client while accepting optional `send`/`control` frames coming up.
/// `events_only` suppresses non-message frames (see `?filter=msg`).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn ws_session(
    socket: WebSocket,
    hub: Hub,
    room: String,
    identity: String,
    name: String,
    kind: SenderType,
    filter: WsFilter,
    turn_max: usize,
    turn_idle_ms: u64,
    turn_max_wait_ms: u64,
    credentials: WsCredentials,
    watch_only: bool,
    is_caretaker: bool,
    kit: Option<hub::kit_version::KitVersion>,
) {
    use futures_util::{SinkExt, StreamExt};

    let events_only = filter != WsFilter::All; // msg or mentions -> no history/members
                                               // Identify this session so we can tell our own eviction broadcast apart
                                               // from a later one aimed at us.
    let session_id = hub.next_session_id();
    // The session's AUTHENTICATED principal, when it took its session with an
    // invite/credential. Everyone per-member reminders are delivered and replayed
    // by this id, never by the connection's display name — so a shared name can
    // never cross wires and a principal-less socket never receives one.
    let session_principal_id = credentials
        .live_session(&hub)
        .and_then(|session| session.principal_id)
        .or_else(|| hub.principal_id_for_invite(&room, credentials.invite.as_deref()));
    // Who this socket's messages are written by, as verified at the door, in
    // the order its display name was chosen: the session, else the invite or
    // membership credential, else the Smaster code that named the seat. A
    // rename follows this id. An open door or the root key records nobody.
    let sender_principal = match credentials.live_session(&hub) {
        Some(session) => session.principal_id,
        None => hub
            .principal_id_for_invite(&room, credentials.invite.as_deref())
            .or_else(|| {
                hub.member_for_credential(credentials.invite.as_deref())
                    .and_then(|member| member.principal_id)
            })
            .or_else(|| {
                credentials
                    .invite
                    .is_none()
                    .then(|| identity.strip_prefix("sm:pr:").map(str::to_string))
                    .flatten()
            }),
    };
    // The key its `op_id`s are remembered under: the same one REST uses for
    // this sender, so a re-send over either transport lands once.
    let operation_scope = {
        let session = credentials.live_session(&hub);
        hub.operation_scope(
            &room,
            credentials.admin.as_deref(),
            credentials.invite.as_deref(),
            session.as_ref(),
            kind,
            &name,
        )
    };
    // `@all` calls ordinary room members everywhere. A caretaker receives it
    // only at its private home table; a cross-loca watch must never turn a
    // room-wide call into access to that room's discussion.
    let accepts_all = !is_caretaker || room == hub.home_room();
    // In the Lobby room `@all` in the words calls nobody; `target=all` does,
    // and only a rank's message keeps it (`Hub::settle_lobby_call`).
    let in_lobby = hub.is_lobby_room(&room);
    let (mut sink, mut stream) = socket.split();
    let (mut rx, history) = hub.subscribe(&room);
    let mut replayed_care_ids = std::collections::HashSet::new();
    if !watch_only {
        hub.prove_seat(&room, &identity, sender_principal.as_deref());
    }
    let kit_log = kit.as_ref().map(ToString::to_string);
    let _kit = (!watch_only)
        .then(|| hub.note_seat_kit_version(&room, &identity, session_id, kit))
        .flatten();
    if !watch_only && !hub.join(&room, &identity, &name, kind, session_id) {
        // The loca is full. Say so rather than dropping silently — and the
        // door stays open for watchers, who take no seat.
        tracing::info!(%room, %name, "loca full — refused");
        let _ = send_frame(
            &mut sink,
            &ServerFrame::Control {
                cmd: format!(
                    "loca full ({} seats). ask the master for a seat, or watch with ?watch=1",
                    hub.room_capacity(&room)
                ),
            },
        )
        .await;
        return;
    }
    tracing::info!(%room, %name, ?kind, kit_version = ?kit_log, "ws join");

    // Memory is server-pushed rather than client-fetched. Send it for every
    // seated connection, including absent and empty state, before history so
    // a freshly connected agent starts with an explicit memory envelope.
    if !watch_only {
        let memory = match hub.loca_memory_snapshot(&room) {
            Ok(memory) => memory,
            Err(error) => {
                tracing::warn!(%room, %name, %error, "could not read loca memory");
                if !watch_only {
                    hub.leave(&room, &identity, session_id);
                }
                return;
            }
        };
        if send_frame(&mut sink, &ServerFrame::memory(&room, memory))
            .await
            .is_err()
        {
            hub.leave(&room, &identity, session_id);
            return;
        }
    }

    // Care signals are a durable outbox, not a best-effort broadcast. Replay
    // anything this identity has not transport-ACKed; the listener ACKs only
    // after writing its durable inbox. Subscribe happened first, so remember
    // ids and suppress a raced live copy from `rx` below.
    for signal in hub.pending_care_scoped(&room, &name, session_principal_id.as_deref()) {
        // Invariant (P0#2): a Care envelope is only ever placed on a socket
        // whose room matches signal.room. pending_care already guarantees this,
        // but never put a mismatched envelope on the wire even if a legacy or
        // divergent outbox row slips through.
        if signal.room != room {
            continue;
        }
        replayed_care_ids.insert(signal.id.clone());
        if send_frame(&mut sink, &ServerFrame::Care { signal })
            .await
            .is_err()
        {
            if !watch_only {
                hub.leave(&room, &identity, session_id);
            }
            return;
        }
    }

    // Initial backlog for the joiner (skipped in events-only mode).
    if !events_only {
        if send_frame(&mut sink, &ServerFrame::History { messages: history })
            .await
            .is_err()
        {
            if !watch_only {
                hub.leave(&room, &identity, session_id);
            }
            return;
        }
        // Current roster right away (join broadcast may race the subscribe).
        let _ = send_frame(
            &mut sink,
            &ServerFrame::Members {
                members: hub.members(&room),
            },
        )
        .await;
    }

    let mut ping = tokio::time::interval(std::time::Duration::from_secs(WS_PING_SECS));
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ping.tick().await; // consume the immediate first tick

    // Set when a newer connection takes this seat over. The seat then belongs
    // to the new holder (join made it the seat's only connection), so this
    // connection has nothing to leave.
    let mut evicted = false;
    // One connection represents one agent in one loca, so this is naturally a
    // per-agent+loca queue. The quiet deadline slides with each fragment, but
    // the hard deadline remains anchored to the FIRST message so continued
    // typing cannot postpone a turn forever.
    let batching = filter == WsFilter::Mentions && turn_max > 1;
    let mut pending_turn = Vec::new();
    let mut turn_idle_deadline: Option<tokio::time::Instant> = None;
    let mut turn_hard_deadline: Option<tokio::time::Instant> = None;

    loop {
        tokio::select! {
            // Server -> client: broadcast frames.
            recv = rx.recv() => match recv {
                Ok(frame) => {
                    // A cross-loca caretaker summon is published only on the
                    // private home-loca bus. Drop it for every other client,
                    // and expose it to the explicitly named caretaker as the
                    // ordinary source-room message its existing listener and
                    // turn queue already understand.
                    let frame = match frame {
                        ServerFrame::Caretaker { message } => {
                            if !is_caretaker || !msg_addresses(&message, &name, false, false) {
                                continue;
                            }
                            ServerFrame::Msg { message }
                        }
                        other => other,
                    };
                    if let ServerFrame::Care { signal } = &frame {
                        if replayed_care_ids.contains(&signal.id) {
                            continue;
                        }
                    }
                    // A newer connection took our SEAT (last-writer-wins):
                    // close so we don't linger as a ghost holding it. Matching
                    // is by identity, not name — the new holder may have taken
                    // the seat under a different display name (one key = one
                    // seat; the label can change, the seat cannot double).
                    if let ServerFrame::Evicted { identity: who, session, .. } = &frame {
                        // Our own eviction broadcast carries our session id;
                        // ignore it. Anyone else on our identity steps aside.
                        // A watcher holds no seat, so a seat change on its
                        // identity never closes it (a Master watching one tab
                        // and seated in another stays at the door).
                        if !watch_only && who == &identity && *session != session_id {
                            let _ = send_frame(&mut sink, &frame).await;
                            evicted = true;
                            break;
                        }
                        continue;
                    }
                    // If I'm the one being kicked/banned, deliver it then close.
                    if let ServerFrame::Kicked { name: who, .. } = &frame {
                        if who == &name {
                            let _ = send_frame(&mut sink, &frame).await;
                            break;
                        }
                        continue; // others don't need to see it
                    }
                    // Credentials can die after the handshake. Never deliver a
                    // new room frame to a logged-out or revoked connection.
                    if !credentials.allowed(&hub, &room, &name) {
                        if let Some(kicked) = removal_notice(&mut rx, &name).await {
                            let _ = send_frame(&mut sink, &kicked).await;
                        }
                        break;
                    }
                    // `msg` is a raw event stream. `mentions` is a runtime
                    // nudge stream, so an operator control such as /stop must
                    // bypass the conversational queue and arrive immediately.
                    if events_only {
                        let allowed = matches!(
                            frame,
                            ServerFrame::Msg { .. }
                                | ServerFrame::Memory { .. }
                                | ServerFrame::MemoryVersion { .. }
                                // The Vault changed: a revision number, no
                                // text. It never enters a turn; the agent's
                                // listener reads the rules when it needs them.
                                | ServerFrame::Vault { .. }
                        )
                            || matches!(&frame, ServerFrame::Reaction { reaction } if reaction.owner == name)
                            || (filter == WsFilter::Mentions
                                && matches!(
                                    frame,
                                    ServerFrame::Control { .. } | ServerFrame::Care { .. }
                                ));
                        if !allowed {
                            continue;
                        }
                    }
                    if batching {
                        if let ServerFrame::Control { cmd } = &frame {
                            if cmd == "stop" {
                                pending_turn.clear();
                                turn_idle_deadline = None;
                                turn_hard_deadline = None;
                            }
                        }
                    }
                    if let ServerFrame::Care { signal } = &frame {
                        // Invariant (P0#2): a Care envelope is only ever placed
                        // on a socket whose room matches signal.room. A caretaker
                        // relay is re-homed onto the home loca before it is sent,
                        // so this now holds for caretakers too — no owner
                        // exception, unlike the retracted 44ef95b half.
                        if signal.room != room {
                            continue;
                        }
                        // A re-homed cross-loca envelope carries only bounded
                        // source context and stays private to its selected
                        // owner. Care's default/all event stream must never turn
                        // that envelope into source-room history for other
                        // caretakers sharing the home loca.
                        if !signal.source_room.is_empty()
                            && signal.source_room != room
                            && signal.owner.as_deref() != Some(name.as_str())
                        {
                            continue;
                        }
                    }
                    // Mentions mode is normally low-noise. Two explicit room
                    // states widen it:
                    //   * live mode lets everybody hear the active discussion;
                    //   * the named lead hears the whole room because that
                    //     overview is the lead's actual job.
                    // The latter is per-name and ends immediately when the
                    // operator names somebody else or clears the title.
                    if filter == WsFilter::Mentions {
                        if let ServerFrame::Care { signal } = &frame {
                            let addressed = match signal.owner_principal_id.as_deref() {
                                // Principal-required (Everyone per-member): ONLY the
                                // session whose authenticated principal matches gets
                                // it — never a display-name or group-audience match.
                                // This is what closes the N×N wake and keeps two
                                // same-named principals apart.
                                Some(pid) => session_principal_id.as_deref() == Some(pid),
                                None => {
                                    signal.owner.as_deref() == Some(name.as_str())
                                        || matches!(&signal.audience,
                                            AttentionAudience::Group { names }
                                                if names.iter().any(|member| member == &name))
                                }
                            };
                            if !addressed {
                                continue;
                            }
                        }
                        if let ServerFrame::Msg { message } = &frame {
                            if message.sender != name
                                && !msg_addresses(message, &name, accepts_all, !in_lobby)
                                && !hub.is_live(&room)
                                && !hub.is_lead(&room, &name)
                            {
                                continue;
                            }
                        }
                    }
                    if batching {
                        if let ServerFrame::Msg { message } = frame {
                            if message.kind == protocol::MessageKind::Announce {
                                // An announcement is immediate, but flushing
                                // pending chat WITH it still costs one turn.
                                pending_turn.push(message);
                                let messages = std::mem::take(&mut pending_turn);
                                turn_idle_deadline = None;
                                turn_hard_deadline = None;
                                if send_turn(&hub, &room, &mut sink, messages).await.is_err() {
                                    break;
                                }
                                continue;
                            }
                            // A filtered agent already ignores its own echo.
                            // Do it before opening a timer so its reply cannot
                            // delay the human's next turn.
                            if message.sender == name {
                                continue;
                            }
                            pending_turn.push(message);
                            let now = tokio::time::Instant::now();
                            if pending_turn.len() == 1 {
                                turn_hard_deadline = Some(
                                    now + std::time::Duration::from_millis(turn_max_wait_ms),
                                );
                            }
                            turn_idle_deadline = Some(
                                now + std::time::Duration::from_millis(turn_idle_ms),
                            );
                            if pending_turn.len() >= turn_max {
                                let messages = std::mem::take(&mut pending_turn);
                                turn_idle_deadline = None;
                                turn_hard_deadline = None;
                                if send_turn(&hub, &room, &mut sink, messages).await.is_err() {
                                    break;
                                }
                            }
                            continue;
                        }
                    }
                    if matches!(
                        frame,
                        ServerFrame::Msg { .. }
                            | ServerFrame::Turn { .. }
                            | ServerFrame::Care { .. }
                            | ServerFrame::Reaction { .. }
                    )
                        && send_memory_version(&hub, &room, &mut sink).await.is_err()
                    {
                        break;
                    }
                    if send_frame(&mut sink, &frame).await.is_err() { break; }
                }
                Err(RecvError::Lagged(n)) => {
                    tracing::warn!(%room, %name, skipped = n, "ws lagged");
                    // The broadcast bus is lossy. Force a reconnect so the
                    // client must resync the missing range from durable REST
                    // history instead of remaining ONLINE with a silent gap.
                    break;
                }
                Err(RecvError::Closed) => break,
            },
            // Keepalive: ping the client so an idle link isn't dropped (1006).
            _ = ping.tick() => {
                if !credentials.allowed(&hub, &room, &name) {
                    if let Some(kicked) = removal_notice(&mut rx, &name).await {
                        let _ = send_frame(&mut sink, &kicked).await;
                    }
                    break;
                }
                if sink.send(WsMessage::Ping(Vec::new().into())).await.is_err() { break; }
            },
            _ = async {
                let deadline = match (turn_idle_deadline, turn_hard_deadline) {
                    (Some(idle), Some(hard)) => Some(idle.min(hard)),
                    (Some(idle), None) => Some(idle),
                    (None, Some(hard)) => Some(hard),
                    (None, None) => None,
                };
                match deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                let messages = std::mem::take(&mut pending_turn);
                turn_idle_deadline = None;
                turn_hard_deadline = None;
                if !messages.is_empty()
                    && send_turn(&hub, &room, &mut sink, messages).await.is_err()
                {
                    break;
                }
            },
            // Client -> server: optional send/control (web client convenience).
            msg = stream.next() => match msg {
                Some(Ok(WsMessage::Text(txt))) => {
                    if !credentials.allowed(&hub, &room, &name) {
                        if let Some(kicked) = removal_notice(&mut rx, &name).await {
                            let _ = send_frame(&mut sink, &kicked).await;
                        }
                        break;
                    }
                    handle_client_frame(
                        &hub,
                        &room,
                        &name,
                        &operation_scope,
                        sender_principal.as_deref(),
                        kind,
                        credentials.is_admin(&hub),
                        credentials.live_session(&hub).is_some(),
                        watch_only,
                        &txt,
                    );
                }
                Some(Ok(WsMessage::Close(_))) | None => break,
                Some(Ok(_)) => {}   // pong/binary: ignore (pong keeps us alive)
                Some(Err(_)) => break,
            },
        }
    }

    // Complete the WebSocket closing handshake when the room bus closes or the
    // server deliberately evicts this connection. Dropping the split TCP sink
    // immediately can surface as code 1006 / ResetWithoutClosingHandshake and
    // may race the final control frame on loaded test or production hosts.
    let _ = sink.send(WsMessage::Close(None)).await;
    if !watch_only && !evicted {
        hub.leave(&room, &identity, session_id);
    }
    tracing::info!(%room, %name, watch_only, evicted, "ws leave");
    // The seat is free; now let the last frame arrive. Closing a TCP socket
    // that still holds bytes the peer sent makes the kernel answer with a
    // reset, and a reset can reach the peer instead of the frames just
    // written: a client that was sending when its seat was taken would then
    // see a broken connection where it should read `evicted` (or `kicked`),
    // and reconnect. So what the peer already sent is read and dropped until
    // it answers the close, for a bounded time.
    drain_before_close(&mut stream).await;
}

/// Read and drop what the peer of a socket the server is closing had already
/// sent, until it answers the close or `WS_CLOSE_DRAIN` passes. Call it after
/// the final frame and the close frame are written and after the seat or
/// presence is released: nothing read here is acted on.
pub(crate) async fn drain_before_close<R>(stream: &mut R)
where
    R: futures_util::Stream<Item = Result<WsMessage, axum::Error>> + Unpin,
{
    use futures_util::StreamExt;
    let _ = tokio::time::timeout(WS_CLOSE_DRAIN, async {
        while let Some(Ok(frame)) = stream.next().await {
            if matches!(frame, WsMessage::Close(_)) {
                break;
            }
        }
    })
    .await;
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_client_frame(
    hub: &Hub,
    room: &str,
    name: &str,
    operation_scope: &hub::OperationScope,
    sender_principal: Option<&str>,
    kind: SenderType,
    is_admin: bool,
    has_session: bool,
    watch_only: bool,
    txt: &str,
) {
    // A watcher listens without taking a seat (PRINCIPLES: "watches, does no work").
    // It reads the stream but produces nothing — no speaking, no typing, no
    // control. Otherwise someone invisible to the roster could still talk at
    // the table. Reads (the pushed frames) already flow; only writes are cut.
    if watch_only {
        tracing::debug!(%name, "watcher client frame dropped — watchers are read-only");
        return;
    }
    match serde_json::from_str::<ClientFrame>(txt) {
        Ok(ClientFrame::Send {
            target,
            text,
            reply_to,
            op_id,
        }) => {
            // With REQUIRE_SESSIONS, WS sends need a session-bound identity
            // too — otherwise the query-string name would be a spoof hole.
            // The Lobby always does (see `post_message`).
            if (hub.require_sessions() || hub.is_lobby_room(room)) && !has_session {
                tracing::debug!(%name, "ws send without session dropped (REQUIRE_SESSIONS)");
                return;
            }
            if !text.trim().is_empty() && op_id.as_ref().is_none_or(|id| id.len() <= 128) {
                // WS sends are subject to mode gating (no admin bypass here;
                // admins post via REST with the token to bypass).
                let mut body = PostMessage {
                    sender: name.to_string(),
                    sender_type: kind,
                    target,
                    text,
                    reply_to,
                    op_id,
                    kind: protocol::MessageKind::Say,
                    // WS text sends carry no attachments (uploads use the
                    // REST endpoint, then a REST/WS send cites their ids).
                    attachments: Vec::new(),
                };
                // In the Lobby room only a rank calls everyone.
                hub.settle_lobby_call(room, &mut body, is_admin);
                let _ = hub.post(room, body, false, operation_scope, sender_principal);
            }
        }
        // Control broadcasts (e.g. `/stop`) steer every agent in the room, so
        // they require live admin authority. Dropped, not
        // an error, for non-admins.
        Ok(ClientFrame::Control { cmd }) => {
            if is_admin {
                hub.control(room, &cmd);
            } else {
                tracing::debug!(%name, %cmd, "control frame from non-admin dropped");
            }
        }
        Ok(ClientFrame::Typing { on }) => hub.typing(room, name, on),
        Err(e) => tracing::debug!(%name, error = %e, "bad client frame"),
    }
}

/// How long a socket whose credential has just ended waits for the `kicked`
/// frame a removal broadcasts right after ending it.
pub(crate) const REMOVAL_NOTICE_WAIT_MS: u64 = 500;

/// A socket's credential has ended. A removal (kick, ban, release, invite or
/// membership revocation, suspension) ends it and then broadcasts `kicked`
/// naming this connection, so that frame can be queued behind ordinary frames
/// the socket may no longer forward, or still be on its way. Skip to it,
/// forwarding nothing else. `None` when no removal names this connection (a
/// sign-out): the socket then closes without a word, as before.
pub(crate) async fn removal_notice(
    rx: &mut tokio::sync::broadcast::Receiver<ServerFrame>,
    name: &str,
) -> Option<ServerFrame> {
    let wait = std::time::Duration::from_millis(REMOVAL_NOTICE_WAIT_MS);
    tokio::time::timeout(wait, async {
        loop {
            match rx.recv().await {
                Ok(frame) if matches!(&frame, ServerFrame::Kicked { name: who, .. } if who == name) => {
                    return Some(frame);
                }
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return None,
            }
        }
    })
    .await
    .ok()
    .flatten()
}

pub(crate) async fn send_frame<S>(sink: &mut S, frame: &ServerFrame) -> Result<(), ()>
where
    S: futures_util::Sink<WsMessage> + Unpin,
{
    use futures_util::SinkExt;
    let txt = serde_json::to_string(frame).map_err(|_| ())?;
    sink.send(WsMessage::Text(txt.into())).await.map_err(|_| ())
}

/// Preserve the legacy single-message frame and use a batch only when it
/// actually saves a wake-up.
pub(crate) async fn send_memory_version<S>(hub: &Hub, room: &str, sink: &mut S) -> Result<(), ()>
where
    S: futures_util::Sink<WsMessage> + Unpin,
{
    let version = hub.loca_memory_version(room).unwrap_or_else(|error| {
        tracing::warn!(%room, %error, "could not read loca memory version");
        0
    });
    send_frame(
        sink,
        &ServerFrame::MemoryVersion {
            room: room.to_string(),
            version,
        },
    )
    .await
}

pub(crate) async fn send_turn<S>(
    hub: &Hub,
    room: &str,
    sink: &mut S,
    mut messages: Vec<protocol::Message>,
) -> Result<(), ()>
where
    S: futures_util::Sink<WsMessage> + Unpin,
{
    let frame = if messages.len() == 1 {
        ServerFrame::Msg {
            message: messages.pop().expect("one pending message"),
        }
    } else {
        ServerFrame::Turn { messages }
    };
    send_memory_version(hub, room, sink).await?;
    send_frame(sink, &frame).await
}
