//! The question turning the Loca Board off asks while tasks are not done:
//! 409 with the counts, and the three answers a caller may give with
//! `?open_tasks=` (hub/work/board_off.rs).
use crate::{hub, IntoResponse, Json, StatusCode};

pub(crate) fn ask(unfinished: hub::Unfinished) -> axum::response::Response {
    let total = unfinished.total();
    let tasks = if total == 1 { "task" } else { "tasks" };
    (
        StatusCode::CONFLICT,
        Json(serde_json::json!({
            "error": format!(
                "{total} {tasks} on the Loca Board {} not done ({} to do, {} doing, {} in review). \
                 Say what becomes of {}: ?open_tasks=keep (kept tasks come back with the board), \
                 done, or remove.",
                if total == 1 { "is" } else { "are" },
                unfinished.open,
                unfinished.taken,
                unfinished.review,
                if total == 1 { "it" } else { "them" },
            ),
            "unfinished": unfinished,
            "choices": ["keep", "done", "remove"],
        })),
    )
        .into_response()
}
