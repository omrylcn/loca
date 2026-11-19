//! `GET /search` — the Building-wide message search.
//!
//! A Building route with no door of its own: it reads exactly the locas the
//! caller may enter, judged loca by loca with the same decision `GET /rooms`
//! lists with (`caller_reach`). An invite-scoped session therefore searches
//! its one loca; a direct message is searched only by its two people.
use crate::*;

/// Hits returned when `limit` is not given.
const DEFAULT_LIMIT: usize = 50;
/// The most hits one request returns.
const MAX_LIMIT: usize = 200;

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
