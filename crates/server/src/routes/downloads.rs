//! Public, secretless skill distribution endpoint.
//!
//! The Host is the skill library: it serves the embedded `loca` / `loca-care`
//! bundles so an agent (or the Web Getting Started) can install a skill from a
//! path instead of cloning a repo. These routes carry NO credential and require
//! NO membership — the bundles are public release artifacts, byte-identical to
//! what the Desktop ships (same SHA-256 in each manifest).

use crate::*;
use axum::extract::Path;
use axum::http::header;
use axum::response::Response;

/// GET /downloads/skills — a discovery index: every available skill with its
/// version, checksum, and download/manifest URLs.
pub(crate) async fn skill_bundles_index() -> Response {
    let skills: Vec<serde_json::Value> = skill_bundles::all()
        .iter()
        .map(|b| {
            let manifest: serde_json::Value =
                serde_json::from_str(b.manifest).unwrap_or_else(|_| serde_json::json!({}));
            serde_json::json!({
                "name": b.name,
                "download_url": format!("/downloads/skills/{}", b.name),
                "manifest_url": format!("/downloads/skills/{}/manifest", b.name),
                "manifest": manifest,
            })
        })
        .collect();
    axum::Json(serde_json::json!({ "skills": skills })).into_response()
}

/// GET /downloads/skills/:name — the deterministic zip archive for one skill.
pub(crate) async fn download_skill_bundle(Path(name): Path<String>) -> Response {
    match skill_bundles::bundle(&name) {
        Some(b) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "application/zip".to_string()),
                (
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{}.zip\"", b.name),
                ),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
            ],
            axum::body::Bytes::from_static(b.zip),
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "unknown skill").into_response(),
    }
}

/// GET /downloads/skills/:name/manifest — the JSON manifest (version, per-file
/// SHA-256, and the archive's own SHA-256) for one skill.
pub(crate) async fn skill_bundle_manifest(Path(name): Path<String>) -> Response {
    match skill_bundles::bundle(&name) {
        Some(b) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            b.manifest,
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "unknown skill").into_response(),
    }
}
