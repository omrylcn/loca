//! Notifications HTTP (docs/concepts.md "Notifications"): read my own, send
//! one, mark read, and review what I sent.
use crate::*;

/// The caller: a session, or a personal credential — an agent's membership
/// key may ride in `x-room-token` as it does at the Lobby.
fn viewer(hub: &Hub, headers: &HeaderMap) -> hub::RequestAuthority {
    let credential = admin_token_of(headers).or_else(|| member_token_of(headers));
    hub.resolve_authority(credential, session_of(headers))
}

fn notification_error(error: hub::NotificationError) -> axum::response::Response {
    use hub::NotificationError::*;
    let status = match error {
        Unauthenticated => StatusCode::UNAUTHORIZED,
        RankRequired | NoGroup | MasterOnly => StatusCode::FORBIDDEN,
        NoSuchGroup | NotFound => StatusCode::NOT_FOUND,
        ArchivedGroup => StatusCode::CONFLICT,
        NoAudience | InvalidTitle | InvalidBody => StatusCode::BAD_REQUEST,
        Unavailable | Storage => StatusCode::SERVICE_UNAVAILABLE,
    };
    (status, error.message()).into_response()
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
    match hub.notifications_of(
        &viewer(&hub, &headers),
        query_i64(&q, "since"),
        query_i64(&q, "before"),
        query_u32(&q, "limit"),
    ) {
        Ok((notifications, unread, more)) => Json(serde_json::json!({
            "notifications": notifications,
            "unread": unread,
            "more": more,
        }))
        .into_response(),
        Err(error) => notification_error(error),
    }
}

/// `POST /notifications {"title", "body", "audience": {"all": true} |
/// {"groups": [ids]}}` — the Master, or a Smaster to its own groups.
pub(crate) async fn send_notification_route(
    State(hub): State<Hub>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let who = viewer(&hub, &headers);
    let title = body.get("title").and_then(|v| v.as_str()).unwrap_or("");
    let text = body.get("body").and_then(|v| v.as_str()).unwrap_or("");
    let audience = body.get("audience");
    let audience = if audience
        .and_then(|a| a.get("all"))
        .and_then(|v| v.as_bool())
        == Some(true)
    {
        Some(hub::NotificationAudience::All)
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
        let refused = hub
            .notification_audiences(&who)
            .can_send
            .then_some(hub::NotificationError::NoAudience)
            .unwrap_or(hub::NotificationError::RankRequired);
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
