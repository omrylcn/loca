//! The Master's delete of a closed loca, and the archive taken before it.
//!
//! `GET /rooms/:id/archive` answers the loca's records as a zip;
//! `DELETE /rooms/:id/records` deletes them (the loca must be closed first).
//! Both are the Master's alone, as a seal is.

use super::access::is_master_req;
use crate::*;
use axum::http::header;
use hub::LocaRemovalError;

fn refusal(error: LocaRemovalError) -> axum::response::Response {
    let status = match error {
        LocaRemovalError::NotFound => StatusCode::NOT_FOUND,
        // The loca's state, not the caller's authority: the Master is the
        // only one who reaches these.
        LocaRemovalError::NotClosed | LocaRemovalError::Protected => StatusCode::CONFLICT,
        LocaRemovalError::Storage => StatusCode::SERVICE_UNAVAILABLE,
    };
    (status, error.message()).into_response()
}

/// A file name for the archive that any system accepts.
fn archive_name(room: &str) -> String {
    let safe: String = room
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("loca-{safe}-archive.zip")
}

pub(crate) async fn loca_archive(
    State(hub): State<Hub>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> axum::response::Response {
    if !is_master_req(&hub, &headers) {
        return (StatusCode::UNAUTHORIZED, "the Master's alone").into_response();
    }
    let room = id.clone();
    let built = tokio::task::spawn_blocking(move || hub.loca_archive(&room)).await;
    match built {
        Ok(Ok(zip)) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "application/zip".to_string()),
                (
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{}\"", archive_name(&id)),
                ),
                (header::CACHE_CONTROL, "no-store".to_string()),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
            ],
            zip,
        )
            .into_response(),
        Ok(Err(error)) => refusal(error),
        Err(_) => refusal(LocaRemovalError::Storage),
    }
}

pub(crate) async fn delete_loca_records(
    State(hub): State<Hub>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> axum::response::Response {
    if !is_master_req(&hub, &headers) {
        return (StatusCode::UNAUTHORIZED, "the Master's alone").into_response();
    }
    match hub.delete_loca(&id) {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => refusal(error),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_archive_name_is_plain() {
        assert_eq!(super::archive_name("şube 2/x"), "loca--ube-2-x-archive.zip");
        assert_eq!(super::archive_name("my-loca"), "loca-my-loca-archive.zip");
    }
}
