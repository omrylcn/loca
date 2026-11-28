//! Notifications HTTP (docs/concepts.md "Notifications"): read my own, send
//! one, mark read, and review what I sent.
use crate::{
    admin_token_of, hub, member_token_of, session_of, HashMap, HeaderMap, Hub, IntoResponse, Json,
    Path, Query, State, StatusCode,
};

/// The caller: a session, or a personal credential — an agent's membership
/// key may ride in `x-room-token` as it does at the Lobby.
fn viewer(hub: &Hub, headers: &HeaderMap) -> hub::RequestAuthority {
    let credential = admin_token_of(headers).or_else(|| member_token_of(headers));
    hub.resolve_authority(credential, session_of(headers))
}

fn notification_error(error: hub::NotificationError) -> axum::response::Response {
    use hub::NotificationError::*;
    if let RateLimited {
        max,
        retry_after_secs,
    } = error
    {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(
                axum::http::header::RETRY_AFTER,
                retry_after_secs.to_string(),
            )],
            rate_limited_sentence(max, retry_after_secs),
        )
            .into_response();
    }
    let status = match error {
        Unauthenticated => StatusCode::UNAUTHORIZED,
        RankRequired | NoGroup | MasterOnly | CaretakerAgentsOnly => StatusCode::FORBIDDEN,
        NoSuchGroup | NotFound => StatusCode::NOT_FOUND,
        ArchivedGroup | NoRecipients => StatusCode::CONFLICT,
        NoAudience | InvalidTitle | InvalidBody => StatusCode::BAD_REQUEST,
        Unavailable | Storage => StatusCode::SERVICE_UNAVAILABLE,
        RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
    };
    (status, error.message()).into_response()
}

/// The 429 body: what happened and when the next one fits, in words a person
/// reads in the composer (the browser shows it as it is).
fn rate_limited_sentence(max: u32, retry_after_secs: u64) -> String {
    let wait = if retry_after_secs < 60 {
        let s = retry_after_secs.max(1);
        format!("{s} second{}", if s == 1 { "" } else { "s" })
    } else {
        let m = retry_after_secs.div_ceil(60);
        format!("{m} minute{}", if m == 1 { "" } else { "s" })
    };
    let sent = if max == 1 {
        "1 notification".to_string()
    } else {
        format!("{max} notifications")
    };
    format!("You have sent {sent} in the last 10 minutes, the most allowed. You can send the next one in {wait}.")
}

fn query_i64(q: &HashMap<String, String>, key: &str) -> Option<i64> {
    q.get(key).and_then(|v| v.parse::<i64>().ok())
}

fn query_u32(q: &HashMap<String, String>, key: &str) -> Option<u32> {
    q.get(key).and_then(|v| v.parse::<u32>().ok())
}

/// `GET /notifications?since=&before=&limit=` — my notifications, newest
/// first, with `read` flags and my unread count. `since` returns only newer
/// ones (the browser's poll); `before` pages back; `more` says an older page
/// exists.
pub(crate) async fn list_notifications(
    State(hub): State<Hub>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let order = q.get("order").map(String::as_str).unwrap_or("desc");
    if !matches!(order, "asc" | "desc") {
        return (StatusCode::BAD_REQUEST, "order must be asc or desc").into_response();
    }
    match hub.notifications_of(
        &viewer(&hub, &headers),
        query_i64(&q, "since"),
        query_i64(&q, "before"),
        query_u32(&q, "limit"),
        order == "asc",
    ) {
        Ok((notifications, unread, more)) => Json(serde_json::json!({
            "notifications": notifications,
            "unread": unread,
            "more": more,
            "order": order,
        }))
        .into_response(),
        Err(error) => notification_error(error),
    }
}

/// `POST /notifications {"title", "body", "audience": {"all": true} |
/// {"groups": [ids]} | {"ranks": true} | {"agents": true}}` — the Master to
/// any; a Smaster to its own groups and the ranks; the caretaker to every
/// agent only.
pub(crate) async fn send_notification_route(
    State(hub): State<Hub>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let who = viewer(&hub, &headers);
    let title = body.get("title").and_then(|v| v.as_str()).unwrap_or("");
    let text = body.get("body").and_then(|v| v.as_str()).unwrap_or("");
    let audience = body.get("audience");
    let flag =
        |key: &str| audience.and_then(|a| a.get(key)).and_then(|v| v.as_bool()) == Some(true);
    let audience = if flag("all") {
        Some(hub::NotificationAudience::All)
    } else if flag("agents") {
        Some(hub::NotificationAudience::Agents)
    } else if flag("ranks") {
        Some(hub::NotificationAudience::Ranks)
    } else {
        audience
            .and_then(|a| a.get("groups"))
            .and_then(|g| g.as_array())
            .map(|groups| {
                hub::NotificationAudience::Groups(
                    groups
                        .iter()
                        .filter_map(|id| id.as_str().map(str::to_string))
                        .collect(),
                )
            })
    };
    let Some(audience) = audience else {
        // Rank first: a member must not learn the request shape it may not use.
        let refused = if hub.notification_audiences(&who).can_send {
            hub::NotificationError::NoAudience
        } else {
            hub::NotificationError::RankRequired
        };
        return notification_error(refused);
    };
    match hub.send_notification(&who, title, text, audience) {
        Ok(sent) => (StatusCode::CREATED, Json(sent)).into_response(),
        Err(error) => notification_error(error),
    }
}

/// `GET /notifications/audiences` — what this caller may address: `everyone`
/// (the Master) and the groups (the Master all, a Smaster its own).
pub(crate) async fn notification_audiences_route(
    State(hub): State<Hub>,
    headers: HeaderMap,
) -> impl IntoResponse {
    Json(hub.notification_audiences(&viewer(&hub, &headers)))
}

/// `POST /notifications/:id/read` — mark one of my own read.
pub(crate) async fn read_notification_route(
    State(hub): State<Hub>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Ok(id) = id.parse::<i64>() else {
        return notification_error(hub::NotificationError::NotFound);
    };
    match hub.read_notification(&viewer(&hub, &headers), id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => notification_error(error),
    }
}

/// `POST /notifications/read-all` — mark all of mine read.
pub(crate) async fn read_all_notifications_route(
    State(hub): State<Hub>,
    headers: HeaderMap,
) -> impl IntoResponse {
    match hub.read_all_notifications(&viewer(&hub, &headers)) {
        Ok(marked) => Json(serde_json::json!({ "marked": marked })).into_response(),
        Err(error) => notification_error(error),
    }
}

/// `GET /notifications/sent?before=&limit=` — what I sent, with recipient
/// and read counts (never who read it).
pub(crate) async fn sent_notifications_route(
    State(hub): State<Hub>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    match hub.sent_notifications(
        &viewer(&hub, &headers),
        query_i64(&q, "before"),
        query_u32(&q, "limit"),
    ) {
        Ok(sent) => Json(sent).into_response(),
        Err(error) => notification_error(error),
    }
}

/// `GET /notifications/:id/recipients` — who it went to and who it reached:
/// its sender's own, and every one for the Master.
pub(crate) async fn notification_recipients_route(
    State(hub): State<Hub>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> impl IntoResponse {
    match hub.notification_recipients(&viewer(&hub, &headers), id) {
        Ok(recipients) => {
            Json(serde_json::json!({ "id": id, "recipients": recipients })).into_response()
        }
        Err(error) => notification_error(error),
    }
}
