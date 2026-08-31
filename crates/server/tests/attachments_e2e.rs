//! End-to-end over the real binary: upload a blob, cite it on a message, fetch
//! it back, and prove the room-isolation / rejection / lifecycle contract from
//! docs/rfc-room-attachments.md. The server runs with DB_PATH so attachments
//! are enabled (memory-only mode disables them). The open door (no ADMIN_TOKEN /
//! no REQUIRE_INVITE) lets a test post without minting sessions — the room
//! isolation we assert is enforced by the attachment ref index, not by auth.

use std::time::Duration;

use serde_json::Value;
use tokio::net::TcpListener;

struct ServerGuard {
    _child: tokio::process::Child,
}

async fn spawn(db_path: &str) -> (String, ServerGuard) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let bin = env!("CARGO_BIN_EXE_room-server");
    let child = tokio::process::Command::new(bin)
        .env("PORT", port.to_string())
        .env("RUST_LOG", "warn")
        .env("ADMIN_TOKEN", "")
        .env("DB_PATH", db_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let guard = ServerGuard { _child: child };

    let client = reqwest::Client::new();
    for _ in 0..100 {
        if let Ok(r) = client
            .get(format!("http://127.0.0.1:{port}/health"))
            .send()
            .await
        {
            if r.status().is_success() {
                return (format!("http://127.0.0.1:{port}"), guard);
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("server did not come up");
}

/// A byte string whose leading bytes sniff as PNG. The rest is arbitrary — the
/// server never decodes the image, it only recognizes and stores the bytes.
fn png_bytes(tag: u8) -> Vec<u8> {
    let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    v.extend_from_slice(&[tag; 64]);
    v
}

async fn upload(
    client: &reqwest::Client,
    base: &str,
    room: &str,
    bytes: Vec<u8>,
    content_type: &str,
    filename: &str,
) -> reqwest::Response {
    client
        .post(format!("{base}/rooms/{room}/attachments"))
        .header("content-type", content_type)
        .header("x-filename", filename)
        .body(bytes)
        .send()
        .await
        .unwrap()
}

async fn send_msg(
    client: &reqwest::Client,
    base: &str,
    room: &str,
    text: &str,
    attachments: &[&str],
) -> reqwest::Response {
    client
        .post(format!("{base}/rooms/{room}/messages"))
        .json(&serde_json::json!({
            "sender": "tester",
            "sender_type": "agent",
            "text": text,
            "attachments": attachments,
        }))
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn upload_cite_fetch_roundtrip_and_room_isolation() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("att.sqlite3");
    let (base, _g) = spawn(&db.to_string_lossy()).await;
    let client = reqwest::Client::new();
    let bytes = png_bytes(1);

    // 1. Upload → 201 with a ref whose id == sha256.
    let up = upload(
        &client,
        &base,
        "alpha",
        bytes.clone(),
        "image/png",
        "pic.png",
    )
    .await;
    assert_eq!(up.status(), 201, "upload should be created");
    let ref_obj: Value = up.json().await.unwrap();
    let id = ref_obj["id"].as_str().unwrap().to_string();
    assert_eq!(ref_obj["sha256"].as_str().unwrap(), id, "id == sha256");
    assert_eq!(ref_obj["mime"].as_str().unwrap(), "image/png");
    assert_eq!(ref_obj["name"].as_str().unwrap(), "pic.png");
    assert_eq!(ref_obj["size"].as_u64().unwrap(), bytes.len() as u64);

    // 2. A pending (uploaded but not yet cited) blob is NOT fetchable: GET auth
    //    requires a referencing message in this room, not merely an upload.
    let pre = client
        .get(format!("{base}/rooms/alpha/attachments/{id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        pre.status(),
        404,
        "an un-cited upload must not be fetchable"
    );

    // 3. Cite it on a message → 201, and the flip pending→referenced happens.
    let msg = send_msg(&client, &base, "alpha", "here is a pic", &[&id]).await;
    assert_eq!(msg.status(), 201, "message citing a valid upload");
    let msg_obj: Value = msg.json().await.unwrap();
    assert_eq!(
        msg_obj["attachments"][0]["id"].as_str().unwrap(),
        id,
        "the accepted message carries the ref"
    );

    // 4. Now GET succeeds with the exact bytes + safe serve headers.
    let got = client
        .get(format!("{base}/rooms/alpha/attachments/{id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(got.status(), 200, "referenced blob is fetchable");
    assert_eq!(
        got.headers().get("x-content-type-options").unwrap(),
        "nosniff"
    );
    assert_eq!(got.headers().get("content-type").unwrap(), "image/png");
    assert!(
        got.headers()
            .get("content-disposition")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("attachment;"),
        "never inline-executed"
    );
    let served = got.bytes().await.unwrap();
    assert_eq!(
        served.as_ref(),
        bytes.as_slice(),
        "bytes round-trip exactly"
    );

    // 5. Cross-room isolation: the SAME id, known by hash, is 404 from another
    //    loca that never referenced it — no cross-room read.
    let cross = client
        .get(format!("{base}/rooms/beta/attachments/{id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        cross.status(),
        404,
        "no cross-room read with a guessed hash"
    );

    // 6. Durability: the message reloads from SQLite with its attachments.
    let history: Vec<Value> = client
        .get(format!("{base}/rooms/alpha/messages?since=0"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let carried = history
        .iter()
        .any(|m| m["attachments"].get(0).and_then(|a| a["id"].as_str()) == Some(id.as_str()));
    assert!(
        carried,
        "reloaded message must still carry its attachment ref"
    );
}

#[tokio::test]
async fn rejects_type_mismatch_and_bad_citations() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("att.sqlite3");
    let (base, _g) = spawn(&db.to_string_lossy()).await;
    let client = reqwest::Client::new();

    // A script/text body sent AS image/png: the sniff disagrees with the claim.
    let mismatch = upload(
        &client,
        &base,
        "alpha",
        b"#!/bin/sh\nrm -rf /\n".to_vec(),
        "image/png",
        "evil.png",
    )
    .await;
    assert_eq!(mismatch.status(), 415, "content must match declared type");

    // An unrecognized binary type (no allowlist magic, not UTF-8 text).
    let unknown = upload(
        &client,
        &base,
        "alpha",
        vec![0x00, 0x01, 0x02, 0x03, 0x7F, b'E', b'L', b'F'],
        "application/octet-stream",
        "blob.bin",
    )
    .await;
    assert_eq!(unknown.status(), 415, "unsupported type rejected");

    // A message citing an id that was never uploaded here → 400, no message.
    let fake = "a".repeat(64);
    let bad = send_msg(&client, &base, "alpha", "cite a ghost", &[&fake]).await;
    assert_eq!(bad.status(), 400, "unknown attachment id rejects the post");

    // Citing more than the per-message cap → 400.
    let ids: Vec<String> = (0..5).map(|i| format!("{i}").repeat(64)).collect();
    let refs: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
    let too_many = send_msg(&client, &base, "alpha", "too many", &refs).await;
    assert_eq!(
        too_many.status(),
        400,
        "over the per-message attachment cap"
    );

    // The ghost citation left no message behind.
    let history: Vec<Value> = client
        .get(format!("{base}/rooms/alpha/messages?since=0"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        history.iter().all(|m| m["text"] != "cite a ghost"),
        "a rejected post must not persist"
    );
}

#[tokio::test]
async fn caption_less_image_is_allowed_but_fully_empty_is_not() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("att.sqlite3");
    let (base, _g) = spawn(&db.to_string_lossy()).await;
    let client = reqwest::Client::new();

    // An empty message with no attachments is still rejected.
    let empty = send_msg(&client, &base, "alpha", "", &[]).await;
    assert_eq!(empty.status(), 400, "a truly empty message is rejected");

    // But an image with no caption is a real message.
    let png = png_bytes(3);
    let up: Value = upload(
        &client,
        &base,
        "alpha",
        png.clone(),
        "image/png",
        "shot.png",
    )
    .await
    .json()
    .await
    .unwrap();
    let id = up["id"].as_str().unwrap();
    let msg = send_msg(&client, &base, "alpha", "", &[id]).await;
    assert_eq!(
        msg.status(),
        201,
        "an attachment with empty text is allowed"
    );
}

#[tokio::test]
async fn identical_bytes_dedupe_to_one_id() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("att.sqlite3");
    let (base, _g) = spawn(&db.to_string_lossy()).await;
    let client = reqwest::Client::new();
    let bytes = png_bytes(7);

    let a: Value = upload(
        &client,
        &base,
        "alpha",
        bytes.clone(),
        "image/png",
        "one.png",
    )
    .await
    .json()
    .await
    .unwrap();
    let b: Value = upload(
        &client,
        &base,
        "alpha",
        bytes.clone(),
        "image/png",
        "two.png",
    )
    .await
    .json()
    .await
    .unwrap();
    assert_eq!(
        a["id"].as_str().unwrap(),
        b["id"].as_str().unwrap(),
        "identical bytes are content-addressed to the same id"
    );
}
