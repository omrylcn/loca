//! Room attachment endpoints (docs/rfc-room-attachments.md).
//!
//! Both are gated by the same `RoomAccess` extractor as every other
//! `/rooms/:id/...` route, so only a member/valid session of THIS loca reaches
//! them. Security decisions that must not be skipped live here, not in the
//! store: the type is decided by SNIFFING the bytes (never the client's
//! `content-type`), the display name is sanitized (never a path), the body is
//! size-capped, and the served blob carries `nosniff` + an always-`attachment`
//! disposition so nothing is ever inline-executed.
use crate::*;
use axum::body::Bytes;
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::HeaderValue;

/// `POST /rooms/:id/attachments` — raw bytes in the body, `x-filename` +
/// `content-type` in headers. Returns the stored ref `{id,sha256,name,mime,size}`.
pub(crate) async fn upload_attachment(
    State(hub): State<Hub>,
    access: RoomAccess,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    if !hub.attachments_enabled() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "attachments are not available on this server",
        )
            .into_response();
    }
    // Identity: with a session the uploader is that principal; otherwise it is
    // required only when the building requires sessions (dev servers don't).
    let session = headers.get(SESSION_HEADER).and_then(|v| v.to_str().ok());
    let identity = hub.session_identity(session);
    let uploader = match identity.as_ref() {
        Some(idy) => idy
            .member
            .as_ref()
            .map(|m| format!("mb:{m}"))
            .unwrap_or_else(|| {
                let kind = match idy.kind {
                    SenderType::Agent => "agent",
                    SenderType::User => "user",
                };
                format!("{kind}:{}", idy.name)
            }),
        None if session.is_some() => {
            return (StatusCode::UNAUTHORIZED, "invalid session token").into_response();
        }
        None if hub.require_sessions() => {
            return (StatusCode::UNAUTHORIZED, "session token required").into_response();
        }
        None => "anon".to_string(),
    };

    if body.is_empty() {
        return (StatusCode::BAD_REQUEST, "empty file").into_response();
    }
    // Defense in depth: the route also caps the streamed body, but reject an
    // oversize buffer explicitly so the limit is enforced even if the layer is
    // ever misconfigured.
    if body.len() as u64 > hub::attachment_max_bytes() {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            "file exceeds the 10 MB attachment limit",
        )
            .into_response();
    }

    // The type is what the BYTES are, never what the header claims. A claimed
    // content-type that disagrees with the sniff is a 415 (e.g. a script sent
    // as image/png). The stored mime is the sniffed one.
    let sniffed = match sniff_mime(&body) {
        Some(m) => m,
        None => {
            return (
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported file type (allowed: png, jpeg, webp, pdf, txt, md)",
            )
                .into_response();
        }
    };
    if let Some(claimed) = headers.get(CONTENT_TYPE).and_then(|v| v.to_str().ok()) {
        if !mime_matches(claimed, sniffed) {
            return (
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "file content does not match the declared content-type",
            )
                .into_response();
        }
    }

    let name = sanitize_filename(
        headers.get("x-filename").and_then(|v| v.to_str().ok()),
        sniffed,
    );

    match hub.put_pending_attachment(&access.room, &uploader, &name, sniffed, &body) {
        Ok(att) => (StatusCode::CREATED, Json(att)).into_response(),
        Err(store::AttachError::Disabled) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "attachments are not available on this server",
        )
            .into_response(),
        Err(store::AttachError::QuotaRoom) => (
            StatusCode::PAYLOAD_TOO_LARGE,
            "this loca's attachment quota is full",
        )
            .into_response(),
        Err(store::AttachError::QuotaBuilding) => (
            StatusCode::PAYLOAD_TOO_LARGE,
            "the building's attachment storage is full",
        )
            .into_response(),
        Err(store::AttachError::Storage) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "could not store the file — try again",
        )
            .into_response(),
    }
}

/// `GET /rooms/:id/attachments/:att_id` — serve a blob IFF a message in THIS
/// loca references it. 404 otherwise (including a valid hash referenced only in
/// another loca — no cross-room read).
pub(crate) async fn get_attachment(
    State(hub): State<Hub>,
    access: RoomAccess,
    Path((_room, att_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if !hub.attachments_enabled() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "attachments are not available on this server",
        )
            .into_response();
    }
    let Some(serve) = hub.read_room_attachment(&access.room, &att_id) else {
        return (StatusCode::NOT_FOUND, "no such attachment in this loca").into_response();
    };
    let mut out = HeaderMap::new();
    out.insert(
        CONTENT_TYPE,
        HeaderValue::from_str(&serve.mime)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    // Never let the browser second-guess the type, and never inline-execute:
    // even a PDF/HTML-looking blob is offered as a download the user opens
    // deliberately. Images still render in an <img> tag (disposition is ignored
    // there), so inline preview in the client is unaffected.
    out.insert(
        "X-Content-Type-Options",
        HeaderValue::from_static("nosniff"),
    );
    let disposition = format!("attachment; filename=\"{}\"", disposition_safe(&serve.name));
    out.insert(
        CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition)
            .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );
    (out, serve.bytes).into_response()
}

/// Recognize a blob from its leading bytes. Returns the stored mime, or `None`
/// for anything outside the allowlist. Text is any valid UTF-8 with no NUL.
fn sniff_mime(bytes: &[u8]) -> Option<&'static str> {
    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if bytes.starts_with(PNG) {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if bytes.starts_with(b"%PDF-") {
        return Some("application/pdf");
    }
    // Text last: valid UTF-8 with no NUL byte. A binary file with a stray magic
    // mismatch never slips through as text because the NUL/│UTF-8 check rejects it.
    if !bytes.contains(&0) && std::str::from_utf8(bytes).is_ok() {
        return Some("text/plain");
    }
    None
}

/// The claimed content-type (minus params) must match the sniffed type. Text
/// accepts either text/plain or a markdown label, since bytes can't distinguish
/// them; everything else is exact.
fn mime_matches(claimed: &str, sniffed: &str) -> bool {
    let c = claimed
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if sniffed == "text/plain" {
        matches!(
            c.as_str(),
            "text/plain" | "text/markdown" | "text/x-markdown"
        )
    } else {
        c == sniffed
    }
}

/// Sanitize `x-filename` into display metadata: strip control chars and header
/// injection, never a path component, cap at 255 UTF-8 bytes. Empty → a default
/// name from the sniffed type.
fn sanitize_filename(raw: Option<&str>, sniffed: &str) -> String {
    let cleaned: String = raw
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| if c == '/' || c == '\\' { '_' } else { c })
        .collect();
    let capped = cap_bytes(cleaned.trim(), 255);
    if capped.is_empty() {
        default_name(sniffed).to_string()
    } else {
        capped
    }
}

/// Escape a name for a quoted `Content-Disposition filename`. Control chars are
/// already gone; here we neutralize the `"` and `\` that would break the quote.
fn disposition_safe(name: &str) -> String {
    name.chars()
        .map(|c| if c == '"' || c == '\\' { '_' } else { c })
        .collect()
}

fn default_name(sniffed: &str) -> &'static str {
    match sniffed {
        "image/png" => "attachment.png",
        "image/jpeg" => "attachment.jpg",
        "image/webp" => "attachment.webp",
        "application/pdf" => "attachment.pdf",
        _ => "attachment.txt",
    }
}

/// Truncate to at most `max` bytes on a UTF-8 char boundary (never splits a
/// multibyte char, so the result is always valid UTF-8).
fn cap_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_recognizes_allowlist_and_rejects_others() {
        assert_eq!(
            sniff_mime(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]),
            Some("image/png")
        );
        assert_eq!(sniff_mime(&[0xFF, 0xD8, 0xFF, 0x00]), Some("image/jpeg"));
        let mut webp = b"RIFF".to_vec();
        webp.extend_from_slice(&[0, 0, 0, 0]);
        webp.extend_from_slice(b"WEBP");
        assert_eq!(sniff_mime(&webp), Some("image/webp"));
        assert_eq!(sniff_mime(b"%PDF-1.7\n"), Some("application/pdf"));
        assert_eq!(sniff_mime(b"just some text"), Some("text/plain"));
        // A NUL byte means not-text; with no magic match it is unsupported.
        assert_eq!(sniff_mime(&[0x00, 0x01, 0x02, 0x03]), None);
        // An ELF binary (script/executable) sent as bytes: rejected.
        assert_eq!(sniff_mime(&[0x7F, b'E', b'L', b'F', 0x00]), None);
    }

    #[test]
    fn claimed_type_must_match_sniff() {
        assert!(mime_matches("image/png", "image/png"));
        assert!(mime_matches("image/png; charset=binary", "image/png"));
        assert!(!mime_matches("image/png", "application/pdf"));
        // A script claimed as png must fail against a text sniff.
        assert!(!mime_matches("image/png", "text/plain"));
        // Text accepts either label.
        assert!(mime_matches("text/markdown", "text/plain"));
        assert!(mime_matches("text/plain", "text/plain"));
    }

    #[test]
    fn filename_is_sanitized_and_never_a_path() {
        assert_eq!(
            sanitize_filename(Some("../../etc/passwd"), "text/plain"),
            ".._.._etc_passwd"
        );
        assert_eq!(sanitize_filename(Some("a\r\nb: c"), "text/plain"), "ab: c");
        assert_eq!(sanitize_filename(Some("  "), "image/png"), "attachment.png");
        assert_eq!(sanitize_filename(None, "application/pdf"), "attachment.pdf");
        // A CRLF injection attempt loses the control chars.
        assert!(!sanitize_filename(Some("x\r\nSet-Cookie: y"), "text/plain").contains('\n'));
    }

    #[test]
    fn filename_capped_at_255_bytes_on_boundary() {
        let long = "é".repeat(200); // 400 bytes
        let out = sanitize_filename(Some(&long), "text/plain");
        assert!(out.len() <= 255);
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
    }

    #[test]
    fn disposition_neutralizes_quotes() {
        assert_eq!(disposition_safe("a\"b\\c"), "a_b_c");
    }
}
