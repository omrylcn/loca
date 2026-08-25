//! room-server: an always-on WS + REST hub where Claude agents and human users
//! share a chat room. See DESIGN.md for the full picture.
//!
//! Routes:
//!   GET  /                       -> web client (WhatsApp-like)
//!   GET  /health                 -> "ok"
//!   GET  /rooms                  -> [{room, members}]
//!   GET  /rooms/{id}/messages    -> ?since=<id> backlog for poll/backfill
//!   POST /rooms/{id}/messages    -> post a message (agents use this to talk)
//!   GET  /rooms/{id}/members     -> [{name, type}]
//!   GET  /ws?room=&name=&type=   -> live channel (listen; users may also send)

mod hub;
mod routes;
mod store;
mod sync;

use std::collections::HashMap;
use std::net::SocketAddr;

use axum::{
    extract::{
        ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
        DefaultBodyLimit, Path, Query, State,
    },
    http::{header::SEC_WEBSOCKET_PROTOCOL, HeaderMap, StatusCode},
    response::{Html, IntoResponse},
    routing::get,
    Json, Router,
};
use tokio::sync::broadcast::error::RecvError;
use tower_http::cors::CorsLayer;

use hub::{
    AttentionError, GoalError, Hub, HubConfig, LeadError, LobbyEvent, NoteError, TaskError,
    WaitError,
};
use protocol::{
    AttentionAction, AttentionAudience, CareAck, ClearWait, ClientFrame, CreateAttention,
    CreateGoal, CreateInvite, CreateNote, CreateSession, CreateTask, Moderate, PostMessage,
    SenderType, ServerFrame, SetMode, SetSettings, SetWait, UpdateGoal, UpdateNote, UpdateTask,
};
use routes::*;

/// Header carrying the admin token for admin-only actions.
const ADMIN_HEADER: &str = "x-admin-token";
/// Header carrying the room (join) token.
const ROOM_HEADER: &str = "x-room-token";
/// Header carrying a session token (server-derived identity).
const SESSION_HEADER: &str = "x-session-token";
/// Header carrying a one-use master-browser pairing code.
const PAIRING_HEADER: &str = "x-pairing-code";
const WS_PROTOCOL: &str = "loca.v1";

fn websocket_credential(headers: &HeaderMap, prefix: &str) -> Option<String> {
    headers
        .get_all(SEC_WEBSOCKET_PROTOCOL)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .find_map(|protocol| protocol.strip_prefix(prefix).map(str::to_string))
        .filter(|value| !value.is_empty())
}

fn legacy_ws_query_auth() -> bool {
    matches!(
        std::env::var("LEGACY_WS_QUERY_AUTH").as_deref(),
        Ok("1") | Ok("true")
    )
}

fn env_u32(key: &str, default: u32) -> u32 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

const WEB_INDEX: &str = include_str!("../../../web/index.html");
const WEB_FAVICON: &str = include_str!("../../../web/assets/favicon.svg");
const WEB_STYLES: &str = include_str!("../../../web/assets/styles.css");
const WEB_STATE: &str = include_str!("../../../web/assets/state.js");
const WEB_SOCKET: &str = include_str!("../../../web/assets/socket.js");
const WEB_PEOPLE: &str = include_str!("../../../web/assets/people.js");
const WEB_CHAT: &str = include_str!("../../../web/assets/chat.js");
const WEB_ADMIN: &str = include_str!("../../../web/assets/admin.js");
const WEB_FOCUS: &str = include_str!("../../../web/assets/focus.js");
const WEB_MEMORY: &str = include_str!("../../../web/assets/memory.js");
const WEB_PROFILE: &str = include_str!("../../../web/assets/profile.js");
const WEB_SIDEBAR: &str = include_str!("../../../web/assets/sidebar.js");
const WEB_API: &str = include_str!("../../../web/assets/api.js");
const WEB_APP: &str = include_str!("../../../web/assets/app.js");
const PRINCIPLES_TR: &str = include_str!("../../../PRINCIPLES.md");
const PRINCIPLES_EN: &str = include_str!("../../../PRINCIPLES.en.md");
const ADMIN_INDEX: &str = include_str!("../../../web/admin.html");
const ADMIN_CONSOLE_HEADER: &str = "x-loca-console";
const MAX_HTTP_BODY_BYTES: usize = 64 * 1024;
const MAX_WS_MESSAGE_BYTES: usize = 64 * 1024;

#[tokio::main]
async fn main() {
    // `room-server --health`: the container healthcheck. The runtime image
    // carries no wget/curl, so the binary probes itself.
    if std::env::args().any(|a| a == "--health") {
        let port: u16 = std::env::var("PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(8787);
        let ok = match tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
            Ok(mut s) => {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let req = "GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
                    .to_string();
                let mut buf = Vec::new();
                s.write_all(req.as_bytes()).await.is_ok()
                    && s.read_to_end(&mut buf).await.is_ok()
                    && String::from_utf8_lossy(&buf).contains("\"ok\":true")
            }
            Err(_) => false,
        };
        std::process::exit(if ok { 0 } else { 1 });
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "room_server=info,server=info".into()),
        )
        .init();

    let admin_token = std::env::var("ADMIN_TOKEN").unwrap_or_default();
    if admin_token.is_empty() {
        tracing::warn!("ADMIN_TOKEN not set — admin actions (chat mode/settings) are OPEN to anyone. Set ADMIN_TOKEN for real control.");
    }
    let room_token = std::env::var("ROOM_TOKEN").unwrap_or_default();
    let require_invite = matches!(
        std::env::var("REQUIRE_INVITE").as_deref(),
        Ok("1") | Ok("true")
    );
    if room_token.is_empty() {
        if require_invite {
            tracing::info!("ROOM_TOKEN retired — davet-only door is active.");
        } else {
            tracing::warn!("ROOM_TOKEN not set — anyone can connect and post. Set ROOM_TOKEN or REQUIRE_INVITE=1 to close the door.");
        }
    }

    // Persistence: DB_PATH set -> SQLite file; unset -> memory-only (clears on restart).
    let db_path = std::env::var("DB_PATH").ok();
    let store = store::Store::open(db_path.as_deref()).expect("open store");
    if let Ok(rename) = std::env::var("ROOM_RENAME") {
        let (from, to) = rename
            .split_once(':')
            .filter(|(from, to)| !from.trim().is_empty() && !to.trim().is_empty())
            .expect("ROOM_RENAME must be old:new");
        if store
            .rename_room(from.trim(), to.trim())
            .expect("ROOM_RENAME migration failed")
        {
            tracing::info!(
                from = from.trim(),
                to = to.trim(),
                "loca renamed atomically"
            );
        }
    }
    let store = std::sync::Arc::new(store);
    if store.is_persistent() {
        tracing::info!(path = %db_path.as_deref().unwrap_or("?"), "persistence: SQLite");
    } else {
        tracing::warn!(
            "DB_PATH not set — running memory-only; all rooms/messages/notes clear on restart."
        );
    }

    // Default rate limit (per-room, admin-tunable at runtime).
    let default_settings = protocol::RoomSettings {
        lead: None,
        rate_limit: env_u32("RATE_LIMIT", 10),
        rate_window_secs: env_u32("RATE_WINDOW_SECS", 30).max(1),
        live: false,
        archived: false,
        live_timeout_secs: env_u32("LIVE_TIMEOUT_SECS", 120),
        operators: Vec::new(),
        turn_max_messages: env_u32("TURN_MAX_MESSAGES", 4).clamp(1, 16),
        turn_idle_ms: env_u32("TURN_IDLE_MS", 5_000).clamp(100, 30_000),
        turn_max_wait_ms: env_u32("TURN_MAX_WAIT_MS", 15_000).clamp(100, 60_000),
        care_wait_secs: env_u32("CARE_WAIT_SECS", 120),
        care_cooldown_secs: env_u32("CARE_COOLDOWN_SECS", 300),
        care_max_attempts: env_u32("CARE_MAX_ATTEMPTS", 2).clamp(1, 10),
        care_context_messages: env_u32("CARE_CONTEXT_MESSAGES", 8).clamp(0, 20),
        care_recipient: protocol::ReminderRecipient::Lead,
        care_goal_secs: env_u32("CARE_GOAL_SECS", 0),
        care_task_secs: env_u32("CARE_TASK_SECS", 0),
        care_silence_secs: env_u32("CARE_SILENCE_SECS", 0),
    };

    // Boot epoch = process start time (seconds); changes every restart.
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // REQUIRE_SESSIONS=1: posting requires a session token, so `sender` is
    // always server-derived (spoof-proof identity; PRODUCTION.md Aşama 0).
    // Unset = compat mode: sessions are available but body identity still works.
    let require_sessions = matches!(
        std::env::var("REQUIRE_SESSIONS").as_deref(),
        Ok("1") | Ok("true")
    );
    // REQUIRE_INVITE=1: an empty ROOM_TOKEN means davet-only, not open-house.
    // This is how the building key is retired — no key, but the door still
    // demands a davet for every loca. Defaults on when there is no room token
    // AND an admin token exists, so retiring the key never accidentally opens
    // the building.
    // Explicit opt-in only: turning this on automatically whenever the key is
    // gone would silently change every existing deployment's behaviour. Prod
    // sets REQUIRE_INVITE=1 in its .env when it retires the building key.
    if require_invite {
        tracing::info!("davet-only mode: no building key, every loca needs a davet");
    }
    let home_room = std::env::var("LOCA_AGENT_ROOM").unwrap_or_else(|_| "iye".into());
    let reserved_room = std::env::var("RESERVED_LOCA").unwrap_or_default();
    let caretakers = std::env::var("LOCA_CARETAKERS")
        .unwrap_or_else(|_| "loca-care".into())
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect();
    let hub = Hub::build(
        HubConfig {
            admin_token,
            room_token,
            require_sessions,
            require_invite,
            home_room,
            reserved_room,
            caretakers,
        },
        store,
        default_settings,
        epoch,
    );
    if hub.admin_pairing_code().is_some() {
        tracing::info!(
            "master browser pairing is available from the loopback master desk; code omitted from logs"
        );
    }

    // Sweep for live rooms that have gone quiet and switch them back off, so a
    // forgotten live room can't keep waking every agent.
    {
        let hub = hub.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(15));
            loop {
                tick.tick().await;
                hub.expire_live();
            }
        });
    }
    {
        let hub = hub.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                hub.tick_care();
            }
        });
    }

    // The master desk is deliberately a second HTTP surface. In production
    // Docker publishes it as 127.0.0.1:3004 only, so the browser reaches it
    // through an SSH forward; the public reverse proxy never sees these
    // routes. It uses the in-process Hub directly, therefore ADMIN_TOKEN stays
    // in the server's environment and is never sent to HTML or JavaScript.
    if let Some(admin_port) = std::env::var("ADMIN_CONSOLE_PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
    {
        let admin_bind_addr: std::net::IpAddr = std::env::var("ADMIN_CONSOLE_BIND_ADDR")
            .ok()
            .and_then(|a| a.parse().ok())
            .unwrap_or_else(|| [127, 0, 0, 1].into());
        let admin_addr = SocketAddr::from((admin_bind_addr, admin_port));
        let admin_app = Router::new()
            .route("/", get(admin_console_index))
            .route("/api/state", get(admin_console_state))
            .route("/api/members", axum::routing::post(admin_console_admit))
            .route("/api/invites", axum::routing::post(admin_console_invite))
            .route("/api/pairing", axum::routing::post(admin_console_pairing))
            .with_state(hub.clone());
        let admin_listener = tokio::net::TcpListener::bind(admin_addr)
            .await
            .expect("bind admin console");
        tracing::info!(
            "master desk listening on http://{admin_addr} (publish on host loopback only)"
        );
        tokio::spawn(async move {
            axum::serve(admin_listener, admin_app)
                .await
                .expect("serve admin console");
        });
    }

    let app = Router::new()
        .route("/", get(index))
        .route("/PRINCIPLES.md", get(principles_tr))
        .route("/PRINCIPLES.en.md", get(principles_en))
        .route("/assets/:name", get(web_asset))
        .route("/health", get(health))
        .route(
            "/sessions",
            axum::routing::post(create_session_route).delete(delete_session_route),
        )
        .route("/pairings", axum::routing::post(create_pairing_route))
        .route(
            "/admission-stock",
            get(get_admission_stock_route).post(create_admission_stock_route),
        )
        .route(
            "/join-requests",
            get(list_join_requests_route).post(create_join_request_route),
        )
        .route("/join-requests/:id", get(get_join_request_route))
        .route(
            "/join-requests/:id/approve",
            axum::routing::post(approve_join_request_route),
        )
        .route(
            "/join-requests/:id/deny",
            axum::routing::post(deny_join_request_route),
        )
        .route(
            "/join-requests/:id/bootstrap",
            axum::routing::post(bootstrap_join_request_route),
        )
        .route("/whoami", get(whoami))
        .route("/profile", get(profile_view))
        .route(
            "/profile/credentials",
            get(list_profile_credentials).post(create_profile_credential_route),
        )
        .route(
            "/profile/credentials/:credential_id",
            axum::routing::delete(revoke_profile_credential_route),
        )
        .route("/membership/claim", axum::routing::post(claim_membership))
        .route("/lobby/ws", get(lobby_ws_handler))
        .route("/members", get(list_members).post(admit_member))
        .route("/profiles", get(list_profiles))
        .route("/care/residents", get(caretaker_residents))
        .route(
            "/runtime/health",
            axum::routing::post(report_runtime_health),
        )
        .route(
            "/members/:token",
            axum::routing::delete(revoke_member_route),
        )
        .route("/residents", get(list_residents))
        .route("/rooms/:id/call", axum::routing::post(call_into_loca))
        .route(
            "/rooms/:id/release",
            axum::routing::post(release_self_from_loca),
        )
        .route("/smasters", get(list_smasters).post(create_smaster))
        .route(
            "/smasters/:token",
            axum::routing::delete(revoke_smaster_route),
        )
        .route("/rooms", get(list_rooms))
        .route("/rooms/:id/messages", get(get_messages).post(post_message))
        .route("/rooms/:id/reactions", get(get_reactions))
        .route(
            "/rooms/:id/messages/:message_id/reactions",
            axum::routing::post(set_reaction),
        )
        .route("/rooms/:id/members", get(get_members))
        .route("/rooms/:id/mode", get(get_mode).put(set_mode))
        .route("/rooms/:id/lead", axum::routing::post(set_lead))
        .route(
            "/rooms/:id/operators",
            get(get_loca_operator)
                .post(appoint_loca_operator_route)
                .delete(revoke_loca_operator_route),
        )
        .route("/rooms/:id/settings", get(get_settings).put(set_settings))
        .route("/rooms/:id/moderate", get(get_mod).post(moderate))
        .route("/rooms/:id", axum::routing::delete(delete_room))
        .route("/rooms/:id/invites", get(list_invites).post(create_invite))
        .route(
            "/rooms/:id/invites/:token",
            axum::routing::delete(revoke_invite),
        )
        .route("/rooms/:id/notes", get(get_notes).post(create_note))
        .route(
            "/rooms/:id/notes/:key",
            get(get_note).put(update_note).delete(delete_note),
        )
        .route("/rooms/:id/notes/:key/history", get(note_history))
        .route("/rooms/:id/search", get(search_room))
        .route("/rooms/:id/journal", get(get_journal).post(post_journal))
        .route("/rooms/:id/tasks", get(list_tasks).post(create_task))
        .route("/rooms/:id/tasks/:tid", axum::routing::patch(update_task))
        .route("/rooms/:id/goals", get(list_goals).post(create_goal))
        .route("/rooms/:id/goals/:gid", axum::routing::patch(update_goal))
        .route(
            "/rooms/:id/attentions",
            get(list_attentions).post(create_attention),
        )
        .route(
            "/rooms/:id/attentions/:attention_id/claim",
            axum::routing::post(claim_attention),
        )
        .route(
            "/rooms/:id/attentions/:attention_id/resolve",
            axum::routing::post(resolve_attention),
        )
        .route("/rooms/:id/waits", get(list_waits).post(set_wait))
        .route("/rooms/:id/waits/:name", axum::routing::delete(clear_wait))
        .route(
            "/rooms/:id/care/:signal_id/ack",
            axum::routing::post(ack_care),
        )
        .route("/ws", get(ws_handler))
        .route_layer(axum::middleware::from_fn_with_state(
            hub.clone(),
            require_membership,
        ))
        .with_state(hub)
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES));

    // CORS is opt-in: the web client is served same-origin, so browsers need
    // no CORS at all by default — and a permissive default would let any web
    // page the operator visits post into a localhost server. Set
    // CORS_ALLOW_ORIGIN to "*" or a comma-separated origin list to allow a
    // cross-origin client (e.g. a UI hosted elsewhere).
    let app = match std::env::var("CORS_ALLOW_ORIGIN") {
        Ok(v) if v.trim() == "*" => app.layer(CorsLayer::permissive()),
        Ok(v) => {
            let origins: Vec<axum::http::HeaderValue> =
                v.split(',').filter_map(|o| o.trim().parse().ok()).collect();
            app.layer(
                CorsLayer::new()
                    .allow_origin(origins)
                    .allow_methods(tower_http::cors::Any)
                    .allow_headers(tower_http::cors::Any),
            )
        }
        Err(_) => app,
    };

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8787);
    // Default to loopback: exposing the room to a network is an explicit choice
    // (set BIND_ADDR=0.0.0.0, e.g. in a container behind a reverse proxy).
    let bind_addr: std::net::IpAddr = std::env::var("BIND_ADDR")
        .ok()
        .and_then(|a| a.parse().ok())
        .unwrap_or_else(|| [127, 0, 0, 1].into());
    let addr = SocketAddr::from((bind_addr, port));
    tracing::info!("room-server listening on http://{addr}  (web client at /)");

    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    // `into_make_service_with_connect_info` exposes the peer address so the
    // authless join-request create endpoint can rate-limit per source IP.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .expect("serve");
}

/// Serve the web client with no-store so browsers never run a stale UI after a
/// server update (this bit users repeatedly during development).
async fn index() -> impl IntoResponse {
    (
        [
            ("cache-control", "no-store"),
            ("x-content-type-options", "nosniff"),
            ("x-frame-options", "DENY"),
            ("referrer-policy", "no-referrer"),
            (
                "permissions-policy",
                "camera=(), microphone=(), geolocation=()",
            ),
            (
                "content-security-policy",
                "default-src 'self'; connect-src 'self'; img-src 'self' data:; \
                 style-src 'self' 'unsafe-inline'; script-src 'self'; \
                 frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
            ),
        ],
        Html(WEB_INDEX),
    )
}

fn principles_document(body: &'static str) -> impl IntoResponse {
    (
        [
            ("content-type", "text/markdown; charset=utf-8"),
            ("cache-control", "no-store"),
            ("x-content-type-options", "nosniff"),
        ],
        body,
    )
}

async fn principles_tr() -> impl IntoResponse {
    principles_document(PRINCIPLES_TR)
}

async fn principles_en() -> impl IntoResponse {
    principles_document(PRINCIPLES_EN)
}

/// Serve only compile-time embedded browser assets. The allow-list prevents
/// this route from becoming a filesystem server, while `include_str!` keeps a
/// deployed binary self-contained and makes missing assets a compile failure.
async fn web_asset(Path(name): Path<String>) -> impl IntoResponse {
    let asset = match name.as_str() {
        "favicon.svg" => Some(("image/svg+xml; charset=utf-8", WEB_FAVICON)),
        "styles.css" => Some(("text/css; charset=utf-8", WEB_STYLES)),
        "state.js" => Some(("text/javascript; charset=utf-8", WEB_STATE)),
        "socket.js" => Some(("text/javascript; charset=utf-8", WEB_SOCKET)),
        "people.js" => Some(("text/javascript; charset=utf-8", WEB_PEOPLE)),
        "chat.js" => Some(("text/javascript; charset=utf-8", WEB_CHAT)),
        "admin.js" => Some(("text/javascript; charset=utf-8", WEB_ADMIN)),
        "focus.js" => Some(("text/javascript; charset=utf-8", WEB_FOCUS)),
        "memory.js" => Some(("text/javascript; charset=utf-8", WEB_MEMORY)),
        "profile.js" => Some(("text/javascript; charset=utf-8", WEB_PROFILE)),
        "sidebar.js" => Some(("text/javascript; charset=utf-8", WEB_SIDEBAR)),
        "api.js" => Some(("text/javascript; charset=utf-8", WEB_API)),
        "app.js" => Some(("text/javascript; charset=utf-8", WEB_APP)),
        _ => None,
    };
    match asset {
        Some((content_type, body)) => (
            StatusCode::OK,
            [
                ("content-type", content_type),
                ("cache-control", "no-store"),
                ("x-content-type-options", "nosniff"),
            ],
            body,
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "asset not found").into_response(),
    }
}

/// The SSH-forward-only master desk. Every asset is inline so opening the
/// tunnel is sufficient; it never fetches code from a third party.
async fn admin_console_index() -> impl IntoResponse {
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
fn admin_console_write_allowed(headers: &HeaderMap) -> bool {
    headers
        .get(ADMIN_CONSOLE_HEADER)
        .and_then(|v| v.to_str().ok())
        == Some("1")
}

async fn admin_console_state(State(hub): State<Hub>) -> impl IntoResponse {
    Json(serde_json::json!({
        "locas": hub.room_summaries_for(|_| true),
        // Resident deliberately omits the mb_ membership token. The browser
        // needs names and loca occupancy, never identity credentials.
        "residents": hub.residents(),
        "server": std::env::var("PUBLIC_SERVER_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8787".into()),
    }))
}

async fn admin_console_admit(
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
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not save building member",
        )
            .into_response(),
    }
}

async fn admin_console_invite(
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
    let Some(member) = hub.member_by_name(name) else {
        return (
            StatusCode::CONFLICT,
            "agent is not a building member — add them first",
        )
            .into_response();
    };
    match hub.invite_member_to_room(&member.token, loca, "master") {
        Ok(invite) => (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "token": invite.token,
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
            "iye is reserved for master, smaster, loca-dev and loca-care",
        )
            .into_response(),
        Err(hub::InviteError::Storage) => {
            (StatusCode::SERVICE_UNAVAILABLE, "could not save davet").into_response()
        }
    }
}

async fn admin_console_pairing(
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

/// Health + boot epoch + whether a room token is required. Clients poll this to
/// detect a restart (epoch change) and to know if they must send a token.
async fn health(State(hub): State<Hub>) -> impl IntoResponse {
    Json(serde_json::json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "epoch": hub.epoch(),
        "needs_token": !hub.is_member(None),
        // true when no ADMIN_TOKEN is configured: admin actions are open to all
        // (dev mode), so clients can surface admin controls without a token.
        "admin_open": hub.is_admin(None),
        // The loca agent tends the place rather than sitting in it: clients
        // offer this name in @-autocomplete everywhere, roster or not.
        "loca_agent": hub.caretaker_names().into_iter().next(),
        "loca_agents": hub.caretaker_names(),
        // The caretaker's one private loca is configurable. Lobby is a
        // building roster, not a room and never the caretaker's special seat.
        "loca_agent_room": hub.home_room(),
    }))
}

async fn ws_handler(
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
        davet: websocket_credential(&headers, "loca.room.")
            .or_else(|| legacy.then(|| q.get("token").cloned()).flatten()),
        session: websocket_credential(&headers, "loca.session.")
            .or_else(|| legacy.then(|| q.get("session").cloned()).flatten()),
    };

    // Resolve the session FIRST: it is a way in on its own (the browser holds
    // one and no davet), so judging the door before reading it would turn
    // every seated operator away.
    let session_identity = hub.session_identity(credentials.session.as_deref());
    if credentials.session.is_some() && session_identity.is_none() {
        return (StatusCode::UNAUTHORIZED, "invalid session token").into_response();
    }

    // The WS door: the SAME single decision the REST gate uses. Bearers arrive
    // as WebSocket subprotocol credentials; ?name= is only the ban hint when
    // there is no authoritative session or davet identity yet.
    match hub.enter_decision(
        &room,
        credentials.admin.as_deref(),
        credentials.davet.as_deref(),
        session_identity.as_ref(),
        q.get("name").map(String::as_str),
    ) {
        hub::EnterDecision::Allowed => {}
        hub::EnterDecision::Banned => {
            return (StatusCode::FORBIDDEN, "banned from this room").into_response();
        }
        hub::EnterDecision::Denied => {
            return (StatusCode::UNAUTHORIZED, "davet required for this loca").into_response();
        }
    }
    // A session OR davet names its holder authoritatively. Query-string names
    // are only a hint for open/legacy doors; allowing a davet to enter under an
    // alias meant revocation kicked the recorded holder while the alias socket
    // stayed alive.
    let davet_identity = credentials
        .davet
        .as_deref()
        .and_then(|token| hub.invite_for(&room, Some(token)));
    let member_identity = credentials
        .davet
        .as_deref()
        .and_then(|token| hub.member_for_credential(Some(token)));
    if let Some(idy) = &session_identity {
        name = idy.name.clone();
    } else if let Some(invite) = &davet_identity {
        name = invite.name.clone();
    } else if let Some(member) = &member_identity {
        name = member.name.clone();
    }
    let kind = match &session_identity {
        Some(idy) => idy.kind,
        None => match davet_identity.as_ref().map(|invite| invite.kind.as_str()) {
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
        credentials.davet.as_deref(),
        session_identity.as_ref(),
        credentials.session.as_deref(),
        &name,
    );
    ws.protocols([WS_PROTOCOL])
        .max_message_size(MAX_WS_MESSAGE_BYTES)
        .max_frame_size(MAX_WS_MESSAGE_BYTES)
        .on_upgrade(move |socket| {
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
            )
        })
        .into_response()
}

/// Bearers used at the WebSocket door. They stay attached to the connection so
/// logout, expiry, smaster revoke, and davet revoke take effect after the
/// handshake too; authorization is not a forever-valid snapshot.
#[derive(Clone)]
struct WsCredentials {
    admin: Option<String>,
    davet: Option<String>,
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
                self.davet.as_deref(),
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
enum WsFilter {
    All,
    Messages,
    Mentions,
}

/// Does message `m` address participant `name`? (target or @-mention of name/all)
///
/// An explicit `@all` or `target=all` addresses every identity seated in this
/// loca, including its caretakers. System announcements remain passive context:
/// they must not manufacture a model turn merely because their target is all.
fn msg_addresses(m: &protocol::Message, name: &str, accepts_all: bool) -> bool {
    let all_is_a_call = accepts_all && m.kind != protocol::MessageKind::Announce;
    if m.target.as_deref() == Some(name) {
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
        if all_is_a_call && tok == "@all" {
            return true;
        }
    }
    false
}

/// Server-side keepalive so an idle connection isn't closed (was seen as 1006).
const WS_PING_SECS: u64 = 30;

/// One WS connection: replay history, then fan broadcast frames down to the
/// client while accepting optional `send`/`control` frames coming up.
/// `events_only` suppresses non-message frames (see `?filter=msg`).
#[allow(clippy::too_many_arguments)]
async fn ws_session(
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
) {
    use futures_util::{SinkExt, StreamExt};

    let events_only = filter != WsFilter::All; // msg or mentions -> no history/members
                                               // Identify this session so we can tell our own eviction broadcast apart
                                               // from a later one aimed at us.
    let session_id = hub.next_session_id();
    // `@all` calls ordinary room members everywhere. A caretaker receives it
    // only at its private home table; a cross-loca watch must never turn a
    // room-wide call into access to that room's discussion.
    let accepts_all = !is_caretaker || room == hub.home_room();
    let (mut sink, mut stream) = socket.split();
    let (mut rx, history) = hub.subscribe(&room);
    let mut replayed_care_ids = std::collections::HashSet::new();
    if !watch_only && !hub.join(&room, &identity, &name, kind, session_id) {
        // The loca is full. Say so rather than dropping silently — and the
        // door stays open for watchers, who take no seat.
        tracing::info!(%room, %name, "loca full — refused");
        let _ = send_frame(
            &mut sink,
            &ServerFrame::Control {
                cmd: format!(
                    "loca full ({} seats). ask the master for a seat, or watch with ?watch=1",
                    hub::Hub::LOCA_KAPASITE
                ),
            },
        )
        .await;
        return;
    }
    tracing::info!(%room, %name, ?kind, "ws join");

    // Care signals are a durable outbox, not a best-effort broadcast. Replay
    // anything this identity has not transport-ACKed; the listener ACKs only
    // after writing its durable inbox. Subscribe happened first, so remember
    // ids and suppress a raced live copy from `rx` below.
    for signal in hub.pending_care(&room, &name) {
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
                hub.leave(&room, &identity);
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
                hub.leave(&room, &identity);
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
    // to the new holder (join reset its count to 1), so this connection must
    // NOT run leave() on the way out — that would free the seat under them.
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
                            if !is_caretaker || !msg_addresses(&message, &name, false) {
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
                        if who == &identity && *session != session_id {
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
                        break;
                    }
                    // `msg` is a raw event stream. `mentions` is a runtime
                    // nudge stream, so an operator control such as /stop must
                    // bypass the conversational queue and arrive immediately.
                    if events_only {
                        let allowed = matches!(frame, ServerFrame::Msg { .. })
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
                        // owner. Iye's default/all event stream must never turn
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
                            let addressed = signal.owner.as_deref() == Some(name.as_str())
                                || matches!(&signal.audience,
                                    AttentionAudience::Group { names }
                                        if names.iter().any(|member| member == &name));
                            if !addressed {
                                continue;
                            }
                        }
                        if let ServerFrame::Msg { message } = &frame {
                            if message.sender != name
                                && !msg_addresses(message, &name, accepts_all)
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
                                if send_turn(&mut sink, messages).await.is_err() {
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
                                if send_turn(&mut sink, messages).await.is_err() {
                                    break;
                                }
                            }
                            continue;
                        }
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
                if !credentials.allowed(&hub, &room, &name) { break; }
                if sink.send(WsMessage::Ping(Vec::new())).await.is_err() { break; }
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
                if !messages.is_empty() && send_turn(&mut sink, messages).await.is_err() {
                    break;
                }
            },
            // Client -> server: optional send/control (web client convenience).
            msg = stream.next() => match msg {
                Some(Ok(WsMessage::Text(txt))) => {
                    if !credentials.allowed(&hub, &room, &name) { break; }
                    handle_client_frame(
                        &hub,
                        &room,
                        &name,
                        &identity,
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
        hub.leave(&room, &identity);
    }
    tracing::info!(%room, %name, watch_only, evicted, "ws leave");
}

#[allow(clippy::too_many_arguments)]
fn handle_client_frame(
    hub: &Hub,
    room: &str,
    name: &str,
    identity: &str,
    kind: SenderType,
    is_admin: bool,
    has_session: bool,
    watch_only: bool,
    txt: &str,
) {
    // A watcher listens without taking a seat (PRINCIPLES: "izler, iş yapmaz").
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
            if hub.require_sessions() && !has_session {
                tracing::debug!(%name, "ws send without session dropped (REQUIRE_SESSIONS)");
                return;
            }
            if !text.trim().is_empty() && op_id.as_ref().is_none_or(|id| id.len() <= 128) {
                // WS sends are subject to mode gating (no admin bypass here;
                // admins post via REST with the token to bypass).
                let _ = hub.post(
                    room,
                    PostMessage {
                        sender: name.to_string(),
                        sender_type: kind,
                        target,
                        text,
                        reply_to,
                        op_id,
                        kind: protocol::MessageKind::Say,
                    },
                    false,
                    identity,
                );
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

async fn send_frame<S>(sink: &mut S, frame: &ServerFrame) -> Result<(), ()>
where
    S: futures_util::Sink<WsMessage> + Unpin,
{
    use futures_util::SinkExt;
    let txt = serde_json::to_string(frame).map_err(|_| ())?;
    sink.send(WsMessage::Text(txt)).await.map_err(|_| ())
}

/// Preserve the legacy single-message frame and use a batch only when it
/// actually saves a wake-up.
async fn send_turn<S>(sink: &mut S, mut messages: Vec<protocol::Message>) -> Result<(), ()>
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
    send_frame(sink, &frame).await
}
