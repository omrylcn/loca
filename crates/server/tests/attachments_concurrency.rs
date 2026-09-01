//! Concurrency proof for the attachment quota + dedup accounting (loca-dev's
//! acceptance gate). The check-then-act in `put_pending_attachment` runs under
//! the store's single connection `Mutex`, held across the whole quota check AND
//! the insert, so concurrent uploads serialize — the second sees the first's
//! committed row. These tests fire many uploads at once against the real binary
//! and assert the invariant that a race would break: the admitted total never
//! exceeds the configured quota. (Quotas are shrunk via env so small files
//! exercise the limit.)

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::net::TcpListener;

struct ServerGuard {
    _child: tokio::process::Child,
}

async fn spawn(db_path: &str, env: &[(&str, &str)]) -> (String, ServerGuard) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let bin = env!("CARGO_BIN_EXE_room-server");
    let mut cmd = tokio::process::Command::new(bin);
    cmd.env("PORT", port.to_string())
        .env("RUST_LOG", "warn")
        .env("ADMIN_TOKEN", "")
        .env("DB_PATH", db_path);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let child = cmd
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

/// A `size`-byte blob that sniffs as PNG and is unique per `tag` (distinct sha).
fn png_of(size: usize, tag: u8) -> Vec<u8> {
    let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    v.resize(size.max(v.len()), tag);
    v
}

async fn upload(
    client: &reqwest::Client,
    base: &str,
    room: &str,
    bytes: Vec<u8>,
) -> (u16, Option<Value>) {
    let r = client
        .post(format!("{base}/rooms/{room}/attachments"))
        .header("content-type", "image/png")
        .header("x-filename", "f.png")
        .body(bytes)
        .send()
        .await
        .unwrap();
    let code = r.status().as_u16();
    let json = if code == 201 {
        r.json().await.ok()
    } else {
        None
    };
    (code, json)
}

/// N identical uploads at once must all succeed and dedupe to the SAME id — one
/// physical blob, no matter the concurrency.
#[tokio::test]
async fn parallel_identical_uploads_dedupe_to_one_id() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("c.sqlite3");
    let (base, _g) = spawn(&db.to_string_lossy(), &[]).await;
    let client = Arc::new(reqwest::Client::new());
    let bytes = png_of(4096, 9);

    let mut handles = Vec::new();
    for _ in 0..12 {
        let c = client.clone();
        let b = base.clone();
        let data = bytes.clone();
        handles.push(tokio::spawn(
            async move { upload(&c, &b, "alpha", data).await },
        ));
    }
    let mut ids = std::collections::HashSet::new();
    for h in handles {
        let (code, json) = h.await.unwrap();
        assert_eq!(code, 201, "every identical upload is accepted");
        ids.insert(json.unwrap()["id"].as_str().unwrap().to_string());
    }
    assert_eq!(ids.len(), 1, "identical bytes dedupe to exactly one id");
}

/// The race loca-dev named: distinct blobs that each fit alone but TOGETHER
/// exceed the room quota, uploaded at once. Only up to the quota may be
/// admitted — the admitted total must never exceed it (a check-then-act race
/// would over-admit and break this).
#[tokio::test]
async fn concurrent_distinct_uploads_never_exceed_room_quota() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("c.sqlite3");
    // Room quota 5000 B; each blob 2000 B. Two fit (4000 ≤ 5000), a third would
    // reach 6000 > 5000. Per-file cap stays at its default so size never gates.
    let (base, _g) = spawn(
        &db.to_string_lossy(),
        &[("ATTACHMENT_ROOM_MAX_BYTES", "5000")],
    )
    .await;
    let client = Arc::new(reqwest::Client::new());
    const BLOB: usize = 2000;
    const QUOTA: usize = 5000;

    let mut handles = Vec::new();
    for i in 0..8u8 {
        let c = client.clone();
        let b = base.clone();
        let data = png_of(BLOB, i + 1); // distinct content per i
        handles.push(tokio::spawn(
            async move { upload(&c, &b, "alpha", data).await },
        ));
    }
    let (mut accepted, mut rejected) = (0usize, 0usize);
    for h in handles {
        let (code, _) = h.await.unwrap();
        match code {
            201 => accepted += 1,
            413 => rejected += 1,
            other => panic!("unexpected status {other}"),
        }
    }
    // The invariant a race would break: admitted bytes never exceed the quota.
    assert!(
        accepted * BLOB <= QUOTA,
        "over-admitted: {accepted} × {BLOB} B exceeds the {QUOTA} B quota"
    );
    assert!(accepted >= 1, "at least one upload fits");
    assert!(rejected >= 1, "the quota must actually reject the overflow");
}

/// Building quota under concurrency, across DIFFERENT rooms: distinct blobs from
/// several rooms at once. The building counts unique physical bytes, so the
/// admitted total across all rooms must not exceed the building quota.
#[tokio::test]
async fn concurrent_uploads_across_rooms_never_exceed_building_quota() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("c.sqlite3");
    let (base, _g) = spawn(
        &db.to_string_lossy(),
        &[("ATTACHMENT_BUILDING_MAX_BYTES", "5000")],
    )
    .await;
    let client = Arc::new(reqwest::Client::new());
    const BLOB: usize = 2000;
    const QUOTA: usize = 5000;

    let rooms = ["alpha", "beta", "gamma"];
    let mut handles = Vec::new();
    for i in 0..9u8 {
        let c = client.clone();
        let b = base.clone();
        let room = rooms[i as usize % rooms.len()].to_string();
        let data = png_of(BLOB, i + 1); // globally distinct blobs
        handles.push(tokio::spawn(
            async move { upload(&c, &b, &room, data).await },
        ));
    }
    let mut accepted = 0usize;
    for h in handles {
        let (code, _) = h.await.unwrap();
        if code == 201 {
            accepted += 1;
        } else {
            assert_eq!(code, 413, "over-building-quota uploads are 413");
        }
    }
    assert!(
        accepted * BLOB <= QUOTA,
        "over-admitted building-wide: {accepted} × {BLOB} B exceeds {QUOTA} B"
    );
    assert!(accepted >= 1);
}

/// Many messages citing the SAME uploaded blob at once must all be accepted and
/// the blob stays fetchable — concurrent pending→referenced flips don't race.
#[tokio::test]
async fn concurrent_cites_of_one_blob_all_succeed() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("c.sqlite3");
    let (base, _g) = spawn(&db.to_string_lossy(), &[]).await;
    let client = Arc::new(reqwest::Client::new());

    let (code, json) = upload(&client, &base, "alpha", png_of(1024, 5)).await;
    assert_eq!(code, 201);
    let id = json.unwrap()["id"].as_str().unwrap().to_string();

    let mut handles = Vec::new();
    for i in 0..10 {
        let c = client.clone();
        let b = base.clone();
        let att = id.clone();
        handles.push(tokio::spawn(async move {
            c.post(format!("{b}/rooms/alpha/messages"))
                .json(&serde_json::json!({
                    "sender": "tester", "sender_type": "agent",
                    "text": format!("cite {i}"), "attachments": [att],
                }))
                .send()
                .await
                .unwrap()
                .status()
                .as_u16()
        }));
    }
    for h in handles {
        assert_eq!(
            h.await.unwrap(),
            201,
            "every concurrent citation is accepted"
        );
    }
    let got = client
        .get(format!("{base}/rooms/alpha/attachments/{id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(got.status(), 200, "the shared blob stays fetchable");
}
