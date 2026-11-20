//! `GET /search` — the Building-wide message search.
//!
//! A Building route with no door of its own: it reads exactly the locas the
//! caller may enter, judged loca by loca with the same decision `GET /rooms`
//! lists with (`caller_reach`). An invite-scoped session therefore searches
//! its one loca; a direct message is searched only by its two people.
use crate::{
    admin_token_of, caller_reach, member_token_of, HashMap, HeaderMap, Hub, IntoResponse, Json,
    Query, State, StatusCode, SESSION_HEADER,
};

/// Hits returned when `limit` is not given.
const DEFAULT_LIMIT: usize = 50;
/// The most hits one request returns.
const MAX_LIMIT: usize = 200;

/// The longest query, in characters.
const MAX_QUERY_CHARS: usize = 200;
/// Searches one caller may run in [`SEARCH_WINDOW`]. A search reads every
/// loca the caller may enter, so a loop of them is a load on the server.
const SEARCH_BURST: usize = 30;
const SEARCH_WINDOW: std::time::Duration = std::time::Duration::from_secs(60);

/// Who is searching, for the rate limit: the session's name, else a hash of
/// the credential presented (the raw secret is never kept), else "anon".
fn caller_key(hub: &Hub, headers: &HeaderMap) -> String {
    use std::hash::{Hash, Hasher};
    if let Some(identity) = headers
        .get(SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|t| hub.session_identity(Some(t)))
    {
        return format!("session:{}", identity.name);
    }
    match member_token_of(headers).or_else(|| admin_token_of(headers)) {
        Some(token) => {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            token.hash(&mut hasher);
            format!("credential:{:x}", hasher.finish())
        }
        None => "anon".into(),
    }
}

/// Seconds to wait when `key` has used its searches, else `None` (and the
/// search is counted).
fn rate_limited(key: String) -> Option<u64> {
    use std::collections::VecDeque;
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;
    static RECENT: OnceLock<Mutex<HashMap<String, VecDeque<Instant>>>> = OnceLock::new();
    let now = Instant::now();
    let mut recent = RECENT
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    recent.retain(|_, times| {
        while times
            .front()
            .is_some_and(|t| now.duration_since(*t) >= SEARCH_WINDOW)
        {
            times.pop_front();
        }
        !times.is_empty()
    });
    let times = recent.entry(key).or_default();
    if times.len() >= SEARCH_BURST {
        let oldest = *times.front()?;
        let wait = SEARCH_WINDOW.saturating_sub(now.duration_since(oldest));
        return Some(wait.as_secs().max(1));
    }
    times.push_back(now);
    None
}

/// GET /search?q=needle[&limit=50]
pub(crate) async fn search_building(
    State(hub): State<Hub>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let needle = q.get("q").cloned().unwrap_or_default();
    if needle.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "missing q").into_response();
    }
    if needle.chars().count() > MAX_QUERY_CHARS {
        return (StatusCode::BAD_REQUEST, "q is too long").into_response();
    }
    if let Some(wait) = rate_limited(caller_key(&hub, &headers)) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(axum::http::header::RETRY_AFTER, wait.to_string())],
            format!("too many searches; try again in {wait}s"),
        )
            .into_response();
    }
    let limit = q
        .get("limit")
        .and_then(|l| l.parse::<usize>().ok())
        .unwrap_or(DEFAULT_LIMIT)
        .clamp(1, MAX_LIMIT);
    match hub.search_building(&needle, limit, caller_reach(&hub, &headers)) {
        Ok(hits) => Json(hits).into_response(),
        Err(error) => {
            tracing::error!(%error, "building search read failed");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
    }
}
