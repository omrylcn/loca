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

// A module names what it uses (Phase 7.8): no `use x::*` outside test modules.
#![warn(clippy::wildcard_imports)]

mod hub;
mod router;
mod routes;
mod store;
mod sync;
mod ws;

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
use routes::{
    accept_lesson, ack_care, ack_join_request_route, ack_membership_claim_route, actor_of,
    add_extension, add_group_member_route, admin_console_admit, admin_console_approve_join_request,
    admin_console_deny_join_request, admin_console_index, admin_console_invite,
    admin_console_join_requests, admin_console_mint_stock, admin_console_pairing,
    admin_console_state, admin_token_of, admit_member, append_long_memory,
    appoint_loca_operator_route, approve_join_request_route, bootstrap_join_request_route,
    call_into_loca, caller_reach, caretaker_memory, caretaker_residents, caretaker_viewer,
    claim_attention, claim_membership, claim_wiki_review, clear_wait, commit_wiki, configure_wiki,
    create_admission_stock_route, create_attention, create_goal, create_group_route, create_invite,
    create_join_request_route, create_loca_route, create_note, create_pairing_route,
    create_profile_credential_route, create_session_route, create_smaster, create_task,
    delete_group_route, delete_loca_records, delete_note, delete_room, delete_session_route,
    deny_join_request_route, dismiss_lesson, dm_candidates_route, dm_close_route, dm_fence,
    dm_master_route, dm_overview_route, dm_seal_route, dm_witness_route, download_skill_bundle,
    edit_wiki_page, extension_catalog, extension_fence, extension_off, finish_wiki_review,
    get_admission_stock_route, get_attachment, get_building_settings, get_extensions, get_group,
    get_join_request_route, get_journal, get_lessons, get_loca_memory, get_loca_memory_snapshot,
    get_loca_operator, get_members, get_messages, get_mod, get_mode, get_note, get_notes,
    get_profile_preferences, get_reactions, get_settings, get_task, get_vault, get_vault_history,
    get_wiki, get_wiki_page, getting_started_doc, group_fence, group_log_route, index,
    is_admin_req, is_master_req, issue_key, leave_building_route, list_attentions,
    list_building_bans, list_building_ranks, list_goals, list_groups, list_invites,
    list_join_requests_route, list_keys, list_long_memory_entries, list_members, list_my_dms,
    list_notifications, list_profile_credentials, list_profiles, list_residents, list_rooms,
    list_smasters, list_tasks, list_waits, lobby_ws_handler, loca_archive, member_token_of,
    moderate, no_such_loca, note_history, notification_audiences_route, open_dm_route,
    pairing_code_of, pairing_ttl_ms, post_journal, post_lesson, post_message, post_typing,
    preview_loca_group_route, principles_en, principles_tr, profile_view, put_building_settings,
    put_extensions, put_profile_card, put_profile_preferences, put_vault,
    read_all_notifications_route, read_notification_route, release_self_from_loca,
    remove_extension, remove_group_member_route, remove_wiki_page, rename_profile_route,
    rename_smaster_route, renew_wiki_review, report_runtime_health, require_membership,
    resolve_attention, revoke_invite, revoke_key_route, revoke_loca_operator_route,
    revoke_member_route, revoke_profile_credential_route, revoke_smaster_route, room_decision_of,
    search_building, search_room, send_notification_route, sent_notifications_route, session_of,
    set_lead, set_loca_group_route, set_loca_memory_owner, set_member_suspension_route, set_mode,
    set_reaction, set_settings, set_wait, skill_bundle_manifest, skill_bundles_index,
    socket_sees_nothing, unexpected_answer, update_goal, update_group_route, update_note,
    update_task, upload_attachment, valid_identity_name, web_asset, whoami, wiki_context,
    wiki_context_accept, wiki_context_offer, wiki_context_request, wiki_context_requests,
    wiki_maintenance_status, wiki_native_ready, write_short_memory, RoomAccess, NAME_WORN_RULE,
    SESSION_HEADER,
};
use ws::{
    drain_before_close, legacy_ws_query_auth, websocket_credential, ws_handler, WS_PING_SECS,
    WS_PROTOCOL,
};

fn env_u32(key: &str, default: u32) -> u32 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

const MAX_HTTP_BODY_BYTES: usize = 64 * 1024;
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
            tracing::info!("ROOM_TOKEN retired — invite-only door is active.");
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
        hub::loca_name::require_rename_rule(&store, from.trim(), to.trim());
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
    hub::loca_name::require_no_legacy_lobby(&store);
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
        lead_principal: None,
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
        // Absent = the legacy set: a loca auto-created by a first join keeps
        // what every loca had before extensions could be chosen.
        extensions: None,
        // A loca has no identity until someone gives it one.
        title: None,
        description: None,
        seats: None,
        icon: None,
        color: None,
        labels: Vec::new(),
        vault_rev: None,
        board_since: None,
    };

    // Boot epoch = process start time (seconds); changes every restart.
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // REQUIRE_SESSIONS=1: posting requires a session token, so `sender` is
    // always server-derived (spoof-proof identity; PRODUCTION.md stage 0).
    // Unset = compat mode: sessions are available but body identity still works.
    let require_sessions = matches!(
        std::env::var("REQUIRE_SESSIONS").as_deref(),
        Ok("1") | Ok("true")
    );
    // REQUIRE_INVITE=1: an empty ROOM_TOKEN means invite-only, not open-house.
    // This is how the building key is retired — no key, but the door still
    // demands an invite for every loca. Defaults on when there is no room token
    // AND an admin token exists, so retiring the key never accidentally opens
    // the building.
    // Explicit opt-in only: turning this on automatically whenever the key is
    // gone would silently change every existing deployment's behaviour. Prod
    // sets REQUIRE_INVITE=1 in its .env when it retires the building key.
    if require_invite {
        tracing::info!("invite-only mode: no building key, every loca needs an invite");
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
    // LOCA_CAPACITY: seats of a loca without a number of its own, humans and
    // agents alike (default 7, clamped 2..=32). Watchers never take a seat.
    let capacity = hub
        .set_loca_capacity(
            env_u32("LOCA_CAPACITY", hub::Hub::DEFAULT_LOCA_CAPACITY as u32) as usize,
        );
    // LOCA_CAPACITY_MAX: the most a loca's own number may reach (default 32,
    // clamped LOCA_CAPACITY..=32). Set it equal to LOCA_CAPACITY to keep every
    // loca at the Building's number.
    let capacity_max = hub
        .set_loca_capacity_max(
            env_u32("LOCA_CAPACITY_MAX", hub::Hub::LOCA_CAPACITY_MAX as u32) as usize,
        );
    tracing::info!(capacity, capacity_max, "seats per loca");
    // NOTIFICATION_RATE_LIMIT: notifications one sender may send in any ten
    // minutes (default 6, at least 1; the window is fixed).
    let notification_limit = hub.set_notification_send_limit(env_u32(
        "NOTIFICATION_RATE_LIMIT",
        hub::NOTIFICATION_SEND_LIMIT,
    ));
    tracing::info!(
        notification_limit,
        "notifications per sender per 10 minutes"
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
                hub.tick_wiki();
                hub.expire_unused_smasters();
                hub.expire_join_request_bootstraps();
                hub.expire_invite_deliveries();
                // Logged inside; a storage error does not stop serving.
                let _ = hub.sweep_sessions();
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
    // Collect expired pending uploads (never sent) and blobs whose last
    // referencing message was deleted, so orphaned bytes never accumulate on
    // disk. Every 5 min is ample against a 1h pending TTL. No-op when the store
    // is memory-only (no blob dir).
    {
        let hub = hub.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(300));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                let removed = hub.sweep_attachments();
                if removed > 0 {
                    tracing::info!(removed, "attachment sweep collected orphaned blobs");
                }
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
            .route("/api/join-requests", get(admin_console_join_requests))
            .route(
                "/api/join-requests/{id}/approve",
                axum::routing::post(admin_console_approve_join_request),
            )
            .route(
                "/api/join-requests/{id}/deny",
                axum::routing::post(admin_console_deny_join_request),
            )
            .route(
                "/api/admission-stock",
                axum::routing::post(admin_console_mint_stock),
            )
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

    let app = router::router(hub);

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
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    let bound_addr = listener.local_addr().expect("bound address");
    tracing::info!("room-server listening on http://{bound_addr}  (web client at /)");
    // Integration tests use PORT=0 so the OS owns port allocation without a
    // probe/drop/bind race across separate test binaries. The optional file is
    // a child-to-parent readiness channel; production does not set it.
    if let Ok(path) = std::env::var("BOUND_PORT_FILE") {
        std::fs::write(path, bound_addr.port().to_string()).expect("write bound port");
    }
    // `into_make_service_with_connect_info` exposes the peer address so the
    // authless join-request create endpoint can rate-limit per source IP.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .expect("serve");
}

/// Health + boot epoch + whether a room token is required. Clients poll this to
/// detect a restart (epoch change) and to know if they must send a token.
async fn health(State(hub): State<Hub>) -> impl IntoResponse {
    Json(serde_json::json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "epoch": hub.epoch(),
        "needs_token": !hub.is_member(None),
        // Seats of a loca without a number of its own (LOCA_CAPACITY),
        // humans and agents alike, and the most a loca's own number may
        // reach (LOCA_CAPACITY_MAX).
        "loca_capacity": hub.loca_capacity(),
        "loca_capacity_max": hub.loca_capacity_max(),
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
