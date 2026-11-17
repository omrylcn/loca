//! The Master's delete of a closed loca (`DELETE /rooms/:id/records`) and
//! the zip of its records taken before it (`GET /rooms/:id/archive`). Seal
//! keeps every record; delete removes them from the server, and an
//! attachment file another loca still cites stays.
use super::*;
use std::io::Read;

const MASTER: &str = "delete-master-key";

async fn call(
    method: reqwest::Method,
    url: String,
    master: bool,
    body: Option<Value>,
) -> reqwest::Response {
    let mut req = reqwest::Client::new().request(method, url);
    if master {
        req = req.header("x-admin-token", MASTER);
    }
    if let Some(body) = body {
        req = req.json(&body);
    }
    req.send().await.unwrap()
}

async fn upload(base: &str, room: &str, bytes: &'static [u8]) -> String {
    let r = reqwest::Client::new()
        .post(format!("{base}/rooms/{room}/attachments"))
        .header("x-admin-token", MASTER)
        .header("content-type", "text/plain")
        .body(bytes)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "upload: {}", r.status());
    r.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn say(base: &str, room: &str, text: &str, attachments: &[&str]) {
    let r = call(
        reqwest::Method::POST,
        format!("{base}/rooms/{room}/messages"),
        true,
        Some(serde_json::json!({
            "sender": "MASTER", "sender_type": "user", "text": text,
            "attachments": attachments,
        })),
    )
    .await;
    assert!(r.status().is_success(), "post: {}", r.status());
}

fn rows(db: &std::path::Path, sql: &str) -> i64 {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.query_row(sql, [], |row| row.get(0)).unwrap()
}

#[tokio::test]
async fn a_closed_loca_is_deleted_with_its_records_after_its_zip() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("delete.db");
    let env = [("DB_PATH", db.to_str().unwrap().to_string())];
    let (port, guard) = spawn_server_env(MASTER, &env).await;
    let base = format!("http://127.0.0.1:{port}");
    create_test_room(port, MASTER, "gone").await;
    create_test_room(port, MASTER, "kept").await;

    // One file both locas cite, one only the deleted loca cites.
    let shared = upload(&base, "gone", b"a file two locas share\n").await;
    let own = upload(&base, "gone", b"a file only this loca has\n").await;
    say(&base, "gone", "the secret words", &[&shared, &own]).await;
    assert_eq!(
        upload(&base, "kept", b"a file two locas share\n").await,
        shared
    );
    say(&base, "kept", "kept words", &[&shared]).await;
    let note = call(
        reqwest::Method::POST,
        format!("{base}/rooms/gone/notes"),
        true,
        Some(serde_json::json!({ "key": "plan", "title": "Plan", "body": "note words", "by": "MASTER" })),
    )
    .await;
    assert!(note.status().is_success(), "note: {}", note.status());

    // An open loca is not deleted.
    let open = call(
        reqwest::Method::DELETE,
        format!("{base}/rooms/gone/records"),
        true,
        None,
    )
    .await;
    assert_eq!(open.status(), 409);
    assert_eq!(
        rows(&db, "SELECT COUNT(*) FROM messages WHERE room = 'gone'"),
        1
    );
    let closed = call(
        reqwest::Method::PUT,
        format!("{base}/rooms/gone/settings"),
        true,
        Some(serde_json::json!({ "archived": true })),
    )
    .await;
    assert!(closed.status().is_success(), "close: {}", closed.status());

    // The zip is the Master's: words, rows and files, and no secret.
    let outsider = call(
        reqwest::Method::GET,
        format!("{base}/rooms/gone/archive"),
        false,
        None,
    )
    .await;
    assert_eq!(outsider.status(), 401);
    let archive = call(
        reqwest::Method::GET,
        format!("{base}/rooms/gone/archive"),
        true,
        None,
    )
    .await;
    assert_eq!(archive.status(), 200);
    assert_eq!(archive.headers()["content-type"], "application/zip");
    assert!(archive.headers()["content-disposition"]
        .to_str()
        .unwrap()
        .contains("loca-gone-archive.zip"));
    let bytes = archive.bytes().await.unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
    fn read(zip: &mut zip::ZipArchive<std::io::Cursor<Vec<u8>>>, name: &str) -> String {
        let mut text = String::new();
        zip.by_name(name)
            .unwrap_or_else(|_| panic!("{name} in the zip"))
            .read_to_string(&mut text)
            .unwrap();
        text
    }
    assert!(read(&mut zip, "transcript.txt").contains("MASTER: the secret words"));
    assert!(read(&mut zip, "data/messages.json").contains("the secret words"));
    assert!(read(&mut zip, "data/notes.json").contains("note words"));
    assert!(
        read(&mut zip, "manifest.json").contains("loca_vault"),
        "the manifest names what it left out"
    );
    let names: Vec<String> = zip
        .file_names()
        .map(|name| name.expect("an entry name").into_owned())
        .collect();
    let own_entry = names
        .iter()
        .find(|n| n.starts_with(&format!("attachments/{}", &own[..12])))
        .expect("the loca's own file is in the zip")
        .clone();
    assert_eq!(read(&mut zip, &own_entry), "a file only this loca has\n");
    assert_eq!(
        names
            .iter()
            .filter(|n| n.starts_with("attachments/"))
            .count(),
        2,
        "{names:?}"
    );
    assert!(
        !names
            .iter()
            .any(|n| n.contains("invites") || n.contains("vault")),
        "{names:?}"
    );

    // Deleted: every row the loca owned, and the file nobody else cites.
    let deleted = call(
        reqwest::Method::DELETE,
        format!("{base}/rooms/gone/records"),
        true,
        None,
    )
    .await;
    assert_eq!(deleted.status(), 204);
    for table in [
        "messages",
        "notes",
        "note_revisions",
        "attachment_refs",
        "journal",
    ] {
        assert_eq!(
            rows(
                &db,
                &format!("SELECT COUNT(*) FROM {table} WHERE room = 'gone'")
            ),
            0,
            "{table}"
        );
    }
    assert_eq!(
        rows(&db, "SELECT COUNT(*) FROM rooms WHERE room = 'gone' AND sealed_at IS NOT NULL AND settings = '{}'"),
        1,
        "the name stays, sealed and empty"
    );
    let blob = |sha: &str| dir.path().join("attachments").join(&sha[..2]).join(sha);
    assert!(
        !blob(&own).exists(),
        "the file only the deleted loca cited is gone"
    );
    assert!(blob(&shared).exists(), "the file another loca cites stays");
    let still = call(
        reqwest::Method::GET,
        format!("{base}/rooms/kept/attachments/{shared}"),
        true,
        None,
    )
    .await;
    assert_eq!(still.status(), 200);
    assert_eq!(
        rows(&db, "SELECT COUNT(*) FROM messages WHERE room = 'kept'"),
        1
    );

    // The name does not come back, also after a restart.
    let again = call(
        reqwest::Method::POST,
        format!("{base}/rooms"),
        true,
        Some(serde_json::json!({ "name": "gone" })),
    )
    .await;
    assert_eq!(again.status(), 409);
    drop(guard);
    wait_for_server_shutdown(port).await;
    let (port, _guard) = spawn_server_env(MASTER, &env).await;
    let base = format!("http://127.0.0.1:{port}");
    let again = call(
        reqwest::Method::POST,
        format!("{base}/rooms"),
        true,
        Some(serde_json::json!({ "name": "gone" })),
    )
    .await;
    assert_eq!(again.status(), 409);
    let empty = call(
        reqwest::Method::GET,
        format!("{base}/rooms/gone/archive"),
        true,
        None,
    )
    .await;
    assert_eq!(
        empty.status(),
        200,
        "a deleted loca's archive is empty, not refused"
    );
}
